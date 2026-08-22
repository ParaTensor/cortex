use std::sync::Arc;
use std::time::Duration;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use tracing::{error, info, warn};
use zeromq::{Socket, SocketRecv, SubSocket};

use crate::ledger::{RadixHashTree, WorkerRuntimeState, WorkerSyncStatus};

#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Parses raw SGLang ZMQ MessagePack payload (or fallback JSON)
    ///
    /// SGLang wire format:
    /// - Frame 0: `topic_bytes` (e.g. `b""` or `b"kv_events"`)
    /// - Frame 1: `seq_bytes` (u64 in 8-byte big-endian: `seq.to_bytes(8, "big")`)
    /// - Frame 2: `payload_bytes` (msgspec msgpack encoded array: `[ts, [[EventName, ...]], attn_dp_rank]`)
    pub fn parse_sglang_multipart(seq_bytes: &[u8], payload_bytes: &[u8]) -> Vec<KvEventMessage> {
        let seq = if seq_bytes.len() == 8 {
            u64::from_be_bytes(seq_bytes.try_into().unwrap_or([0; 8]))
        } else {
            0
        };

        let mut results = Vec::new();

        // 1. Try decoding SGLang native msgspec msgpack structure
        if let Ok(rmp_val) = rmp_serde::from_slice::<rmpv::Value>(payload_bytes) {
            if let rmpv::Value::Array(batch) = rmp_val {
                if batch.len() >= 2 {
                    if let rmpv::Value::Array(events_list) = &batch[1] {
                        for ev in events_list {
                            if let rmpv::Value::Array(ev_fields) = ev {
                                if let Some(rmpv::Value::String(ev_name)) = ev_fields.first() {
                                    let name_str = ev_name.as_str().unwrap_or("");
                                    if name_str == "BlockStored" {
                                        if let Some(rmpv::Value::Array(hash_arr)) = ev_fields.get(1) {
                                            let mut hashes = Vec::with_capacity(hash_arr.len());
                                            for h in hash_arr {
                                                if let Some(val) = h.as_i64() {
                                                    hashes.push(val);
                                                } else if let Some(val) = h.as_u64() {
                                                    hashes.push(val as i64);
                                                }
                                            }
                                            results.push(KvEventMessage {
                                                seq,
                                                payload: KvEventPayload::BlockStored { page_hashes: hashes },
                                            });
                                        }
                                    } else if name_str == "BlockRemoved" {
                                        if let Some(rmpv::Value::Array(hash_arr)) = ev_fields.get(1) {
                                            let mut hashes = Vec::with_capacity(hash_arr.len());
                                            for h in hash_arr {
                                                if let Some(val) = h.as_i64() {
                                                    hashes.push(val);
                                                } else if let Some(val) = h.as_u64() {
                                                    hashes.push(val as i64);
                                                }
                                            }
                                            results.push(KvEventMessage {
                                                seq,
                                                payload: KvEventPayload::BlockRemoved { page_hashes: hashes },
                                            });
                                        }
                                    } else if name_str == "AllBlocksCleared" {
                                        results.push(KvEventMessage {
                                            seq,
                                            payload: KvEventPayload::AllBlocksCleared,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Fallback JSON decoder
        if results.is_empty() {
            #[derive(Deserialize)]
            struct JsonEventWrapper {
                #[serde(default)]
                seq: u64,
                #[serde(rename = "type")]
                event_type: String,
                #[serde(default)]
                page_hashes: Vec<i64>,
            }

            if let Ok(json_ev) = serde_json::from_slice::<JsonEventWrapper>(payload_bytes) {
                let actual_seq = if seq > 0 { seq } else { json_ev.seq };
                let payload = match json_ev.event_type.as_str() {
                    "block_stored" | "BlockStored" => KvEventPayload::BlockStored { page_hashes: json_ev.page_hashes },
                    "block_removed" | "BlockRemoved" => KvEventPayload::BlockRemoved { page_hashes: json_ev.page_hashes },
                    "all_blocks_cleared" | "AllBlocksCleared" => KvEventPayload::AllBlocksCleared,
                    _ => KvEventPayload::AllBlocksCleared,
                };
                results.push(KvEventMessage { seq: actual_seq, payload });
            }
        }

        results
    }

    /// Processes an incoming KV event for a worker, validating strict sequence monotonicity
    /// and enforcing the ledger synchronization gatekeeper.
    pub fn process_event(&self, worker: &WorkerRuntimeState, event: KvEventMessage) {
        let mut last_seq_guard = worker.last_seq.write();
        let mut current_status = *worker.status.read();
        let expected_seq = *last_seq_guard + 1;

        // 1a. Engine restart detection: seq regressed below our watermark.
        // The worker process was recycled (crash / redeploy) and its Radix tree
        // starts from scratch, so previously synced blocks are phantom entries.
        // Full resync: drop the per-worker subtree and adopt the new stream.
        if *last_seq_guard > 0 && event.seq < *last_seq_guard {
            warn!(
                worker_id = %worker.config.id,
                previous_seq = *last_seq_guard,
                actual_seq = event.seq,
                "KV event sequence regression detected (engine restart?). Resetting worker ledger and resyncing."
            );
            self.tree.clear_worker(&worker.config.id);
            *last_seq_guard = event.seq;
            worker.set_status(WorkerSyncStatus::Syncing);
            current_status = WorkerSyncStatus::Syncing;
        } else if *last_seq_guard > 0 && event.seq != *last_seq_guard && event.seq != expected_seq {
            // 1b. True forward gap: incremental events were lost on the wire.
            // The ledger is provably incomplete — prune it and gate further
            // incremental writes until an explicit resync signal arrives.
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

                    // Watchdog: PUB/SUB offers no liveness signal, and zeromq-rs
                    // recv() may hang forever on a silently-dead peer (e.g. the
                    // worker container was recycled). Recycle the subscription
                    // when the stream stays silent, forcing a fresh TCP connect;
                    // ledger integrity across restarts is arbitrated by seq.
                    loop {
                        let recv = tokio::time::timeout(Duration::from_secs(30), socket.recv()).await;
                        let msg = match recv {
                            Ok(Ok(msg)) => msg,
                            Ok(Err(e)) => {
                                warn!(worker_id = %worker_id, error = %e, "ZMQ receive failed; reconnecting.");
                                break;
                            }
                            Err(_) => {
                                warn!(worker_id = %worker_id, idle_secs = 30, "ZMQ stream silent for 30s; recycling subscription to detect dead peer.");
                                break;
                            }
                        };

                        // SGLang sends multipart: [topic_bytes, seq_bytes, payload_bytes]
                        let frames: Vec<&[u8]> = msg.iter().map(|b| b.as_ref()).collect();

                        let events = if frames.len() >= 3 {
                            KvEventProcessor::parse_sglang_multipart(frames[1], frames[2])
                        } else if frames.len() == 2 {
                            KvEventProcessor::parse_sglang_multipart(&[], frames[1])
                        } else if let Some(&single) = frames.first() {
                            KvEventProcessor::parse_sglang_multipart(&[], single)
                        } else {
                            Vec::new()
                        };

                        worker.update_heartbeat();

                        for ev in events {
                            processor.process_event(&worker, ev);
                        }
                    }

                    warn!(worker_id = %worker_id, "ZMQ connection interrupted. Entering reconnect backoff; ledger consistency will be re-arbitrated by sequence numbers.");
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
    fn test_sglang_native_msgpack_multipart_decoding() {
        // Hex produced by SGLang msgspec msgpack:
        // [123456.78, [["BlockStored", [123, 456], None, [1, 2, 3], 16, None, None]], 0]
        let payload_hex = "93cb40fe240c7ae147ae9197ab426c6f636b53746f726564927bcd01c8c09301020310c0c000";
        let payload_bytes = hex::decode(payload_hex).unwrap();
        let seq_bytes: [u8; 8] = 42u64.to_be_bytes();

        let events = KvEventProcessor::parse_sglang_multipart(&seq_bytes, &payload_bytes);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 42);
        match &events[0].payload {
            KvEventPayload::BlockStored { page_hashes } => {
                assert_eq!(page_hashes, &vec![123, 456]);
            }
            _ => panic!("Expected BlockStored event"),
        }
    }
}
