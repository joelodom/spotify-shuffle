//! AI provider backed by the locally installed Claude Code CLI.
//!
//! Invocation (flags verified against Claude Code 2.1.207 and the official
//! headless docs — see README):
//!
//! ```text
//! claude -p --output-format json --tools "" --strict-mcp-config \
//!        --setting-sources user --no-session-persistence \
//!        [--model M] [--append-system-prompt SYS]
//! ```
//!
//! * prompt is piped via stdin (no argv length limits);
//! * `--tools ""` disables all tools — pure text generation;
//! * `--strict-mcp-config` with no `--mcp-config` loads zero MCP servers;
//! * `--setting-sources user` ignores whatever project the app was launched
//!   from; the working directory is additionally an empty scratch dir so no
//!   CLAUDE.md is picked up;
//! * `--no-session-persistence` keeps these one-shot runs out of the user's
//!   session history.
//!
//! Billing: when Claude Code is authenticated via claude.ai OAuth, headless
//! use counts against the subscription with no per-token charge. Because a
//! set `ANTHROPIC_API_KEY` env var SILENTLY switches the CLI to pay-per-token
//! API billing, this provider strips the key (and related overrides) from the
//! subprocess environment — the whole point of this provider is
//! "use my subscription".

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::{AiConfig, claude_workdir};

use super::{AiError, AiProvider, AiRequest};

/// Environment variables that would redirect the CLI away from subscription
/// OAuth (API-key billing, Bedrock/Vertex, alternate endpoints/profiles).
const BILLING_ENV_OVERRIDES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_PROFILE",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    // Nested-invocation markers; harmless but stripped for a clean slate.
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
];

pub struct ClaudeCodeProvider {
    binary: PathBuf,
    model: String,
    timeout: Duration,
    workdir: PathBuf,
}

#[derive(Deserialize, Debug)]
struct ResultEnvelope {
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    total_cost_usd: Option<f64>,
    #[serde(default)]
    api_error_status: Option<u32>,
}

#[derive(Deserialize, Debug, Default)]
struct AuthStatus {
    #[serde(default, rename = "loggedIn")]
    logged_in: bool,
    #[serde(default, rename = "authMethod")]
    auth_method: Option<String>,
    #[serde(default, rename = "subscriptionType")]
    subscription_type: Option<String>,
}

/// Locate the `claude` binary: explicit config path, then PATH, then the
/// standard install locations (GUI-launched apps often have a minimal PATH).
fn resolve_claude_binary(configured: &str) -> Option<PathBuf> {
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        return p.is_file().then_some(p);
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("claude");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let home = dirs::home_dir().unwrap_or_default();
    [
        home.join(".local/bin/claude"),
        home.join(".claude/local/claude"),
        Path::new("/opt/homebrew/bin/claude").to_path_buf(),
        Path::new("/usr/local/bin/claude").to_path_buf(),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

impl ClaudeCodeProvider {
    pub fn from_config(cfg: &AiConfig) -> Result<Self, AiError> {
        let binary = resolve_claude_binary(&cfg.claude_binary).ok_or_else(|| {
            AiError::Config(
                "the `claude` CLI was not found — install Claude Code \
                 (https://claude.com/claude-code), or set ai.claude_binary in config.toml"
                    .into(),
            )
        })?;
        let workdir = claude_workdir();
        std::fs::create_dir_all(&workdir)?;
        Ok(Self {
            binary,
            model: cfg.claude_code_model.clone(),
            timeout: Duration::from_secs(cfg.claude_timeout_secs.max(30)),
            workdir,
        })
    }

    fn base_command(&self) -> Command {
        let mut cmd = Command::new(&self.binary);
        cmd.current_dir(&self.workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for var in BILLING_ENV_OVERRIDES {
            cmd.env_remove(var);
        }
        cmd
    }

    async fn run(
        &self,
        args: &[&str],
        stdin_payload: Option<&str>,
        timeout: Duration,
    ) -> Result<std::process::Output, AiError> {
        let mut cmd = self.base_command();
        cmd.args(args);
        if stdin_payload.is_none() {
            cmd.stdin(Stdio::null());
        }
        let mut child = cmd.spawn()?;
        if let Some(payload) = stdin_payload {
            let mut stdin = child.stdin.take().expect("stdin piped");
            stdin.write_all(payload.as_bytes()).await?;
            stdin.shutdown().await?;
            drop(stdin);
        }
        tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| AiError::Timeout(timeout))?
            .map_err(AiError::from)
    }
}

#[async_trait::async_trait]
impl AiProvider for ClaudeCodeProvider {
    fn describe(&self) -> String {
        let model = if self.model.is_empty() {
            "CLI default model".to_string()
        } else {
            self.model.clone()
        };
        format!(
            "Claude Code (subscription) — {} · {model}",
            self.binary.display()
        )
    }

    async fn complete(&self, req: &AiRequest) -> Result<String, AiError> {
        let mut args: Vec<&str> = vec![
            "-p",
            "--output-format",
            "json",
            "--tools",
            "",
            "--strict-mcp-config",
            "--setting-sources",
            "user",
            "--no-session-persistence",
        ];
        if !self.model.is_empty() {
            args.push("--model");
            args.push(&self.model);
        }
        if !req.system.is_empty() {
            args.push("--append-system-prompt");
            args.push(&req.system);
        }

        let started = std::time::Instant::now();
        let output = self.run(&args, Some(&req.user), self.timeout).await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            // Error envelopes still arrive on stdout with exit code 1.
            if let Ok(envelope) = serde_json::from_str::<ResultEnvelope>(stdout.trim()) {
                return Err(AiError::Provider(envelope_error(&envelope)));
            }
            let tail: String = stderr
                .chars()
                .rev()
                .take(400)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            return Err(AiError::Provider(format!(
                "claude exited with {}: {}",
                output.status,
                if tail.trim().is_empty() {
                    stdout.chars().take(400).collect()
                } else {
                    tail
                }
            )));
        }

        let envelope: ResultEnvelope = serde_json::from_str(stdout.trim()).map_err(|e| {
            AiError::Parse(format!(
                "unexpected CLI output ({e}): {}",
                stdout.chars().take(300).collect::<String>()
            ))
        })?;
        if envelope.is_error {
            return Err(AiError::Provider(envelope_error(&envelope)));
        }
        let result = envelope
            .result
            .ok_or_else(|| AiError::Parse("CLI envelope had no `result` field".into()))?;
        tracing::info!(
            "claude -p finished in {:.1}s (reported cost ${:.4} — informational on \
             subscription billing)",
            started.elapsed().as_secs_f32(),
            envelope.total_cost_usd.unwrap_or(0.0)
        );
        Ok(result)
    }

    async fn health_check(&self) -> Result<String, AiError> {
        let version = self
            .run(&["--version"], None, Duration::from_secs(20))
            .await?;
        if !version.status.success() {
            return Err(AiError::Provider(format!(
                "`claude --version` failed: {}",
                String::from_utf8_lossy(&version.stderr)
            )));
        }
        let version = String::from_utf8_lossy(&version.stdout).trim().to_string();

        let status = self
            .run(&["auth", "status", "--json"], None, Duration::from_secs(20))
            .await?;
        let parsed: AuthStatus =
            serde_json::from_str(String::from_utf8_lossy(&status.stdout).trim())
                .unwrap_or_default();
        if !status.status.success() || !parsed.logged_in {
            return Err(AiError::Provider(format!(
                "Claude Code {version} is installed but not logged in — run `claude` once \
                 in a terminal and use /login"
            )));
        }
        Ok(format!(
            "Claude Code {version} — logged in via {}{}",
            parsed.auth_method.unwrap_or_else(|| "unknown".into()),
            parsed
                .subscription_type
                .map(|s| format!(" ({s} plan)"))
                .unwrap_or_default()
        ))
    }
}

fn envelope_error(envelope: &ResultEnvelope) -> String {
    let mut msg = envelope
        .result
        .clone()
        .filter(|r| !r.trim().is_empty())
        .or_else(|| envelope.subtype.clone())
        .unwrap_or_else(|| "unknown CLI error".into());
    if let Some(status) = envelope.api_error_status {
        msg.push_str(&format!(" (API status {status})"));
        if status == 429 {
            msg.push_str(
                " — this usually means the subscription usage limit was reached; \
                          it resets on a rolling window",
            );
        }
    }
    msg
}
