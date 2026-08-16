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
    #[serde(alias = "store", alias = "BlockStored")]
    BlockStored {
        page_hashes: Vec<i64>,
    },
    #[serde(alias = "remove", alias = "BlockRemoved")]
    BlockRemoved {
        page_hashes: Vec<i64>,
    },
    #[serde(alias = "clear", alias = "AllBlocksCleared")]
    AllBlocksCleared,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KvEventMessage {
    pub seq: u64,
    #[serde(flatten)]
    pub payload: KvEventPayload,
}

pub struct KvEventProcessor {
    tree: Arc<RadixHashTree>,
}

impl KvEventProcessor {
    pub fn new(tree: Arc<RadixHashTree>) -> Self {
        Self { tree }
    }

    /// Parses raw frame bytes, supporting both MessagePack (SGLang native) and JSON.
    pub fn parse_frame(bytes: &[u8]) -> Option<KvEventMessage> {
        // Try MessagePack first (standard SGLang SRT format)
        if let Ok(event) = rmp_serde::from_slice::<KvEventMessage>(bytes) {
            return Some(event);
        }
        // Fallback to JSON
        if let Ok(event) = serde_json::from_slice::<KvEventMessage>(bytes) {
            return Some(event);
        }
        None
    }

    /// Processes an incoming KV event for a worker, validating strict sequence monotonicity
    /// and enforcing the ledger synchronization gatekeeper.
    pub fn process_event(&self, worker: &WorkerRuntimeState, event: KvEventMessage) {
        let mut last_seq_guard = worker.last_seq.write();
        let current_status = *worker.status.read();
        let expected_seq = *last_seq_guard + 1;

        // 1. Seq Monotonicity Check
        if *last_seq_guard > 0 && event.seq != expected_seq {
            warn!(
                worker_id = %worker.config.id,
                expected_seq = expected_seq,
                actual_seq = event.seq,
                "ZMQ event sequence gap detected. Marking worker as STALE and pruning dirty ledger."
            );
            worker.set_status(WorkerSyncStatus::Stale);
            self.tree.clear_worker(&worker.config.id);
            *last_seq_guard = event.seq;
            return;
        }

        *last_seq_guard = event.seq;
        worker.update_heartbeat();

        // 2. Ledger Sync Gatekeeper
        match event.payload {
            KvEventPayload::BlockStored { page_hashes } => {
                if current_status == WorkerSyncStatus::Stale {
                    // Critical Gatekeeper: STALE workers cannot accept incremental block insertions
                    // until baseline is fully reset via AllBlocksCleared or snapshot sync.
                    warn!(
                        worker_id = %worker.config.id,
                        seq = event.seq,
                        "Worker is STALE. Ignoring incremental BlockStored event to prevent ledger pollution."
                    );
                    return;
                }

                self.tree.insert_chain(&worker.config.id, &page_hashes);
                if current_status == WorkerSyncStatus::Syncing || current_status == WorkerSyncStatus::Init {
                    info!(worker_id = %worker.config.id, "Worker synchronized. Transitioning to READY.");
                    worker.set_status(WorkerSyncStatus::Ready);
                }
            }
            KvEventPayload::BlockRemoved { page_hashes } => {
                if current_status == WorkerSyncStatus::Stale {
                    return;
                }
                self.tree.remove_chain(&worker.config.id, &page_hashes);
            }
            KvEventPayload::AllBlocksCleared => {
                info!(worker_id = %worker.config.id, "All blocks cleared on worker. Baseline reset; transitioning to READY.");
                self.tree.clear_worker(&worker.config.id);
                // Baseline cleanly reset -> worker is now clean and Ready
                worker.set_status(WorkerSyncStatus::Ready);
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

                    if *worker.status.read() == WorkerSyncStatus::Init {
                        worker.set_status(WorkerSyncStatus::Syncing);
                    }

                    while let Ok(msg) = socket.recv().await {
                        // SGLang multipart message format: [topic, payload] or single payload frame
                        for part in msg.iter() {
                            if let Some(event) = KvEventProcessor::parse_frame(part) {
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
    fn test_kv_event_processor_msgpack_and_json_parsing() {
        let event = KvEventMessage {
            seq: 42,
            payload: KvEventPayload::BlockStored {
                page_hashes: vec![101, 102, 103],
            },
        };

        // Test JSON parsing
        let json_bytes = serde_json::to_vec(&event).unwrap();
        let parsed_json = KvEventProcessor::parse_frame(&json_bytes).unwrap();
        assert_eq!(parsed_json.seq, 42);

        // Test MessagePack parsing
        let msgpack_bytes = rmp_serde::to_vec(&event).unwrap();
        let parsed_msgpack = KvEventProcessor::parse_frame(&msgpack_bytes).unwrap();
        assert_eq!(parsed_msgpack.seq, 42);
    }

    #[test]
    fn test_kv_event_processor_stale_gatekeeper() {
        let tree = Arc::new(RadixHashTree::new());
        let processor = KvEventProcessor::new(tree.clone());

        let cfg = WorkerConfig {
            id: "worker-1".to_string(),
            model: "test".to_string(),
            engine: EngineType::Sglang,
            http_endpoint: "http://127.0.0.1:8000".to_string(),
            zmq_endpoint: None,
            tokenizer_path: None,
            role: WorkerRole::Standard,
            page_size: 16,
            weight: 100,
        };
        let worker = WorkerRuntimeState::new(cfg);

        // Event 1: Normal initialization
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
        assert_eq!(tree.total_cached_blocks(), 1);

        // Event 2: Gap detected (seq 5 instead of 2) -> Worker becomes STALE and tree is cleared
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
        assert_eq!(tree.total_cached_blocks(), 0);

        // Event 3: Incremental BlockStored while STALE must NOT re-enter Ready and must NOT pollute tree
        processor.process_event(
            &worker,
            KvEventMessage {
                seq: 6,
                payload: KvEventPayload::BlockStored {
                    page_hashes: vec![99999],
                },
            },
        );
        assert_eq!(*worker.status.read(), WorkerSyncStatus::Stale);
        assert_eq!(tree.total_cached_blocks(), 0);

        // Event 4: AllBlocksCleared cleanly resets baseline -> Worker transitions back to Ready
        processor.process_event(
            &worker,
            KvEventMessage {
                seq: 7,
                payload: KvEventPayload::AllBlocksCleared,
            },
        );
        assert_eq!(*worker.status.read(), WorkerSyncStatus::Ready);
    }
}
