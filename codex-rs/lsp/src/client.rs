//! Diagnostics-only stdio LSP client.
//!
//! Lifecycle per server: spawn → `initialize` → `initialized`
//! → `didOpen` → wait for `publishDiagnostics` → `shutdown` + `exit`.
//! Bounded by timeouts everywhere so a hung server never blocks the agent.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::process::Child;
use tokio::process::Command;
use tracing::instrument;

use crate::config::LspServerConfig;
use crate::diagnostic::Diagnostic;
use crate::diagnostic::DiagnosticSeverity;
use crate::protocol::MessageReader;
use crate::protocol::write_message;

const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_DIAGNOSTICS_TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_DIAGNOSTICS_PER_FILE: usize = 50;

/// Diagnostics-only client for one stdio language server.
pub struct LspClient {
    command: String,
    args: Vec<String>,
    startup_timeout: Duration,
    diagnostics_timeout: Duration,
}

impl LspClient {
    pub fn new(server: &LspServerConfig, diagnostics_timeout: Duration) -> Self {
        Self {
            command: server.command.clone(),
            args: server.args.clone(),
            startup_timeout: server.startup_timeout_ms.unwrap_or(DEFAULT_STARTUP_TIMEOUT),
            diagnostics_timeout,
        }
    }

    pub fn with_timeouts(
        server: &LspServerConfig,
        startup_timeout: Duration,
        diagnostics_timeout: Duration,
    ) -> Self {
        let _ = DEFAULT_STARTUP_TIMEOUT;
        Self {
            command: server.command.clone(),
            args: server.args.clone(),
            startup_timeout: server
                .startup_timeout_ms
                .or(Some(startup_timeout))
                .unwrap_or(startup_timeout),
            diagnostics_timeout,
        }
    }

    pub(crate) fn startup_timeout(&self) -> Duration {
        self.startup_timeout
    }

    /// Open `file_path` in the server and collect its diagnostics.
    #[instrument(skip(self, file_content), fields(command = %self.command))]
    pub async fn diagnostics_for_file(
        &self,
        language_id: &str,
        file_path: &Path,
        file_content: &str,
    ) -> Result<Vec<Diagnostic>> {
        let mut child = self.spawn_server().await?;
        let result = self
            .collect_diagnostics(&mut child, language_id, file_path, file_content)
            .await;
        shutdown_silently(&mut child).await;
        result
    }

    async fn spawn_server(&self) -> Result<Child> {
        Command::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawning LSP server `{}`", self.command))
    }

    async fn collect_diagnostics(
        &self,
        child: &mut Child,
        language_id: &str,
        file_path: &Path,
        file_content: &str,
    ) -> Result<Vec<Diagnostic>> {
        let uri = path_to_uri(file_path);
        let mut stdin = child.stdin.take().context("LSP server stdin")?;
        let stdout = child.stdout.take().context("LSP server stdout")?;
        let mut reader = MessageReader::new(stdout);

        tokio::time::timeout(self.startup_timeout, async {
            write_message(&mut stdin, &initialize_request()).await?;
            // Server `initialize` response (id 0).
            let response = reader
                .next_message()
                .await?
                .context("no initialize response")?;
            assert_initialize_response(&response)?;

            write_message(
                &mut stdin,
                &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
            )
            .await?;
            write_message(
                &mut stdin,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "languageId": language_id,
                            "version": 1,
                            "text": file_content,
                        }
                    }
                }),
            )
            .await?;
            anyhow::Ok(())
        })
        .await
        .context("LSP handshake timed out")??;

        let deadline = tokio::time::Instant::now() + self.diagnostics_timeout;
        let mut diagnostics = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let message = tokio::time::timeout(remaining, reader.next_message())
                .await
                .context("diagnostics wait timed out")??;
            let Some(message) = message else { break };
            if is_publish_diagnostics_for(&message, &uri) {
                diagnostics = parse_diagnostics(&message);
                if !diagnostics.is_empty() {
                    break;
                }
            }
        }
        diagnostics.truncate(MAX_DIAGNOSTICS_PER_FILE);
        Ok(diagnostics)
    }
}

fn initialize_request() -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "processId": std::process::id(),
            "capabilities": {},
            "rootUri": null,
        }
    })
}

fn assert_initialize_response(response: &serde_json::Value) -> Result<()> {
    if response.get("error").is_some() {
        anyhow::bail!("LSP initialize failed: {response}");
    }
    Ok(())
}

fn is_publish_diagnostics_for(message: &serde_json::Value, uri: &str) -> bool {
    message.get("method").and_then(|m| m.as_str()) == Some("textDocument/publishDiagnostics")
        && message.pointer("/params/uri").and_then(|u| u.as_str()) == Some(uri)
}

fn parse_diagnostics(message: &serde_json::Value) -> Vec<Diagnostic> {
    let uri = message
        .pointer("/params/uri")
        .and_then(|u| u.as_str())
        .unwrap_or_default();
    message
        .pointer("/params/diagnostics")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|d| parse_one(uri, d))
        .collect()
}

fn parse_one(uri: &str, value: &serde_json::Value) -> Option<Diagnostic> {
    Some(Diagnostic {
        uri: uri.to_string(),
        line: value
            .pointer("/range/start/line")
            .and_then(|v| v.as_u64())? as u32,
        character: value
            .pointer("/range/start/character")
            .and_then(|v| v.as_u64())? as u32,
        severity: DiagnosticSeverity::from_lsp(
            value.get("severity").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
        ),
        message: value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        code: value.get("code").and_then(|c| {
            c.as_str()
                .map(str::to_string)
                .or_else(|| Some(c.to_string()))
        }),
        source: value
            .get("source")
            .and_then(|s| s.as_str())
            .map(str::to_string),
    })
}

fn path_to_uri(path: &Path) -> String {
    #[cfg(windows)]
    {
        let display = path.display().to_string().replace('\\', "/");
        if display.len() >= 2 && display.chars().nth(1) == Some(':') {
            return format!("file:///{display}");
        }
        format!("file://{display}")
    }
    #[cfg(not(windows))]
    {
        format!("file://{}", path.display())
    }
}

async fn shutdown_silently(child: &mut Child) {
    if let Some(mut stdin) = child.stdin.take() {
        let _ = write_message(
            &mut stdin,
            &serde_json::json!({"jsonrpc": "2.0", "id": 9999, "method": "shutdown", "params": {}}),
        )
        .await;
        let _ = write_message(
            &mut stdin,
            &serde_json::json!({"jsonrpc": "2.0", "method": "exit", "params": {}}),
        )
        .await;
        let _ = stdin.shutdown().await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    let _ = child.kill().await;
}

/// Default diagnostics wait when no config override is set.
pub fn default_diagnostics_timeout() -> Duration {
    DEFAULT_DIAGNOSTICS_TIMEOUT
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
