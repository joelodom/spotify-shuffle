//! The swappable AI layer.
//!
//! Everything AI-related goes through the [`AiProvider`] trait, so the
//! backing engine is a config choice:
//!
//! * [`claude_code::ClaudeCodeProvider`] (default) drives the locally
//!   installed, already-authenticated Claude Code CLI in headless print mode
//!   — usage counts against the user's Claude subscription, no API key.
//! * [`anthropic::AnthropicApiProvider`] calls the Anthropic Messages API
//!   directly (pay-per-token) — switch by setting `ai.provider =
//!   "anthropic-api"` in config.toml and exporting an API key.

pub mod anthropic;
pub mod claude_code;
pub mod prompts;

use std::time::Duration;

use crate::config::{AiConfig, AiProviderKind};

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("AI provider not configured: {0}")]
    Config(String),
    #[error("AI provider error: {0}")]
    Provider(String),
    #[error("AI provider timed out after {0:?}")]
    Timeout(Duration),
    #[error("could not parse the AI response: {0}")]
    Parse(String),
    #[error("network error talking to the AI provider: {0}")]
    Network(#[from] reqwest::Error),
    #[error("i/o error launching the AI provider: {0}")]
    Io(#[from] std::io::Error),
}

/// One-shot generation request. `system` sets role/format rules; `user`
/// carries the task payload.
#[derive(Clone, Debug)]
pub struct AiRequest {
    pub system: String,
    pub user: String,
}

#[async_trait::async_trait]
pub trait AiProvider: Send + Sync {
    /// Human-readable description shown in Settings/logs.
    fn describe(&self) -> String;

    /// Run one generation and return the model's text output.
    async fn complete(&self, req: &AiRequest) -> Result<String, AiError>;

    /// Cheap, no-generation check that the provider is usable
    /// (binary/auth/key present). Returns a status line.
    async fn health_check(&self) -> Result<String, AiError>;
}

/// Build the provider selected in config.
pub fn build_provider(cfg: &AiConfig) -> Result<Box<dyn AiProvider>, AiError> {
    match cfg.provider {
        AiProviderKind::ClaudeCode => {
            Ok(Box::new(claude_code::ClaudeCodeProvider::from_config(cfg)?))
        }
        AiProviderKind::AnthropicApi => {
            Ok(Box::new(anthropic::AnthropicApiProvider::from_config(cfg)?))
        }
    }
}
