use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
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
