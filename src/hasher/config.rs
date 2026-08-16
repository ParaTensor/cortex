use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashConfig {
    pub schema_version: u32,
    pub engine: String,
    pub engine_version: String,
    pub model_id: String,
    pub tokenizer_digest: String,
    pub chat_template_digest: String,
    pub page_size: usize,
    pub hash_algorithm: String,
}

impl HashConfig {
    pub fn new(
        engine: impl Into<String>,
        engine_version: impl Into<String>,
        model_id: impl Into<String>,
        tokenizer_digest: impl Into<String>,
        chat_template_digest: impl Into<String>,
        page_size: usize,
        hash_algorithm: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: 1,
            engine: engine.into(),
            engine_version: engine_version.into(),
            model_id: model_id.into(),
            tokenizer_digest: tokenizer_digest.into(),
            chat_template_digest: chat_template_digest.into(),
            page_size,
            hash_algorithm: hash_algorithm.into(),
        }
    }

    /// Computes the immutable config fingerprint (SHA-256)
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.schema_version.to_le_bytes());
        hasher.update(self.engine.as_bytes());
        hasher.update(self.engine_version.as_bytes());
        hasher.update(self.model_id.as_bytes());
        hasher.update(self.tokenizer_digest.as_bytes());
        hasher.update(self.chat_template_digest.as_bytes());
        hasher.update((self.page_size as u64).to_le_bytes());
        hasher.update(self.hash_algorithm.as_bytes());
        hex::encode(hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_config_fingerprint_deterministic() {
        let c1 = HashConfig::new(
            "sglang",
            "v0.4.3",
            "meta-llama/Llama-3.1-8B-Instruct",
            "sha256:abc123",
            "sha256:def456",
            16,
            "sglang_recursive_sha256_v1",
        );
        let c2 = c1.clone();
        assert_eq!(c1.fingerprint(), c2.fingerprint());
    }
}
