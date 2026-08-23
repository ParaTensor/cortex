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
    /// Whether the message carries tool calls. Drives harness-declared anchor
    /// detection (zene rule: tool-call groups start at such messages).
    #[serde(default)]
    pub has_tool_calls: bool,
}

/// Chat encoding with the offsets needed to map text markers back to tokens.
pub struct ChatEncodeOutput {
    pub token_ids: Vec<u32>,
    /// Char span of each token within the rendered string.
    pub offsets: Vec<(usize, usize)>,
    pub rendered: String,
}

pub struct TokenizerEngine {
    tokenizer: Tokenizer,
    chat_template: Option<String>,
    /// Vocab ids of structural special tokens whose positions mark semantic
    /// block boundaries (turn ends, thinking ends, tool markers). Derived at
    /// load time by scanning the vocab; see [`anchor_token_ids_from_vocab`].
    anchor_token_ids: HashSet<u32>,
}

/// Built-in structural marker patterns used to seed the anchor set.
/// Conservative on purpose: false positives dilute the survival signal,
/// false negatives merely fall back to sigma_plain.
const ANCHOR_TOKEN_PATTERNS: &[&str] = &[
    "<|im_end|>",
    "</think>",
    "<|eot",
    "eot_id",
    "<tool_",
    "</tool",
];

/// ChatML family role marker used to locate message boundaries in the
/// rendered string. Templates that do not emit it fall back to special-token
/// anchors only.
const CHATML_MESSAGE_MARKER: &str = "<|im_start|>";

/// Boundary tokens this far past a page end still count the page as anchored
/// (covers trailing whitespace / newline tokens after the boundary marker).
const ANCHOR_PAGE_SLACK: usize = 3;

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

/// Harness-declared semantic anchors (zene rule): user messages start turns;
/// assistant messages carrying tool calls start tool-call groups. Index 0
/// (the system message) is never an anchor — it is the frozen prefix itself.
pub fn harness_anchor_indices(messages: &[ChatMessage]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, m)| m.role == "user" || (m.role == "assistant" && m.has_tool_calls))
        .map(|(i, _)| i)
        .collect()
}

/// Maps harness anchor message indices to page-anchor flags using the
/// rendered text and token offsets. Returns `None` when the rendered text is
/// not a recognizable ChatML stream (caller falls back to special-token
/// anchors only).
///
/// A page is anchored when a harness block starts within `ANCHOR_PAGE_SLACK`
/// tokens after the page end — i.e. a match ending at that page boundary
/// ends on a semantic block boundary.
fn harness_anchor_pages(
    messages: &[ChatMessage],
    rendered: &str,
    offsets: &[(usize, usize)],
    page_size: usize,
    num_pages: usize,
) -> Option<Vec<bool>> {
    let marker_positions: Vec<usize> = rendered
        .match_indices(CHATML_MESSAGE_MARKER)
        .map(|(pos, _)| pos)
        .collect();
    // Require at least one marker per message; templates with extra markers
    // (none known) would still line up because markers precede messages in
    // document order and the system message is marker 0.
    if marker_positions.len() < messages.len() {
        return None;
    }

    let mut flags = vec![false; num_pages];
    for msg_idx in harness_anchor_indices(messages) {
        let char_pos = marker_positions[msg_idx];
        // First token starting at (or just after) the marker position.
        let Some(tok_idx) = offsets.iter().position(|(start, _)| *start >= char_pos) else {
            continue;
        };
        let rem = tok_idx % page_size;
        if rem <= ANCHOR_PAGE_SLACK && tok_idx >= page_size {
            let page = (tok_idx - rem) / page_size - 1;
            if page < num_pages {
                flags[page] = true;
            }
        }
    }
    Some(flags)
}

impl TokenizerEngine {
    /// Loads a tokenizer from a local `tokenizer.json` file.
    pub fn from_file(path: &str, chat_template: Option<String>) -> Result<Self, TokenizerError> {
        let tokenizer =
            Tokenizer::from_file(path).map_err(|e| TokenizerError::LoadError(e.to_string()))?;
        let anchor_token_ids = anchor_token_ids_from_vocab(&tokenizer.get_vocab(true));
        Ok(Self {
            tokenizer,
            chat_template,
            anchor_token_ids,
        })
    }

    /// Creates a TokenizerEngine from an in-memory byte buffer.
    pub fn from_bytes(bytes: &[u8], chat_template: Option<String>) -> Result<Self, TokenizerError> {
        let tokenizer =
            Tokenizer::from_bytes(bytes).map_err(|e| TokenizerError::LoadError(e.to_string()))?;
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

    /// Anchor flags combining tokenizer-inferred structural tokens with
    /// harness-declared message boundaries (turn starts / tool-call groups).
    pub fn page_is_anchor_chat(
        &self,
        messages: &[ChatMessage],
        encoded: &ChatEncodeOutput,
        page_size: usize,
    ) -> Vec<bool> {
        let mut flags = self.page_is_anchor(&encoded.token_ids, page_size);
        if let Some(harness) = harness_anchor_pages(
            messages,
            &encoded.rendered,
            &encoded.offsets,
            page_size,
            flags.len(),
        ) {
            for (dst, src) in flags.iter_mut().zip(harness) {
                *dst |= src;
            }
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
        self.encode_chat_with_tools(messages, None, add_generation_prompt)
    }

    /// Renders + tokenizes with an optional `tools` definition.
    ///
    /// Alignment contract (docs/tokenizer-hash-alignment.md): engines render
    /// the chat template with the request's tool schema included, so the
    /// gateway MUST hash the same rendered stream. Coding agents (zene) always
    /// attach tools; omitting them here would desynchronize page hashes from
    /// the very first block and permanently disable exact KV matching.
    pub fn encode_chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: Option<&serde_json::Value>,
        add_generation_prompt: bool,
    ) -> Result<Vec<u32>, TokenizerError> {
        Ok(self
            .encode_chat_full(messages, tools, add_generation_prompt)?
            .token_ids)
    }

    /// Renders the chat template and tokenizes, returning tokens + offsets.
    pub fn encode_chat_full(
        &self,
        messages: &[ChatMessage],
        tools: Option<&serde_json::Value>,
        add_generation_prompt: bool,
    ) -> Result<ChatEncodeOutput, TokenizerError> {
        let rendered = self.render_chat(messages, tools, add_generation_prompt)?;
        let encoding = self
            .tokenizer
            .encode(rendered.as_str(), true)
            .map_err(|e| TokenizerError::EncodeError(e.to_string()))?;
        Ok(ChatEncodeOutput {
            token_ids: encoding.get_ids().to_vec(),
            offsets: encoding.get_offsets().to_vec(),
            rendered,
        })
    }

    fn render_chat(
        &self,
        messages: &[ChatMessage],
        tools: Option<&serde_json::Value>,
        add_generation_prompt: bool,
    ) -> Result<String, TokenizerError> {
        if let Some(template_str) = &self.chat_template {
            let mut env = minijinja::Environment::new();
            env.add_template("chat_template", template_str)
                .map_err(|e| TokenizerError::TemplateError(e.to_string()))?;

            let template = env
                .get_template("chat_template")
                .map_err(|e| TokenizerError::TemplateError(e.to_string()))?;

            let tools_value = match tools {
                Some(t) => minijinja::Value::from_serialize(t),
                None => minijinja::Value::UNDEFINED,
            };
            let context = minijinja::context! {
                messages => messages,
                add_generation_prompt => add_generation_prompt,
                tools => tools_value,
            };

            template
                .render(context)
                .map_err(|e| TokenizerError::TemplateError(e.to_string()))
        } else {
            // Fallback: Concatenate messages if no template is provided
            let mut combined = String::new();
            for msg in messages {
                combined.push_str(&format!("{}: {}\n", msg.role, msg.content));
            }
            if add_generation_prompt {
                combined.push_str("assistant:\n");
            }
            Ok(combined)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, has_tool_calls: bool) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: "x".to_string(),
            has_tool_calls,
        }
    }

    #[test]
    fn harness_anchor_indices_follow_zene_rule() {
        let messages = vec![
            msg("system", false),
            msg("user", false),     // turn start
            msg("assistant", true), // tool-call group start
            msg("tool", false),
            msg("assistant", false), // plain reply: not an anchor
            msg("user", false),      // turn start
        ];
        assert_eq!(harness_anchor_indices(&messages), vec![1, 2, 5]);
    }

    #[test]
    fn harness_anchor_pages_align_boundaries_to_pages() {
        // ChatML: one marker per message.
        let messages = vec![msg("system", false), msg("user", false), msg("user", false)];
        let rendered = "<|im_start|>system\nx<|im_end|>\n<|im_start|>user\nx<|im_end|>\n<|im_start|>user\nx<|im_end|>\n";
        // Fake offsets: token t occupies [t*4, t*4+4); marker for message 1 at
        // char 28 => token 7; marker for message 2 at char 56 => token 14.
        let offsets: Vec<(usize, usize)> = (0..20).map(|t| (t * 4, t * 4 + 4)).collect();
        let flags =
            harness_anchor_pages(&messages, rendered, &offsets, 16, 2).expect("chatml stream");
        // token 7 -> 7 % 16 = 7 > slack => page 0 not anchored by message 1.
        // token 14 -> 14 % 16 = 14 > slack => nothing anchored.
        assert_eq!(flags, vec![false, false]);

        // Now boundary exactly at page end: message 2 marker at char 64 => token 16.
        let rendered2 = format!(
            "<|im_start|>system\nx<|im_end|>\n<|im_start|>user\nx<|im_end|>\n{}<|im_start|>user\nx",
            " ".repeat(
                64 - "<|im_start|>system\nx<|im_end|>\n<|im_start|>user\nx<|im_end|>\n".len()
            )
        );
        let offsets2: Vec<(usize, usize)> = (0..20).map(|t| (t * 4, t * 4 + 4)).collect();
        let flags2 =
            harness_anchor_pages(&messages, &rendered2, &offsets2, 16, 2).expect("chatml stream");
        assert_eq!(flags2, vec![true, false]);
    }

    #[test]
    fn harness_anchor_pages_rejects_non_chatml() {
        let messages = vec![msg("system", false), msg("user", false)];
        let offsets: Vec<(usize, usize)> = vec![(0, 4)];
        assert!(harness_anchor_pages(&messages, "no markers here", &offsets, 16, 1).is_none());
    }
}
