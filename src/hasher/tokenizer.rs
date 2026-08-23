use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokenizers::Tokenizer;

#[derive(Error, Debug)]
pub enum TokenizerError {
    #[error("Failed to load tokenizer: {0}")]
    LoadError(String),
    #[error("Tokenization failed: {0}")]
    EncodeError(String),
    #[error("Chat template rendering failed: {0}")]
    TemplateError(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

pub struct TokenizerEngine {
    tokenizer: Tokenizer,
    chat_template: Option<String>,
    /// Vocab ids of structural special tokens whose positions mark semantic
    /// block boundaries (turn ends, thinking ends, tool markers). Derived at
    /// load time by scanning the vocab; see [`anchor_token_ids`].
    anchor_token_ids: HashSet<u32>,
}

/// Built-in structural marker patterns used to seed the anchor set.
/// Conservative on purpose: false positives dilute the survival signal,
/// false negatives merely fall back to sigma_plain.
const ANCHOR_TOKEN_PATTERNS: &[&str] = &[
    "<|im_end|>", "</think>", "<|eot", "eot_id", "<tool_", "</tool",
];

fn anchor_token_ids_from_vocab(vocab: &std::collections::HashMap<String, u32>) -> HashSet<u32> {
    vocab
        .iter()
        .filter(|(token, _)| {
            let lower = token.to_lowercase();
            ANCHOR_TOKEN_PATTERNS
                .iter()
                .any(|pattern| lower.contains(&pattern.to_lowercase()))
        })
        .map(|(_, id)| *id)
        .collect()
}

impl TokenizerEngine {
    /// Loads a tokenizer from a local `tokenizer.json` file.
    pub fn from_file(path: &str, chat_template: Option<String>) -> Result<Self, TokenizerError> {
        let tokenizer = Tokenizer::from_file(path)
            .map_err(|e| TokenizerError::LoadError(e.to_string()))?;
        let anchor_token_ids = anchor_token_ids_from_vocab(&tokenizer.get_vocab(true));
        Ok(Self {
            tokenizer,
            chat_template,
            anchor_token_ids,
        })
    }

    /// Creates a TokenizerEngine from an in-memory byte buffer.
    pub fn from_bytes(bytes: &[u8], chat_template: Option<String>) -> Result<Self, TokenizerError> {
        let tokenizer = Tokenizer::from_bytes(bytes)
            .map_err(|e| TokenizerError::LoadError(e.to_string()))?;
        let anchor_token_ids = anchor_token_ids_from_vocab(&tokenizer.get_vocab(true));
        Ok(Self {
            tokenizer,
            chat_template,
            anchor_token_ids,
        })
    }

    /// Tokenizes raw text into token IDs.
    pub fn encode_text(&self, text: &str) -> Result<Vec<u32>, TokenizerError> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| TokenizerError::EncodeError(e.to_string()))?;
        Ok(encoding.get_ids().to_vec())
    }

    /// Computes per-page anchor flags for a token stream (docs/semantic-anchor-routing.md).
    ///
    /// Page i is anchored when the stream passes a structural boundary token at
    /// its very end, or immediately after it (trailing newline after `<|im_end|>`
    /// etc.). Anchored page boundaries are the positions where agentic context
    /// edits are likely to cut, so matches ending there survive later turns.
    pub fn page_is_anchor(&self, token_ids: &[u32], page_size: usize) -> Vec<bool> {
        let n = token_ids.len() / page_size;
        let mut flags = Vec::with_capacity(n);
        for i in 0..n {
            let end = (i + 1) * page_size;
            let last_here = token_ids[end - 1];
            let just_after = token_ids.get(end).copied();
            flags.push(
                self.anchor_token_ids.contains(&last_here)
                    || just_after.is_some_and(|t| self.anchor_token_ids.contains(&t)),
            );
        }
        flags
    }

    /// Number of detected anchor token ids (diagnostics).
    pub fn anchor_vocab_size(&self) -> usize {
        self.anchor_token_ids.len()
    }

    /// Renders chat messages with Jinja2 template and tokenizes the rendered string.
    pub fn encode_chat(
        &self,
        messages: &[ChatMessage],
        add_generation_prompt: bool,
    ) -> Result<Vec<u32>, TokenizerError> {
        if let Some(template_str) = &self.chat_template {
            let mut env = minijinja::Environment::new();
            env.add_template("chat_template", template_str)
                .map_err(|e| TokenizerError::TemplateError(e.to_string()))?;

            let template = env
                .get_template("chat_template")
                .map_err(|e| TokenizerError::TemplateError(e.to_string()))?;

            let context = minijinja::context! {
                messages => messages,
                add_generation_prompt => add_generation_prompt,
            };

            let rendered = template
                .render(context)
                .map_err(|e| TokenizerError::TemplateError(e.to_string()))?;

            self.encode_text(&rendered)
        } else {
            // Fallback: Concatenate messages if no template is provided
            let mut combined = String::new();
            for msg in messages {
                combined.push_str(&format!("{}: {}\n", msg.role, msg.content));
            }
            if add_generation_prompt {
                combined.push_str("assistant:\n");
            }
            self.encode_text(&combined)
        }
    }
}
