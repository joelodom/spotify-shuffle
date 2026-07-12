//! AI provider that calls the Anthropic Messages API directly (raw HTTP —
//! there is no official Rust SDK). Pay-per-token; requires an API key in the
//! environment variable named by `ai.anthropic_api_key_env` (default
//! `ANTHROPIC_API_KEY`). The key is never written to disk by this app.
//!
//! Request shape follows the current API guidance (July 2026):
//! * `POST https://api.anthropic.com/v1/messages`, version `2023-06-01`;
//! * default model `claude-opus-4-8`;
//! * no `temperature`/`top_p`/`top_k` — removed on Opus 4.7+ (400 if sent);
//! * the `thinking` parameter is OMITTED entirely: that is valid on every
//!   current model (always-on for Fable 5, adaptive-by-default for Sonnet 5,
//!   off for Opus 4.8, unsupported on older Haiku) — the robust choice when
//!   the model id is user-configurable free text;
//! * `max_tokens` 16000 non-streaming, per current SDK-timeout guidance.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::config::AiConfig;

use super::{AiError, AiProvider, AiRequest};

const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 16_000;

pub struct AnthropicApiProvider {
    http: reqwest::Client,
    model: String,
    api_key: String,
    key_env: String,
}

#[derive(Deserialize)]
struct ApiMessage {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type", default)]
    block_type: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct ApiErrorEnvelope {
    error: ApiErrorBody,
}
#[derive(Deserialize)]
struct ApiErrorBody {
    #[serde(rename = "type", default)]
    error_type: String,
    #[serde(default)]
    message: String,
}

impl AnthropicApiProvider {
    pub fn from_config(cfg: &AiConfig) -> Result<Self, AiError> {
        let key_env = cfg.anthropic_api_key_env.clone();
        let api_key = std::env::var(&key_env).map_err(|_| {
            AiError::Config(format!(
                "the Anthropic API provider needs an API key in ${key_env} \
                 (or switch ai.provider back to \"claude-code\")"
            ))
        })?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .expect("reqwest client");
        Ok(Self {
            http,
            model: if cfg.anthropic_model.is_empty() {
                "claude-opus-4-8".to_string()
            } else {
                cfg.anthropic_model.clone()
            },
            api_key,
            key_env,
        })
    }
}

#[async_trait::async_trait]
impl AiProvider for AnthropicApiProvider {
    fn describe(&self) -> String {
        format!(
            "Anthropic API (pay per token) — {} · key from ${}",
            self.model, self.key_env
        )
    }

    async fn complete(&self, req: &AiRequest) -> Result<String, AiError> {
        let body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": req.system,
            "messages": [{ "role": "user", "content": req.user }],
        });

        let mut attempt = 0u8;
        let resp = loop {
            attempt += 1;
            let resp = self
                .http
                .post(MESSAGES_URL)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .json(&body)
                .send()
                .await?;
            let status = resp.status().as_u16();
            if (status == 429 || status >= 500) && attempt < 3 {
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(2u64.pow(attempt as u32));
                tracing::warn!("Anthropic API {status}; retrying in {wait}s");
                tokio::time::sleep(Duration::from_secs(wait.min(60))).await;
                continue;
            }
            break resp;
        };

        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            let msg = serde_json::from_str::<ApiErrorEnvelope>(&text)
                .map(|e| format!("{}: {}", e.error.error_type, e.error.message))
                .unwrap_or_else(|_| {
                    format!(
                        "HTTP {status}: {}",
                        text.chars().take(300).collect::<String>()
                    )
                });
            return Err(AiError::Provider(msg));
        }

        let message: ApiMessage = serde_json::from_str(&text)
            .map_err(|e| AiError::Parse(format!("unexpected Messages API response: {e}")))?;

        match message.stop_reason.as_deref() {
            Some("refusal") => {
                return Err(AiError::Provider(
                    "the model declined this request (stop_reason: refusal)".into(),
                ));
            }
            Some("max_tokens") => {
                return Err(AiError::Provider(
                    "the response was truncated at the token limit; try a smaller request \
                     (fewer tracks)"
                        .into(),
                ));
            }
            _ => {}
        }

        let combined: String = message
            .content
            .iter()
            .filter(|b| b.block_type == "text")
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if combined.trim().is_empty() {
            return Err(AiError::Parse("the API response contained no text".into()));
        }
        Ok(combined)
    }

    async fn health_check(&self) -> Result<String, AiError> {
        // Passive check only — a real generation costs money; the Settings
        // "Test AI" button exercises complete() explicitly.
        Ok(format!(
            "Anthropic API configured: model {}, key present in ${} ({} chars)",
            self.model,
            self.key_env,
            self.api_key.len()
        ))
    }
}
