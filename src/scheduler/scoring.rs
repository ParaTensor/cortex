use std::collections::HashSet;
use std::sync::Arc;
use dashmap::DashMap;
use rand::Rng;
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

    /// Selects the best worker for a given request using the strict 4-tier scheduling fallback chain.
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

        // -------------------------------------------------------------
        // Tier 1: Exact KV Events Matching (Only for READY workers)
        // -------------------------------------------------------------
        let ready_worker_ids: HashSet<String> = eligible_workers
            .iter()
            .filter(|w| *w.status.read() == WorkerSyncStatus::Ready)
            .map(|w| w.config.id.clone())
            .collect();

        if !page_hashes.is_empty() && !ready_worker_ids.is_empty() {
            let matches = self.tree.find_lcp_matches(page_hashes, &ready_worker_ids);
            if !matches.is_empty() {
                let mut best_worker: Option<(Arc<WorkerRuntimeState>, usize, f64)> = None;

                for worker in &eligible_workers {
                    if let Some(&matched) = matches.get(&worker.config.id) {
                        let active = worker.get_active_requests();
                        // Overload avoidance check: if worker is beyond high-watermark, skip KV affinity
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

        // -------------------------------------------------------------
        // Tier 2: Power of Two Choices (P2C) Randomized Load Balancing
        // (Active when enabled and at least 2 candidate workers exist)
        // -------------------------------------------------------------
        if self.config.enable_p2c && eligible_workers.len() >= 2 {
            let mut rng = rand::thread_rng();
            let idx_a = rng.gen_range(0..eligible_workers.len());
            let mut idx_b = rng.gen_range(0..eligible_workers.len());
            while idx_b == idx_a {
                idx_b = rng.gen_range(0..eligible_workers.len());
            }

            let worker_a = &eligible_workers[idx_a];
            let worker_b = &eligible_workers[idx_b];

            let chosen = if worker_a.get_active_requests() <= worker_b.get_active_requests() {
                worker_a
            } else {
                worker_b
            };

            return Some(SchedulingDecision {
                worker_id: chosen.config.id.clone(),
                http_endpoint: chosen.config.http_endpoint.clone(),
                matched_pages: 0,
                mode: RoutingMode::FallbackP2c,
            });
        }

        // -------------------------------------------------------------
        // Tier 3: Load Aware (Least Active Connections across all)
        // -------------------------------------------------------------
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

        // -------------------------------------------------------------
        // Tier 4: Fallback Round Robin
        // -------------------------------------------------------------
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
    fn test_scheduler_exact_kv_and_p2c_fallback() {
        let tree = Arc::new(RadixHashTree::new());
        let workers = Arc::new(DashMap::new());

        let w1_cfg = WorkerConfig {
            id: "worker-1".to_string(),
            model: "test-model".to_string(),
            engine: EngineType::Sglang,
            http_endpoint: "http://127.0.0.1:8001".to_string(),
            zmq_endpoint: None,
            tokenizer_path: None,
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
            tokenizer_path: None,
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

        let scheduler = LocalityScheduler::new(SchedulerConfig::default(), tree.clone(), workers.clone());

        // Preload KV on w1
        let hashes = vec![111, 222, 333];
        tree.insert_chain("worker-1", &hashes);

        // Case 1: Exact KV match
        let decision = scheduler.select_worker("test-model", &hashes, None).unwrap();
        assert_eq!(decision.worker_id, "worker-1");
        assert_eq!(decision.mode, RoutingMode::ExactKvEvents);
        assert_eq!(decision.matched_pages, 3);

        // Case 2: No KV match -> P2C Fallback (with 2 workers and enable_p2c: true)
        let unseeded_hashes = vec![999];
        let p2c_decision = scheduler.select_worker("test-model", &unseeded_hashes, None).unwrap();
        assert_eq!(p2c_decision.mode, RoutingMode::FallbackP2c);
        assert_eq!(p2c_decision.matched_pages, 0);
    }
}
