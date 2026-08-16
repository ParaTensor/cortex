use std::collections::{HashMap, HashSet};
use parking_lot::RwLock;

#[derive(Debug, Default)]
struct TreeNode {
    /// Children indexed by page hash (i64)
    children: HashMap<i64, TreeNode>,
    /// Set of worker IDs holding this prefix node
    workers: HashSet<String>,
}

impl TreeNode {
    fn is_empty(&self) -> bool {
        self.workers.is_empty() && self.children.is_empty()
    }

    fn count_nodes(&self) -> usize {
        let mut count = if !self.workers.is_empty() { 1 } else { 0 };
        for child in self.children.values() {
            count += child.count_nodes();
        }
        count
    }
}

#[derive(Debug, Default)]
pub struct RadixHashTree {
    root: RwLock<TreeNode>,
}

impl RadixHashTree {
    pub fn new() -> Self {
        Self {
            root: RwLock::new(TreeNode::default()),
        }
    }

    /// Records that a worker holds a chain of page hashes
    pub fn insert_chain(&self, worker_id: &str, page_hashes: &[i64]) {
        if page_hashes.is_empty() {
            return;
        }

        let mut root = self.root.write();
        let mut current = &mut *root;
        for &hash in page_hashes {
            let next = current.children.entry(hash).or_default();
            next.workers.insert(worker_id.to_string());
            current = next;
        }
    }

    /// Removes specific block hashes for a given worker (e.g. upon LRU eviction)
    pub fn remove_chain(&self, worker_id: &str, page_hashes: &[i64]) {
        if page_hashes.is_empty() {
            return;
        }

        let mut root = self.root.write();

        fn prune_path(node: &mut TreeNode, worker_id: &str, hashes: &[i64]) {
            if hashes.is_empty() {
                return;
            }

            let first = hashes[0];
            if let Some(child) = node.children.get_mut(&first) {
                if hashes.len() == 1 {
                    child.workers.remove(worker_id);
                } else {
                    prune_path(child, worker_id, &hashes[1..]);
                }

                if child.is_empty() {
                    node.children.remove(&first);
                }
            }
        }

        prune_path(&mut root, worker_id, page_hashes);
    }

    /// Clears all blocks associated with a given worker
    pub fn clear_worker(&self, worker_id: &str) {
        let mut root = self.root.write();
        fn prune(node: &mut TreeNode, worker_id: &str) {
            node.workers.remove(worker_id);
            for child in node.children.values_mut() {
                prune(child, worker_id);
            }
            // Retain only children that still have workers in their subtree
            node.children.retain(|_, child| !child.workers.is_empty() || !child.children.is_empty());
        }
        prune(&mut root, worker_id);
    }

    /// Finds the Longest Common Prefix (LCP) match depth for all eligible live workers.
    /// Returns a map of `worker_id -> matched_pages_count`.
    pub fn find_lcp_matches(&self, page_hashes: &[i64], eligible_workers: &HashSet<String>) -> HashMap<String, usize> {
        let mut results: HashMap<String, usize> = HashMap::new();
        if page_hashes.is_empty() || eligible_workers.is_empty() {
            return results;
        }

        let root = self.root.read();
        let mut current = &*root;
        let mut depth = 0;

        for &hash in page_hashes {
            if let Some(child) = current.children.get(&hash) {
                depth += 1;
                for worker in &child.workers {
                    if eligible_workers.contains(worker) {
                        results.insert(worker.clone(), depth);
                    }
                }
                current = child;
            } else {
                break;
            }
        }

        results
    }

    /// Returns the total number of cached prefix nodes in the tree across all workers.
    pub fn total_cached_blocks(&self) -> usize {
        let root = self.root.read();
        root.count_nodes()
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

        // Test remove_chain on worker-2 for block 4
        tree.remove_chain("worker-2", &hashes[0..4]);
        let matches_after_remove = tree.find_lcp_matches(&hashes, &eligible);
        assert_eq!(matches_after_remove.get("worker-2"), Some(&3)); // still has 1..3

        // Test clear worker 1
        tree.clear_worker("worker-1");
        let matches_after_clear = tree.find_lcp_matches(&hashes, &eligible);
        assert_eq!(matches_after_clear.get("worker-1"), None);
    }
}
