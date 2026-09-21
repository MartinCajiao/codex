use std::path::Path;
use std::time::Duration;

use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tokio::io::DuplexStream;
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;

use super::LspClient;
use super::LspSession;
use super::absolute_path;
use crate::LspServerConfig;
use crate::protocol::MessageReader;
use crate::protocol::write_message;

const TEST_URI: &str = "file:///workspace/main.rs";

type TestSession = LspSession<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>;

fn session_pair(timeout: Duration) -> (TestSession, DuplexStream) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (client_read, client_write) = tokio::io::split(client);
    (LspSession::new(client_read, client_write, timeout), server)
}

#[tokio::test]
async fn initialize_handles_notifications_and_server_requests() {
    let (mut session, server) = session_pair(Duration::from_secs(1));
    let server_task = tokio::spawn(async move {
        let (server_read, mut server_write) = tokio::io::split(server);
        let mut reader = MessageReader::new(server_read);
        let initialize = next_message(&mut reader).await;
        assert_eq!(
            initialize.get("method").and_then(Value::as_str),
            Some("initialize")
        );
        assert_eq!(
            initialize
                .pointer("/params/rootUri")
                .and_then(Value::as_str),
            Some("file:///workspace")
        );
        write_message(
            &mut server_write,
            &json!({"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"ready"}}),
        )
        .await
        .unwrap();
        write_message(
            &mut server_write,
            &json!({
                "jsonrpc":"2.0",
                "id":77,
                "method":"workspace/configuration",
                "params":{"items":[{"section":"rust-analyzer"}]}
            }),
        )
        .await
        .unwrap();
        let configuration = next_message(&mut reader).await;
        assert_eq!(
            configuration,
            json!({"jsonrpc":"2.0","id":77,"result":[null]})
        );
        write_message(
            &mut server_write,
            &json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"capabilities":{}}}),
        )
        .await
        .unwrap();
        let initialized = next_message(&mut reader).await;
        assert_eq!(
            initialized.get("method").and_then(Value::as_str),
            Some("initialized")
        );
    });

    session
        .initialize(
            "file:///workspace",
            Path::new("/workspace"),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn clean_publication_is_distinct_from_timeout() {
    let (mut session, server) = session_pair(Duration::from_secs(1));
    let server_task = tokio::spawn(async move {
        let (server_read, mut server_write) = tokio::io::split(server);
        let mut reader = MessageReader::new(server_read);
        let did_open = next_message(&mut reader).await;
        assert_eq!(
            did_open.get("method").and_then(Value::as_str),
            Some("textDocument/didOpen")
        );
        write_message(
            &mut server_write,
            &json!({
                "jsonrpc":"2.0",
                "method":"textDocument/publishDiagnostics",
                "params":{"uri":TEST_URI,"version":1,"diagnostics":[]}
            }),
        )
        .await
        .unwrap();
    });

    let diagnostics = session
        .diagnostics_for_uri("rust", TEST_URI, "fn main() {}\n")
        .await
        .unwrap();
    assert_eq!(diagnostics, Some(Vec::new()));
    server_task.await.unwrap();
}

#[tokio::test]
async fn repeated_document_uses_did_change_and_ignores_stale_diagnostics() {
    let (mut session, server) = session_pair(Duration::from_secs(1));
    let server_task = tokio::spawn(async move {
        let (server_read, mut server_write) = tokio::io::split(server);
        let mut reader = MessageReader::new(server_read);
        let did_open = next_message(&mut reader).await;
        assert_eq!(
            did_open.pointer("/params/textDocument/version"),
            Some(&json!(1))
        );
        publish_diagnostics(&mut server_write, 1, "first").await;

        let did_change = next_message(&mut reader).await;
        assert_eq!(
            did_change.get("method").and_then(Value::as_str),
            Some("textDocument/didChange")
        );
        assert_eq!(
            did_change.pointer("/params/textDocument/version"),
            Some(&json!(2))
        );
        assert_eq!(
            did_change.pointer("/params/contentChanges/0/text"),
            Some(&json!("fn main() { todo!() }\n"))
        );
        publish_diagnostics(&mut server_write, 1, "stale").await;
        publish_diagnostics(&mut server_write, 2, "current").await;
    });

    let first = session
        .diagnostics_for_uri("rust", TEST_URI, "fn main() {}\n")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first[0].message, "first");
    let second = session
        .diagnostics_for_uri("rust", TEST_URI, "fn main() { todo!() }\n")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second[0].message, "current");
    server_task.await.unwrap();
}

#[tokio::test]
async fn no_publication_returns_none_after_timeout() {
    let (mut session, server) = session_pair(Duration::from_millis(20));
    let server_task = tokio::spawn(async move {
        let (server_read, _server_write) = tokio::io::split(server);
        let mut reader = MessageReader::new(server_read);
        let _did_open = next_message(&mut reader).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    let diagnostics = session
        .diagnostics_for_uri("rust", TEST_URI, "fn main() {}\n")
        .await
        .unwrap();
    assert_eq!(diagnostics, None);
    server_task.abort();
}

#[tokio::test]
async fn missing_server_binary_errors_cleanly() {
    let server = LspServerConfig::new("codex-lsp-definitely-missing-binary-xyz");
    let result = LspClient::start(&server, ".", Duration::from_millis(200)).await;
    assert!(result.is_err());
}

#[test]
fn paths_are_absolute_before_uri_conversion() {
    let path = absolute_path(Path::new("folder with spaces/main #1.rs")).unwrap();
    assert!(path.is_absolute());
    let uri = codex_utils_path_uri::PathUri::from_host_native_path(path)
        .unwrap()
        .to_string();
    assert!(uri.contains("folder%20with%20spaces"));
    assert!(uri.contains("main%20%231.rs"));
}

async fn next_message<R: tokio::io::AsyncRead + Unpin>(reader: &mut MessageReader<R>) -> Value {
    reader.next_message().await.unwrap().unwrap()
}

async fn publish_diagnostics<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    version: i32,
    message: &str,
) {
    write_message(
        writer,
        &json!({
            "jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{
                "uri":TEST_URI,
                "version":version,
                "diagnostics":[{
                    "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                    "severity":1,
                    "message":message
                }]
            }
        }),
    )
    .await
    .unwrap();
}
