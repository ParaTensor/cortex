use std::num::NonZeroUsize;
use std::sync::Arc;
use dashmap::DashMap;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::hasher::sglang::compute_sglang_page_hashes;
use crate::hasher::tokenizer::{ChatMessage, TokenizerEngine};

/// Cached tokenization result containing both token IDs and precomputed recursive block page hashes.
#[derive(Debug, Clone)]
pub struct TokenizationOutput {
    pub token_ids: Arc<Vec<u32>>,
    pub page_hashes: Arc<Vec<i64>>,
}

pub struct TokenizerRegistry {
    tokenizers: DashMap<String, Arc<TokenizerEngine>>,
    /// Bounded zero-allocation LRU cache: [u8; 32] digest -> TokenizationOutput
    cache: Mutex<lru::LruCache<[u8; 32], TokenizationOutput>>,
}

impl TokenizerRegistry {
    pub fn new(cache_capacity: usize) -> Self {
        let cap = NonZeroUsize::new(cache_capacity).unwrap_or(NonZeroUsize::new(10000).unwrap());
        Self {
            tokenizers: DashMap::new(),
            cache: Mutex::new(lru::LruCache::new(cap)),
        }
    }

    pub fn register(&self, model_id: impl Into<String>, engine: TokenizerEngine) {
        self.tokenizers.insert(model_id.into(), Arc::new(engine));
    }

    pub fn contains_model(&self, model_id: &str) -> bool {
        self.tokenizers.contains_key(model_id)
    }

    /// Computes direct 32-byte cache key digest without string heap allocation.
    pub fn compute_text_cache_key(model_id: &str, text: &str, page_size: usize) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(model_id.as_bytes());
        hasher.update(page_size.to_le_bytes());
        hasher.update(text.as_bytes());
        hasher.finalize().into()
    }

    /// Computes direct 32-byte cache key digest for chat messages without JSON serialization.
    pub fn compute_chat_cache_key(model_id: &str, messages: &[ChatMessage], page_size: usize) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(model_id.as_bytes());
        hasher.update(page_size.to_le_bytes());
        for msg in messages {
            hasher.update(msg.role.as_bytes());
            hasher.update([0u8]);
            hasher.update(msg.content.as_bytes());
            hasher.update([1u8]);
        }
        hasher.finalize().into()
    }

    /// Tokenizes raw text and returns precomputed block hashes with zero-copy LRU caching.
    pub fn tokenize_and_hash_text(&self, model_id: &str, text: &str, page_size: usize) -> Option<TokenizationOutput> {
        let cache_key = Self::compute_text_cache_key(model_id, text, page_size);

        // Fast path: L1/L2 LRU Cache Hit (< 1µs)
        {
            let mut cache = self.cache.lock();
            if let Some(cached) = cache.get(&cache_key) {
                return Some(cached.clone());
            }
        }

        // Slow path: Fast Tokenizer + SHA-256 Block Hashes
        let engine = self.tokenizers.get(model_id)?;
        match engine.encode_text(text) {
            Ok(tokens) => {
                let page_hashes = compute_sglang_page_hashes(&tokens, page_size);
                let output = TokenizationOutput {
                    token_ids: Arc::new(tokens),
                    page_hashes: Arc::new(page_hashes),
                };
                let mut cache = self.cache.lock();
                cache.put(cache_key, output.clone());
                Some(output)
            }
            Err(e) => {
                warn!(model_id = %model_id, error = %e, "Failed to tokenize text");
                None
            }
        }
    }

    /// Tokenizes chat messages with Jinja2 template and returns precomputed block hashes with LRU caching.
    pub fn tokenize_and_hash_chat(
        &self,
        model_id: &str,
        messages: &[ChatMessage],
        page_size: usize,
    ) -> Option<TokenizationOutput> {
        let cache_key = Self::compute_chat_cache_key(model_id, messages, page_size);

        // Fast path: L1/L2 LRU Cache Hit (< 1µs)
        {
            let mut cache = self.cache.lock();
            if let Some(cached) = cache.get(&cache_key) {
                return Some(cached.clone());
            }
        }

        // Slow path: Jinja Chat Template + Fast Tokenizer + Recursive Block Hashes
        let engine = self.tokenizers.get(model_id)?;
        match engine.encode_chat(messages, true) {
            Ok(tokens) => {
                let page_hashes = compute_sglang_page_hashes(&tokens, page_size);
                let output = TokenizationOutput {
                    token_ids: Arc::new(tokens),
                    page_hashes: Arc::new(page_hashes),
                };
                let mut cache = self.cache.lock();
                cache.put(cache_key, output.clone());
                Some(output)
            }
            Err(e) => {
                warn!(model_id = %model_id, error = %e, "Failed to tokenize chat messages");
                None
            }
        }
    }

    /// Tokenizes raw text (legacy fallback).
    pub fn tokenize_text(&self, model_id: &str, text: &str) -> Option<Vec<u32>> {
        self.tokenize_and_hash_text(model_id, text, 16)
            .map(|out| (*out.token_ids).clone())
    }

    /// Tokenizes chat messages (legacy fallback).
    pub fn tokenize_chat(&self, model_id: &str, messages: &[ChatMessage]) -> Option<Vec<u32>> {
        self.tokenize_and_hash_chat(model_id, messages, 16)
            .map(|out| (*out.token_ids).clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenizer_registry_cache_key_deterministic() {
        let k1 = TokenizerRegistry::compute_text_cache_key("llama-3", "Hello world", 16);
        let k2 = TokenizerRegistry::compute_text_cache_key("llama-3", "Hello world", 16);
        assert_eq!(k1, k2);

        let k3 = TokenizerRegistry::compute_text_cache_key("qwen-2", "Hello world", 16);
        assert_ne!(k1, k3);

        let chat_msgs = vec![
            ChatMessage { role: "system".into(), content: "You are an assistant".into() },
            ChatMessage { role: "user".into(), content: "Hi".into() },
        ];
        let c1 = TokenizerRegistry::compute_chat_cache_key("qwen", &chat_msgs, 16);
        let c2 = TokenizerRegistry::compute_chat_cache_key("qwen", &chat_msgs, 16);
        assert_eq!(c1, c2);
    }
}
