use pretty_assertions::assert_eq;
use serde_json::json;

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
async fn reader_returns_none_on_empty() {
    let mut reader = MessageReader::new(&[][..]);
    assert_eq!(reader.next_message().await.expect("read"), None);
}
