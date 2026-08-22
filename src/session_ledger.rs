//! Zene session ledger: agent-declared canonical prefix state for B-tier gateway linkage.
//!
//! Protocol counterpart of zene's `docs/agent-inference-context.md` and its
//! `X-Zene-*` outbound headers plus the `POST /v1/zene/sessions/{id}/publish`
//! contract. The agent (zene) is the semantic authority: it owns the canonical
//! transcript, bumps `context_epoch` when the cacheable prefix changes
//! (compaction / system resize), and publishes the new baseline.
//!
//! Cortex stores only routing metadata (never message bodies beyond what is
//! needed for diagnostics): epoch, fingerprint, anchor boundaries, and the
//! last worker that served the session — enabling sticky affinity even while
//! the ZMQ KV-event ledger is cold (gateway restart / engine restart gap).

use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// Sessions idle longer than this are evicted lazily on publish sweeps.
const IDLE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Hard cap to bound memory; publishing evicts stale entries when exceeded.
const MAX_SESSIONS: usize = 100_000;

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub epoch: u64,
    pub prefix_hash: Option<String>,
    /// Message indices marking whole-block edit boundaries declared by the
    /// agent harness (thinking / tool / turn blocks). Consumed by anchored
    /// routing (docs/semantic-anchor-routing.md).
    pub anchor_boundaries: Vec<usize>,
    /// Boundary before which the prefix is pinned stable by the harness.
    pub pinned_boundary: usize,
    pub message_count: usize,
    pub last_worker_id: Option<String>,
    pub last_seen: Instant,
}

impl SessionEntry {
    fn new(epoch: u64) -> Self {
        Self {
            epoch,
            prefix_hash: None,
            anchor_boundaries: Vec::new(),
            pinned_boundary: 0,
            message_count: 0,
            last_worker_id: None,
            last_seen: Instant::now(),
        }
    }
}

#[derive(Default)]
pub struct SessionLedger {
    sessions: DashMap<String, SessionEntry>,
}

/// Payload of zene's publish call (`POST /v1/zene/sessions/{id}/publish`).
#[derive(Debug, Clone, Deserialize)]
pub struct SessionPublishRequest {
    pub epoch: u64,
    #[serde(default)]
    pub message_count: usize,
    #[serde(default)]
    pub pinned_boundary: Option<usize>,
    #[serde(default)]
    pub anchor_boundaries: Option<Vec<usize>>,
    #[serde(default)]
    pub fingerprint: Option<SessionFingerprint>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionFingerprint {
    pub algorithm: String,
    pub value: String,
}

impl SessionLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an agent-published baseline. A newer epoch overwrites the old
    /// state; a stale (older) epoch is rejected to guard against reordered
    /// deliveries.
    pub fn publish(&self, session_id: &str, req: &SessionPublishRequest) -> bool {
        self.evict_stale();

        let mut entry = self.sessions.entry(session_id.to_string()).or_insert_with(|| SessionEntry::new(req.epoch));
        if req.epoch < entry.epoch {
            return false;
        }
        if req.epoch > entry.epoch || entry.message_count == 0 {
            // Fresh baseline for this epoch resets routing hints.
            entry.last_worker_id = None;
        }
        entry.epoch = req.epoch;
        entry.message_count = req.message_count;
        entry.pinned_boundary = req.pinned_boundary.unwrap_or(0);
        entry.anchor_boundaries = req.anchor_boundaries.clone().unwrap_or_default();
        if let Some(fp) = &req.fingerprint {
            entry.prefix_hash = Some(fp.value.clone());
        }
        entry.last_seen = Instant::now();
        true
    }

    /// Removes a session (zene `close_session` / run teardown).
    pub fn close(&self, session_id: &str) -> bool {
        self.sessions.remove(session_id).is_some()
    }

    /// Records which worker served the latest turn of a session.
    pub fn record_assignment(&self, session_id: &str, epoch: u64, worker_id: &str) {
        let mut entry = self
            .sessions
            .entry(session_id.to_string())
            .or_insert_with(|| SessionEntry::new(epoch));
        if entry.epoch != epoch {
            // Epoch drifted without a publish (should not happen); re-anchor.
            entry.epoch = epoch;
            entry.last_worker_id = None;
            entry.anchor_boundaries.clear();
        }
        entry.last_worker_id = Some(worker_id.to_string());
        entry.last_seen = Instant::now();
    }

    /// Returns the sticky worker for a session when the caller's epoch matches
    /// the published baseline exactly.
    pub fn sticky_worker(&self, session_id: &str, epoch: u64) -> Option<String> {
        let entry = self.sessions.get(session_id)?;
        if entry.epoch != epoch {
            return None;
        }
        entry.last_worker_id.clone()
    }

    pub fn get(&self, session_id: &str) -> Option<SessionEntry> {
        self.sessions.get(session_id).map(|e| e.clone())
    }

    pub fn total_sessions(&self) -> usize {
        self.sessions.len()
    }

    fn evict_stale(&self) {
        if self.sessions.len() < MAX_SESSIONS {
            return;
        }
        let stale: Vec<String> = self
            .sessions
            .iter()
            .filter(|e| e.last_seen.elapsed() > IDLE_TTL)
            .map(|e| e.key().clone())
            .collect();
        for id in stale {
            self.sessions.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publish_req(epoch: u64, count: usize) -> SessionPublishRequest {
        SessionPublishRequest {
            epoch,
            message_count: count,
            pinned_boundary: Some(3),
            anchor_boundaries: Some(vec![3, 7, 12]),
            fingerprint: Some(SessionFingerprint {
                algorithm: "zene-v1".into(),
                value: "deadbeef".into(),
            }),
        }
    }

    #[test]
    fn publish_then_sticky_roundtrip() {
        let ledger = SessionLedger::new();
        assert!(ledger.publish("run-1", &publish_req(2, 20)));
        ledger.record_assignment("run-1", 2, "worker-a");
        assert_eq!(ledger.sticky_worker("run-1", 2).as_deref(), Some("worker-a"));
        // Epoch mismatch → no affinity.
        assert_eq!(ledger.sticky_worker("run-1", 3), None);
    }

    #[test]
    fn stale_epoch_publish_rejected_newer_accepted() {
        let ledger = SessionLedger::new();
        assert!(ledger.publish("run-1", &publish_req(5, 30)));
        assert!(!ledger.publish("run-1", &publish_req(4, 30)));
        assert!(ledger.publish("run-1", &publish_req(6, 31)));
        let e = ledger.get("run-1").unwrap();
        assert_eq!(e.epoch, 6);
        assert_eq!(e.message_count, 31);
    }

    #[test]
    fn epoch_bump_resets_affinity_until_new_turn() {
        let ledger = SessionLedger::new();
        ledger.publish("run-1", &publish_req(1, 10));
        ledger.record_assignment("run-1", 1, "worker-a");
        // Compact happened: new baseline published with bumped epoch.
        ledger.publish("run-1", &publish_req(2, 8));
        // Old-turn affinity must not leak across epochs.
        assert_eq!(ledger.sticky_worker("run-1", 2), None);
        ledger.record_assignment("run-1", 2, "worker-b");
        assert_eq!(ledger.sticky_worker("run-1", 2).as_deref(), Some("worker-b"));
    }

    #[test]
    fn close_removes_session() {
        let ledger = SessionLedger::new();
        ledger.publish("run-1", &publish_req(1, 4));
        assert!(ledger.close("run-1"));
        assert!(!ledger.close("run-1"));
        assert!(ledger.get("run-1").is_none());
    }
}
