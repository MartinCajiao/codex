//! Diagnostics-only stdio LSP client.
//!
//! One client owns one long-lived language-server process. Documents are opened
//! once and updated on later calls, preserving the server's incremental state.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use codex_utils_path_uri::PathUri;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::process::Child;
use tokio::process::Command;
use tracing::debug;
use tracing::instrument;

use crate::config::LspServerConfig;
use crate::diagnostic::Diagnostic;
use crate::diagnostic::DiagnosticSeverity;
use crate::protocol::MessageReader;
use crate::protocol::write_message;

const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DIAGNOSTICS_PER_FILE: usize = 50;

/// Diagnostics-only client for one persistent stdio language server.
pub struct LspClient {
    child: Child,
    session: LspSession<tokio::process::ChildStdout, tokio::process::ChildStdin>,
    workspace_root: PathBuf,
}

impl LspClient {
    /// Spawn and initialize a language server for `workspace_root`.
    pub async fn start(
        server: &LspServerConfig,
        workspace_root: impl AsRef<Path>,
        diagnostics_timeout: Duration,
    ) -> Result<Self> {
        let workspace_root = absolute_path(workspace_root.as_ref())?;
        let root_uri = PathUri::from_host_native_path(&workspace_root)
            .context("converting LSP workspace root to a file URI")?
            .to_string();

        let mut child = Command::new(&server.command)
            .args(&server.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning LSP server `{}`", server.command))?;

        let stdin = child.stdin.take().context("LSP server stdin")?;
        let stdout = child.stdout.take().context("LSP server stdout")?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(drain_stderr(stderr, server.command.clone()));
        }

        let mut session = LspSession::new(stdout, stdin, diagnostics_timeout);
        let startup_timeout = server.startup_timeout_ms.unwrap_or(DEFAULT_STARTUP_TIMEOUT);
        session
            .initialize(&root_uri, &workspace_root, startup_timeout)
            .await?;

        Ok(Self {
            child,
            session,
            workspace_root,
        })
    }

    /// Open or update `file_path`, then wait for a matching diagnostics publication.
    ///
    /// `Some` means the server published diagnostics, including an empty list for
    /// a clean file. `None` means it published nothing before the configured timeout.
    #[instrument(skip(self, file_content))]
    pub async fn diagnostics_for_file(
        &mut self,
        language_id: &str,
        file_path: &Path,
        file_content: &str,
    ) -> Result<Option<Vec<Diagnostic>>> {
        let file_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            self.workspace_root.join(file_path)
        };
        let uri = PathUri::from_host_native_path(&file_path)
            .context("converting document path to a file URI")?
            .to_string();
        self.session
            .diagnostics_for_uri(language_id, &uri, file_content)
            .await
    }

    /// Gracefully shut down the server, killing it if it does not exit promptly.
    pub async fn shutdown(mut self) -> Result<()> {
        self.session.shutdown(SHUTDOWN_TIMEOUT).await?;
        match tokio::time::timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(result) => {
                result.context("waiting for LSP server to exit")?;
            }
            Err(_) => {
                self.child.kill().await.context("killing LSP server")?;
                self.child.wait().await.context("reaping LSP server")?;
            }
        }
        Ok(())
    }
}

struct LspSession<R, W> {
    reader: MessageReader<R>,
    writer: W,
    diagnostics_timeout: Duration,
    next_request_id: i64,
    open_documents: HashMap<String, i32>,
}

impl<R, W> LspSession<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    fn new(reader: R, writer: W, diagnostics_timeout: Duration) -> Self {
        Self {
            reader: MessageReader::new(reader),
            writer,
            diagnostics_timeout,
            next_request_id: 1,
            open_documents: HashMap::new(),
        }
    }

    async fn initialize(
        &mut self,
        root_uri: &str,
        workspace_root: &Path,
        timeout: Duration,
    ) -> Result<()> {
        let request_id = self.next_id();
        let workspace_name = workspace_root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("workspace");
        write_message(
            &mut self.writer,
            &json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "method": "initialize",
                "params": {
                    "processId": std::process::id(),
                    "clientInfo": {"name": "codex-lsp"},
                    "rootUri": root_uri,
                    "workspaceFolders": [{"uri": root_uri, "name": workspace_name}],
                    "capabilities": {
                        "workspace": {"configuration": true},
                        "textDocument": {
                            "publishDiagnostics": {
                                "relatedInformation": true,
                                "versionSupport": true
                            }
                        }
                    }
                }
            }),
        )
        .await?;
        let response = tokio::time::timeout(timeout, self.wait_for_response(request_id))
            .await
            .context("LSP initialize timed out")??;
        assert_successful_response("initialize", &response)?;
        write_message(
            &mut self.writer,
            &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )
        .await
    }

    async fn diagnostics_for_uri(
        &mut self,
        language_id: &str,
        uri: &str,
        file_content: &str,
    ) -> Result<Option<Vec<Diagnostic>>> {
        let version = self.open_documents.get(uri).copied().unwrap_or(0) + 1;
        let message = if version == 1 {
            json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {"textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": file_content
                }}
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didChange",
                "params": {
                    "textDocument": {"uri": uri, "version": version},
                    "contentChanges": [{"text": file_content}]
                }
            })
        };
        write_message(&mut self.writer, &message).await?;
        self.open_documents.insert(uri.to_string(), version);

        let diagnostics_timeout = self.diagnostics_timeout;
        let wait = async {
            loop {
                let message = self
                    .reader
                    .next_message()
                    .await?
                    .context("LSP server closed while waiting for diagnostics")?;
                if is_publish_diagnostics_for(&message, uri)
                    && !is_stale_publication(&message, version)
                {
                    return Ok(parse_diagnostics(&message));
                }
                self.handle_auxiliary_message(&message).await?;
            }
        };

        match tokio::time::timeout(diagnostics_timeout, wait).await {
            Ok(result) => result.map(Some),
            Err(_) => Ok(None),
        }
    }

    async fn shutdown(&mut self, timeout: Duration) -> Result<()> {
        let request_id = self.next_id();
        write_message(
            &mut self.writer,
            &json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "method": "shutdown",
                "params": null
            }),
        )
        .await?;
        let response = tokio::time::timeout(timeout, self.wait_for_response(request_id))
            .await
            .context("LSP shutdown timed out")??;
        assert_successful_response("shutdown", &response)?;
        write_message(
            &mut self.writer,
            &json!({"jsonrpc": "2.0", "method": "exit", "params": null}),
        )
        .await?;
        self.writer.shutdown().await.context("closing LSP stdin")
    }

    fn next_id(&mut self) -> i64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    async fn wait_for_response(&mut self, request_id: i64) -> Result<Value> {
        loop {
            let message = self
                .reader
                .next_message()
                .await?
                .context("LSP server closed before responding")?;
            if is_response_for(&message, request_id) {
                return Ok(message);
            }
            self.handle_auxiliary_message(&message).await?;
        }
    }

    async fn handle_auxiliary_message(&mut self, message: &Value) -> Result<()> {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Ok(());
        };
        let Some(id) = message.get("id") else {
            return Ok(());
        };
        let response = server_request_response(method, id, message.get("params"));
        write_message(&mut self.writer, &response).await
    }
}

fn server_request_response(method: &str, id: &Value, params: Option<&Value>) -> Value {
    let result = match method {
        "workspace/configuration" => {
            let count = params
                .and_then(|value| value.get("items"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Some(Value::Array(vec![Value::Null; count]))
        }
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create" => Some(Value::Null),
        _ => None,
    };
    match result {
        Some(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        None => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": format!("unsupported server request: {method}")}
        }),
    }
}

fn is_response_for(message: &Value, request_id: i64) -> bool {
    message.get("method").is_none()
        && message.get("id") == Some(&json!(request_id))
        && (message.get("result").is_some() || message.get("error").is_some())
}

fn assert_successful_response(method: &str, response: &Value) -> Result<()> {
    if let Some(error) = response.get("error") {
        bail!("LSP {method} failed: {error}");
    }
    Ok(())
}

fn is_publish_diagnostics_for(message: &Value, uri: &str) -> bool {
    message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
        && message.pointer("/params/uri").and_then(Value::as_str) == Some(uri)
}

fn is_stale_publication(message: &Value, document_version: i32) -> bool {
    message
        .pointer("/params/version")
        .and_then(Value::as_i64)
        .is_some_and(|version| version < i64::from(document_version))
}

fn parse_diagnostics(message: &Value) -> Vec<Diagnostic> {
    let uri = message
        .pointer("/params/uri")
        .and_then(Value::as_str)
        .unwrap_or_default();
    message
        .pointer("/params/diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|diagnostic| parse_one(uri, diagnostic))
        .take(MAX_DIAGNOSTICS_PER_FILE)
        .collect()
}

fn parse_one(uri: &str, value: &Value) -> Option<Diagnostic> {
    let line = u32::try_from(value.pointer("/range/start/line").and_then(Value::as_u64)?).ok()?;
    let character = u32::try_from(
        value
            .pointer("/range/start/character")
            .and_then(Value::as_u64)?,
    )
    .ok()?;
    let severity = value
        .get("severity")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .map_or(DiagnosticSeverity::Error, DiagnosticSeverity::from_lsp);
    let code = value.get("code").and_then(|code| match code {
        Value::String(code) => Some(code.clone()),
        Value::Number(code) => Some(code.to_string()),
        _ => None,
    });
    Some(Diagnostic {
        uri: uri.to_string(),
        line,
        character,
        severity,
        message: value.get("message").and_then(Value::as_str)?.to_string(),
        code,
        source: value
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .context("resolving current directory")?
            .join(path))
    }
}

async fn drain_stderr(mut stderr: tokio::process::ChildStderr, command: String) {
    let mut buffer = [0_u8; 4096];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) => break,
            Ok(length) => debug!(
                command,
                stderr = %String::from_utf8_lossy(&buffer[..length]),
                "language server stderr"
            ),
            Err(error) => {
                debug!(command, %error, "failed reading language server stderr");
                break;
            }
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
