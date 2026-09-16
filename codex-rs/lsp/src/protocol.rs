//! Minimal LSP JSON-RPC framing (`Content-Length` headers over stdio).

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Encode one JSON-RPC value with LSP headers.
pub fn encode_message(value: &Value) -> Result<Vec<u8>> {
    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_MESSAGE_BYTES {
        bail!("LSP message exceeds {MAX_MESSAGE_BYTES} bytes");
    }
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut out = header.into_bytes();
    out.extend_from_slice(&body);
    Ok(out)
}

/// Incrementally decode LSP-framed messages from an async reader.
pub struct MessageReader<R> {
    reader: BufReader<R>,
}

impl<R: AsyncRead + Unpin> MessageReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            reader: BufReader::new(inner),
        }
    }

    /// Read the next JSON-RPC message. Returns `Ok(None)` on clean EOF.
    pub async fn next_message(&mut self) -> Result<Option<Value>> {
        let content_length = read_headers(&mut self.reader).await?;
        let Some(content_length) = content_length else {
            return Ok(None);
        };
        if content_length > MAX_MESSAGE_BYTES {
            bail!("LSP message exceeds {MAX_MESSAGE_BYTES} bytes");
        }
        let mut body = vec![0u8; content_length];
        tokio::io::AsyncReadExt::read_exact(&mut self.reader, &mut body)
            .await
            .context("reading LSP message body")?;
        let value: Value = serde_json::from_slice(&body).context("parsing LSP message")?;
        Ok(Some(value))
    }
}

async fn read_headers<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Result<Option<usize>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).await?;
        if bytes == 0 {
            if content_length.is_none() {
                return Ok(None);
            }
            bail!("unexpected EOF in LSP headers");
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(rest.trim().parse().context("invalid Content-Length")?);
        }
    }
    content_length
        .context("missing Content-Length header")
        .map(Some)
}

/// Write one JSON-RPC value with LSP framing.
pub async fn write_message<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> Result<()> {
    let bytes = encode_message(value)?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
