use std::time::Duration;

use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::MAX_MESSAGE_BYTES;
use super::MessageReader;
use super::encode_message;

#[test]
fn encode_message_adds_content_length() {
    let bytes = encode_message(&json!({"jsonrpc": "2.0"})).expect("encode");
    let text = String::from_utf8(bytes).expect("utf8");
    assert!(text.starts_with("Content-Length:"));
    assert!(text.contains(r#"{"jsonrpc":"2.0"}"#));
}

#[tokio::test]
async fn reader_round_trips_one_message() {
    let bytes = encode_message(&json!({"jsonrpc": "2.0", "id": 1})).expect("encode");
    let mut reader = MessageReader::new(&bytes[..]);
    let message = reader.next_message().await.expect("read").expect("message");
    assert_eq!(message, json!({"jsonrpc": "2.0", "id": 1}));
}

#[tokio::test]
async fn cancelled_read_retains_partial_frame() {
    let value = json!({"jsonrpc": "2.0", "id": 1});
    let bytes = encode_message(&value).unwrap();
    let split = bytes.len() - 3;
    let (mut writer, stream) = tokio::io::duplex(1024);
    writer.write_all(&bytes[..split]).await.unwrap();
    let mut reader = MessageReader::new(stream);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), reader.next_message())
            .await
            .is_err()
    );
    writer.write_all(&bytes[split..]).await.unwrap();
    assert_eq!(reader.next_message().await.unwrap(), Some(value));
}

#[tokio::test]
async fn reader_rejects_oversized_message() {
    let frame = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1);
    let mut reader = MessageReader::new(frame.as_bytes());
    assert!(reader.next_message().await.is_err());
}

#[tokio::test]
async fn reader_returns_none_on_empty() {
    let mut reader = MessageReader::new(&[][..]);
    assert_eq!(reader.next_message().await.expect("read"), None);
}
