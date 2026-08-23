//! Routing observability counters.
//!
//! Aggregated at gateway level and exposed via `/api/v1/cluster/status`
//! (`routing_stats`). Consumed by agent-side design work — e.g. zene issue
//! #130 correlates prefix-break causes with gateway routing outcomes.

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct RoutingStats {
    pub exact_kv_events: AtomicU64,
    pub session_affinity: AtomicU64,
    pub load_aware: AtomicU64,
    pub fallback_p2c: AtomicU64,
    pub fallback_round_robin: AtomicU64,
    /// Exact hits whose final matched page ends on a semantic anchor.
    pub anchor_aligned_hits: AtomicU64,
    /// Sum of matched pages across exact hits (for average hit size).
    pub exact_matched_pages_total: AtomicU64,
}

impl RoutingStats {
    pub fn record_mode(&self, mode: &str) {
        let counter = match mode {
            "exact_kv_events" => &self.exact_kv_events,
            "session_affinity" => &self.session_affinity,
            "load_aware" => &self.load_aware,
            "fallback_p2c" => &self.fallback_p2c,
            "fallback_round_robin" => &self.fallback_round_robin,
            _ => return,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_exact_hit(&self, matched_pages: usize, anchor_aligned: bool) {
        self.exact_matched_pages_total
            .fetch_add(matched_pages as u64, Ordering::Relaxed);
        if anchor_aligned {
            self.anchor_aligned_hits.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let exact = self.exact_kv_events.load(Ordering::Relaxed);
        let anchor_hits = self.anchor_aligned_hits.load(Ordering::Relaxed);
        let pages_total = self.exact_matched_pages_total.load(Ordering::Relaxed);
        serde_json::json!({
            "exact_kv_events": exact,
            "session_affinity": self.session_affinity.load(Ordering::Relaxed),
            "load_aware": self.load_aware.load(Ordering::Relaxed),
            "fallback_p2c": self.fallback_p2c.load(Ordering::Relaxed),
            "fallback_round_robin": self.fallback_round_robin.load(Ordering::Relaxed),
            "anchor_aligned_hits": anchor_hits,
            "avg_exact_hit_pages": if exact > 0 { pages_total as f64 / exact as f64 } else { 0.0 },
        })
    }
}
