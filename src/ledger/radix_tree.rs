use std::collections::{HashMap, HashSet};
use dashmap::DashMap;

/// KV-Cache locality ledger.
///
/// IMPORTANT INVARIANT: SGLang page hashes are RECURSIVE digests
/// (`h_i = SHA256(h_{i-1} ++ tokens_i)`), so a block hash cryptographically
/// identifies its entire prefix path. A flat `hash -> holders` map therefore
/// preserves exactly the same information as a materialized trie while avoiding
/// two structural defects of the trie form:
///
/// 1. Engine events report one block per `BlockStored` (with a parent field);
///    naive trie insertion from the root would flatten every page into a sibling
///    of depth 1, silently capping LCP matches at a single page.
/// 2. Trie pruning on eviction/clear requires fragile recursive ownership cleanup.

#[derive(Debug, Default)]
pub struct RadixHashTree {
    /// page hash -> set of worker IDs whose engine holds this block in VRAM
    blocks: DashMap<i64, HashSet<String>>,
}

impl RadixHashTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that a worker holds a chain of page hashes.
    pub fn insert_chain(&self, worker_id: &str, page_hashes: &[i64]) {
        for &hash in page_hashes {
            let mut entry = self.blocks.entry(hash).or_default();
            entry.insert(worker_id.to_string());
        }
    }

    /// Removes specific block hashes for a given worker (e.g. upon LRU eviction).
    pub fn remove_chain(&self, worker_id: &str, page_hashes: &[i64]) {
        for &hash in page_hashes {
            if let Some(mut entry) = self.blocks.get_mut(&hash) {
                entry.remove(worker_id);
                if entry.is_empty() {
                    drop(entry);
                    self.blocks.remove_if(&hash, |_, holders| holders.is_empty());
                }
            }
        }
    }

    /// Clears all blocks associated with a given worker.
    pub fn clear_worker(&self, worker_id: &str) {
        let stale: Vec<i64> = self
            .blocks
            .iter()
            .filter(|entry| entry.value().iter().any(|w| w == worker_id))
            .map(|entry| *entry.key())
            .collect();
        for hash in stale {
            if let Some(mut entry) = self.blocks.get_mut(&hash) {
                entry.remove(worker_id);
                if entry.is_empty() {
                    drop(entry);
                    self.blocks.remove_if(&hash, |_, holders| holders.is_empty());
                }
            }
        }
    }

    /// Finds the Longest Common Prefix (LCP) match depth for all eligible live workers.
    /// Returns a map of `worker_id -> matched_pages_count`.
    pub fn find_lcp_matches(
        &self,
        page_hashes: &[i64],
        eligible_workers: &HashSet<String>,
    ) -> HashMap<String, usize> {
        let mut results: HashMap<String, usize> = HashMap::new();
        if page_hashes.is_empty() || eligible_workers.is_empty() {
            return results;
        }

        // Walk the query chain front-to-back. Because hashes are recursive,
        // membership of consecutive prefixes implies a contiguous cached path.
        let mut alive: Vec<&String> = eligible_workers.iter().collect();
        for (depth, &hash) in page_hashes.iter().enumerate() {
            let Some(holders) = self.blocks.get(&hash) else { break };
            alive.retain(|worker| holders.contains(*worker));
            if alive.is_empty() {
                break;
            }
            let d = depth + 1;
            for worker in alive.iter().copied() {
                results.insert(worker.clone(), d);
            }
        }

        results
    }

    /// Returns the total number of cached prefix blocks held by at least one worker.
    pub fn total_cached_blocks(&self) -> usize {
        self.blocks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_radix_tree_insert_remove_and_match() {
        let tree = RadixHashTree::new();
        let hashes = vec![1001, 1002, 1003, 1004];

        tree.insert_chain("worker-1", &hashes[0..3]);
        tree.insert_chain("worker-2", &hashes[0..4]);

        let mut eligible = HashSet::new();
        eligible.insert("worker-1".to_string());
        eligible.insert("worker-2".to_string());

        let matches = tree.find_lcp_matches(&hashes, &eligible);
        assert_eq!(matches.get("worker-1"), Some(&3));
        assert_eq!(matches.get("worker-2"), Some(&4));
        assert_eq!(tree.total_cached_blocks(), 4);

        // Evict the deepest block from worker-2: it now matches only 1..3
        tree.remove_chain("worker-2", &hashes[3..4]);
        let matches_after_remove = tree.find_lcp_matches(&hashes, &eligible);
        assert_eq!(matches_after_remove.get("worker-2"), Some(&3));

        // Clearing worker-1 removes its ownership entirely
        tree.clear_worker("worker-1");
        let matches_after_clear = tree.find_lcp_matches(&hashes, &eligible);
        assert_eq!(matches_after_clear.get("worker-1"), None);
        assert_eq!(matches_after_clear.get("worker-2"), Some(&3));
        // block 1004 was evicted from w2 and now cleared from w1 -> gone
        assert_eq!(tree.total_cached_blocks(), 3);

        tree.clear_worker("worker-2");
        assert_eq!(tree.total_cached_blocks(), 0);
    }

    #[test]
    fn test_lcp_stops_at_first_divergence() {
        let tree = RadixHashTree::new();
        // Shared prefix pages 1..3, divergent page 4
        tree.insert_chain("worker-a", &[11, 22, 33]);
        tree.insert_chain("worker-b", &[11, 22, 99]);

        let mut eligible = HashSet::new();
        eligible.insert("worker-a".to_string());
        eligible.insert("worker-b".to_string());

        let matches = tree.find_lcp_matches(&[11, 22, 33], &eligible);
        assert_eq!(matches.get("worker-a"), Some(&3));
        assert_eq!(matches.get("worker-b"), Some(&2));
    }
}
