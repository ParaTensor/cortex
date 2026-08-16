use std::num::NonZeroUsize;
use std::sync::Arc;
use dashmap::DashMap;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::hasher::tokenizer::{ChatMessage, TokenizerEngine};

pub struct TokenizerRegistry {
    tokenizers: DashMap<String, Arc<TokenizerEngine>>,
    /// Bounded LRU cache: (model_id + input_sha256) -> Vec<u32>
    cache: Mutex<lru::LruCache<String, Vec<u32>>>,
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

    /// Computes input cache key: SHA-256(model_id ++ raw_input)
    fn compute_cache_key(model_id: &str, input: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(model_id.as_bytes());
        hasher.update(input.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Tokenizes raw text with LRU caching.
    pub fn tokenize_text(&self, model_id: &str, text: &str) -> Option<Vec<u32>> {
        let cache_key = Self::compute_cache_key(model_id, text);

        // Check LRU cache
        {
            let mut cache = self.cache.lock();
            if let Some(cached) = cache.get(&cache_key) {
                return Some(cached.clone());
            }
        }

        let engine = self.tokenizers.get(model_id)?;
        match engine.encode_text(text) {
            Ok(tokens) => {
                let mut cache = self.cache.lock();
                cache.put(cache_key, tokens.clone());
                Some(tokens)
            }
            Err(e) => {
                warn!(model_id = %model_id, error = %e, "Failed to tokenize text");
                None
            }
        }
    }

    /// Tokenizes chat messages with Jinja2 template and LRU caching.
    pub fn tokenize_chat(&self, model_id: &str, messages: &[ChatMessage]) -> Option<Vec<u32>> {
        let serialized = serde_json::to_string(messages).unwrap_or_default();
        let cache_key = Self::compute_cache_key(model_id, &serialized);

        {
            let mut cache = self.cache.lock();
            if let Some(cached) = cache.get(&cache_key) {
                return Some(cached.clone());
            }
        }

        let engine = self.tokenizers.get(model_id)?;
        match engine.encode_chat(messages, true) {
            Ok(tokens) => {
                let mut cache = self.cache.lock();
                cache.put(cache_key, tokens.clone());
                Some(tokens)
            }
            Err(e) => {
                warn!(model_id = %model_id, error = %e, "Failed to tokenize chat messages");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenizer_registry_cache_key_deterministic() {
        let k1 = TokenizerRegistry::compute_cache_key("llama-3", "Hello world");
        let k2 = TokenizerRegistry::compute_cache_key("llama-3", "Hello world");
        assert_eq!(k1, k2);

        let k3 = TokenizerRegistry::compute_cache_key("qwen-2", "Hello world");
        assert_ne!(k1, k3);
    }
}
