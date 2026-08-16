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
}

impl TokenizerEngine {
    /// Loads a tokenizer from a local `tokenizer.json` file.
    pub fn from_file(path: &str, chat_template: Option<String>) -> Result<Self, TokenizerError> {
        let tokenizer = Tokenizer::from_file(path)
            .map_err(|e| TokenizerError::LoadError(e.to_string()))?;
        Ok(Self {
            tokenizer,
            chat_template,
        })
    }

    /// Creates a TokenizerEngine from an in-memory byte buffer.
    pub fn from_bytes(bytes: &[u8], chat_template: Option<String>) -> Result<Self, TokenizerError> {
        let tokenizer = Tokenizer::from_bytes(bytes)
            .map_err(|e| TokenizerError::LoadError(e.to_string()))?;
        Ok(Self {
            tokenizer,
            chat_template,
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
