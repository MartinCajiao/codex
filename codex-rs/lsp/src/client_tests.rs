use std::path::Path;
use std::time::Duration;

use pretty_assertions::assert_eq;

use super::LspClient;
use crate::LspServerConfig;

#[test]
fn startup_defaults_to_ten_seconds() {
    let server = LspServerConfig::new("pyright-langserver");
    let client = LspClient::new(&server, Duration::from_millis(200));
    assert_eq!(client.startup_timeout(), Duration::from_secs(10));
}

#[tokio::test]
async fn missing_server_binary_errors_cleanly() {
    let server = LspServerConfig::new("codex-lsp-definitely-missing-binary-xyz");
    let client = LspClient::new(&server, Duration::from_millis(200));
    let result = client
        .diagnostics_for_file("python", Path::new("main.py"), "x = 1\n")
        .await;
    assert!(result.is_err());
}

#[test]
fn client_config_prefers_server_timeout() {
    let mut server = LspServerConfig::new("pyright-langserver");
    server.startup_timeout_ms = Some(Duration::from_secs(3));
    let client =
        LspClient::with_timeouts(&server, Duration::from_secs(9), Duration::from_millis(100));
    assert_eq!(client.startup_timeout(), Duration::from_secs(3));
}
