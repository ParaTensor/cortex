use std::sync::Arc;
use std::time::Duration;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use tracing::{error, info, warn};
use zeromq::{Socket, SocketRecv, SubSocket};

use crate::ledger::{RadixHashTree, WorkerRuntimeState, WorkerSyncStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KvEventPayload {
    BlockStored {
        page_hashes: Vec<i64>,
    },
    BlockRemoved {
        page_hashes: Vec<i64>,
    },
    AllBlocksCleared,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KvEventMessage {
    pub seq: u64,
    pub payload: KvEventPayload,
}

pub struct KvEventProcessor {
    tree: Arc<RadixHashTree>,
}

impl KvEventProcessor {
    pub fn new(tree: Arc<RadixHashTree>) -> Self {
        Self { tree }
    }

    /// Processes an incoming KV event for a worker, validating strict sequence monotonicity.
    pub fn process_event(&self, worker: &WorkerRuntimeState, event: KvEventMessage) {
        let mut last_seq_guard = worker.last_seq.write();
        let expected_seq = *last_seq_guard + 1;

        if *last_seq_guard > 0 && event.seq != expected_seq {
            warn!(
                worker_id = %worker.config.id,
                expected_seq = expected_seq,
                actual_seq = event.seq,
                "ZMQ event sequence gap detected. Marking worker as STALE."
            );
            worker.set_status(WorkerSyncStatus::Stale);
            // Clear dirty radix ledger entries for this stale worker
            self.tree.clear_worker(&worker.config.id);
            *last_seq_guard = event.seq;
            return;
        }

        *last_seq_guard = event.seq;
        worker.update_heartbeat();

        match event.payload {
            KvEventPayload::BlockStored { page_hashes } => {
                self.tree.insert_chain(&worker.config.id, &page_hashes);
                if *worker.status.read() == WorkerSyncStatus::Syncing || *worker.status.read() == WorkerSyncStatus::Init {
                    worker.set_status(WorkerSyncStatus::Ready);
                }
            }
            KvEventPayload::BlockRemoved { .. } => {
                // Individual block removal
            }
            KvEventPayload::AllBlocksCleared => {
                info!(worker_id = %worker.config.id, "All blocks cleared on worker. Resetting ledger branch.");
                self.tree.clear_worker(&worker.config.id);
            }
        }
    }
}

/// Spawns a dedicated background Tokio task to subscribe to a Worker's ZMQ event stream.
pub fn spawn_worker_zmq_subscriber(
    worker: Arc<WorkerRuntimeState>,
    processor: Arc<KvEventProcessor>,
) {
    let endpoint = match &worker.config.zmq_endpoint {
        Some(ep) if !ep.is_empty() => ep.clone(),
        _ => return,
    };

    let worker_id = worker.config.id.clone();

    tokio::spawn(async move {
        info!(worker_id = %worker_id, endpoint = %endpoint, "Starting ZMQ event subscriber loop");

        loop {
            let mut socket = SubSocket::new();
            match socket.connect(&endpoint).await {
                Ok(_) => {
                    info!(worker_id = %worker_id, endpoint = %endpoint, "Connected to worker ZMQ endpoint");
                    if let Err(e) = socket.subscribe("").await {
                        error!(worker_id = %worker_id, error = %e, "Failed to subscribe to ZMQ topic");
                        sleep(Duration::from_secs(2)).await;
                        continue;
                    }

                    // Set status to Syncing upon connection
                    if *worker.status.read() == WorkerSyncStatus::Init {
                        worker.set_status(WorkerSyncStatus::Syncing);
                    }

                    while let Ok(msg) = socket.recv().await {
                        // SGLang multipart message format: [topic, seq/payload] or single JSON frame
                        for part in msg.iter() {
                            if let Ok(event) = serde_json::from_slice::<KvEventMessage>(part) {
                                processor.process_event(&worker, event);
                            }
                        }
                    }

                    warn!(worker_id = %worker_id, "ZMQ connection disconnected. Marking worker as STALE.");
                    worker.set_status(WorkerSyncStatus::Stale);
                }
                Err(e) => {
                    warn!(worker_id = %worker_id, endpoint = %endpoint, error = %e, "Failed to connect to worker ZMQ endpoint, retrying in 2s...");
                }
            }

            sleep(Duration::from_secs(2)).await;
        }
    });
}

/// Spawns background ZMQ subscribers for all registered workers in the cluster.
pub fn spawn_all_worker_zmq_subscribers(
    workers: &DashMap<String, Arc<WorkerRuntimeState>>,
    processor: Arc<KvEventProcessor>,
) {
    for entry in workers.iter() {
        let worker = entry.value().clone();
        if worker.config.zmq_endpoint.is_some() {
            spawn_worker_zmq_subscriber(worker, processor.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EngineType, WorkerConfig, WorkerRole};

    #[test]
    fn test_kv_event_processor_seq_gap() {
        let tree = Arc::new(RadixHashTree::new());
        let processor = KvEventProcessor::new(tree.clone());

        let cfg = WorkerConfig {
            id: "worker-1".to_string(),
            model: "test".to_string(),
            engine: EngineType::Sglang,
            http_endpoint: "http://127.0.0.1:8000".to_string(),
            zmq_endpoint: None,
            role: WorkerRole::Standard,
            page_size: 16,
            weight: 100,
        };
        let worker = WorkerRuntimeState::new(cfg);

        // First event (seq 1)
        processor.process_event(
            &worker,
            KvEventMessage {
                seq: 1,
                payload: KvEventPayload::BlockStored {
                    page_hashes: vec![12345],
                },
            },
        );
        assert_eq!(*worker.status.read(), WorkerSyncStatus::Ready);

        // Gap event (seq 5 instead of 2) -> should mark STALE
        processor.process_event(
            &worker,
            KvEventMessage {
                seq: 5,
                payload: KvEventPayload::BlockStored {
                    page_hashes: vec![67890],
                },
            },
        );
        assert_eq!(*worker.status.read(), WorkerSyncStatus::Stale);
    }
}
