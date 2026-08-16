use std::collections::HashSet;
use std::sync::Arc;
use dashmap::DashMap;
use crate::config::{SchedulerConfig, WorkerRole};
use crate::ledger::{RadixHashTree, WorkerRuntimeState, WorkerSyncStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingMode {
    ExactKvEvents,
    LoadAware,
    FallbackP2c,
    FallbackRoundRobin,
}

impl RoutingMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ExactKvEvents => "exact_kv_events",
            Self::LoadAware => "load_aware",
            Self::FallbackP2c => "fallback_p2c",
            Self::FallbackRoundRobin => "fallback_round_robin",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SchedulingDecision {
    pub worker_id: String,
    pub http_endpoint: String,
    pub matched_pages: usize,
    pub mode: RoutingMode,
}

pub struct LocalityScheduler {
    config: SchedulerConfig,
    tree: Arc<RadixHashTree>,
    workers: Arc<DashMap<String, Arc<WorkerRuntimeState>>>,
    rr_counter: std::sync::atomic::AtomicUsize,
}

impl LocalityScheduler {
    pub fn new(
        config: SchedulerConfig,
        tree: Arc<RadixHashTree>,
        workers: Arc<DashMap<String, Arc<WorkerRuntimeState>>>,
    ) -> Self {
        Self {
            config,
            tree,
            workers,
            rr_counter: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Selects the best worker for a given request.
    pub fn select_worker(
        &self,
        model_id: &str,
        page_hashes: &[i64],
        required_role: Option<WorkerRole>,
    ) -> Option<SchedulingDecision> {
        let role = required_role.unwrap_or(WorkerRole::Standard);

        // Filter eligible workers matching model and role
        let eligible_workers: Vec<Arc<WorkerRuntimeState>> = self
            .workers
            .iter()
            .filter(|entry| {
                let w = entry.value();
                w.config.model == model_id && (w.config.role == role || w.config.role == WorkerRole::Standard)
            })
            .map(|entry| entry.value().clone())
            .collect();

        if eligible_workers.is_empty() {
            return None;
        }

        // 1. Tier 1: Try Exact KV Events Matching (Only for READY workers)
        let ready_worker_ids: HashSet<String> = eligible_workers
            .iter()
            .filter(|w| *w.status.read() == WorkerSyncStatus::Ready)
            .map(|w| w.config.id.clone())
            .collect();

        if !page_hashes.is_empty() && !ready_worker_ids.is_empty() {
            let matches = self.tree.find_lcp_matches(page_hashes, &ready_worker_ids);
            if !matches.is_empty() {
                // Find worker with highest score: kv_weight * matched_pages - load_weight * active_requests
                let mut best_worker: Option<(Arc<WorkerRuntimeState>, usize, f64)> = None;

                for worker in &eligible_workers {
                    if let Some(&matched) = matches.get(&worker.config.id) {
                        let active = worker.get_active_requests();
                        // Overload avoidance check
                        if active < self.config.max_active_requests_per_worker {
                            let score = (self.config.kv_weight * matched as f64)
                                - (self.config.load_weight * active as f64);

                            if let Some((_, _, best_score)) = best_worker {
                                if score > best_score {
                                    best_worker = Some((worker.clone(), matched, score));
                                }
                            } else {
                                best_worker = Some((worker.clone(), matched, score));
                            }
                        }
                    }
                }

                if let Some((worker, matched, _)) = best_worker {
                    return Some(SchedulingDecision {
                        worker_id: worker.config.id.clone(),
                        http_endpoint: worker.config.http_endpoint.clone(),
                        matched_pages: matched,
                        mode: RoutingMode::ExactKvEvents,
                    });
                }
            }
        }

        // 2. Tier 2: Load Aware (Least Active Connections)
        let mut min_active = usize::MAX;
        let mut least_loaded_worker: Option<Arc<WorkerRuntimeState>> = None;

        for worker in &eligible_workers {
            let active = worker.get_active_requests();
            if active < min_active {
                min_active = active;
                least_loaded_worker = Some(worker.clone());
            }
        }

        if let Some(worker) = least_loaded_worker {
            return Some(SchedulingDecision {
                worker_id: worker.config.id.clone(),
                http_endpoint: worker.config.http_endpoint.clone(),
                matched_pages: 0,
                mode: RoutingMode::LoadAware,
            });
        }

        // 3. Tier 3: Fallback Round Robin
        let idx = self.rr_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % eligible_workers.len();
        let fallback_worker = &eligible_workers[idx];

        Some(SchedulingDecision {
            worker_id: fallback_worker.config.id.clone(),
            http_endpoint: fallback_worker.config.http_endpoint.clone(),
            matched_pages: 0,
            mode: RoutingMode::FallbackRoundRobin,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EngineType, WorkerConfig};

    #[test]
    fn test_scheduler_exact_kv_and_load_fallback() {
        let tree = Arc::new(RadixHashTree::new());
        let workers = Arc::new(DashMap::new());

        let w1_cfg = WorkerConfig {
            id: "worker-1".to_string(),
            model: "test-model".to_string(),
            engine: EngineType::Sglang,
            http_endpoint: "http://127.0.0.1:8001".to_string(),
            zmq_endpoint: None,
            role: WorkerRole::Standard,
            page_size: 16,
            weight: 100,
        };
        let w2_cfg = WorkerConfig {
            id: "worker-2".to_string(),
            model: "test-model".to_string(),
            engine: EngineType::Sglang,
            http_endpoint: "http://127.0.0.1:8002".to_string(),
            zmq_endpoint: None,
            role: WorkerRole::Standard,
            page_size: 16,
            weight: 100,
        };

        let w1 = Arc::new(WorkerRuntimeState::new(w1_cfg));
        let w2 = Arc::new(WorkerRuntimeState::new(w2_cfg));

        w1.set_status(WorkerSyncStatus::Ready);
        w2.set_status(WorkerSyncStatus::Ready);

        workers.insert("worker-1".to_string(), w1.clone());
        workers.insert("worker-2".to_string(), w2.clone());

        let scheduler = LocalityScheduler::new(SchedulerConfig::default(), tree.clone(), workers);

        // Preload KV on w1
        let hashes = vec![111, 222, 333];
        tree.insert_chain("worker-1", &hashes);

        // Should select worker-1 with exact_kv_events
        let decision = scheduler.select_worker("test-model", &hashes, None).unwrap();
        assert_eq!(decision.worker_id, "worker-1");
        assert_eq!(decision.mode, RoutingMode::ExactKvEvents);
        assert_eq!(decision.matched_pages, 3);
    }
}
