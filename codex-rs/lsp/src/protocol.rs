//! Minimal, cancellation-safe LSP JSON-RPC framing over stdio.

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde_json::Value;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;

const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const READ_CHUNK_BYTES: usize = 8 * 1024;

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
///
/// Bytes are retained on cancellation, so timing out a read cannot desynchronize
/// the next frame.
pub struct MessageReader<R> {
    reader: R,
    buffer: Vec<u8>,
}

impl<R: AsyncRead + Unpin> MessageReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::new(),
        }
    }

    /// Read the next JSON-RPC message. Returns `Ok(None)` on clean EOF.
    pub async fn next_message(&mut self) -> Result<Option<Value>> {
        loop {
            if let Some(header_end) = find_header_end(&self.buffer) {
                if header_end > MAX_HEADER_BYTES {
                    bail!("LSP header exceeds {MAX_HEADER_BYTES} bytes");
                }
                let content_length = parse_content_length(&self.buffer[..header_end])?;
                if content_length > MAX_MESSAGE_BYTES {
                    bail!("LSP message exceeds {MAX_MESSAGE_BYTES} bytes");
                }
                let frame_end = header_end
                    .checked_add(content_length)
                    .context("LSP frame length overflow")?;
                if self.buffer.len() >= frame_end {
                    let value = serde_json::from_slice(&self.buffer[header_end..frame_end])
                        .context("parsing LSP message")?;
                    self.buffer.drain(..frame_end);
                    return Ok(Some(value));
                }
            } else if self.buffer.len() > MAX_HEADER_BYTES {
                bail!("LSP header exceeds {MAX_HEADER_BYTES} bytes");
            }

            let mut chunk = [0_u8; READ_CHUNK_BYTES];
            let bytes_read = self.reader.read(&mut chunk).await?;
            if bytes_read == 0 {
                if self.buffer.is_empty() {
                    return Ok(None);
                }
                bail!("unexpected EOF in LSP frame");
            }
            self.buffer.extend_from_slice(&chunk[..bytes_read]);
        }
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn parse_content_length(header: &[u8]) -> Result<usize> {
    let header = std::str::from_utf8(header).context("non-UTF-8 LSP header")?;
    let mut content_length = None;
    for line in header.lines().filter(|line| !line.is_empty()) {
        let Some((name, value)) = line.split_once(':') else {
            bail!("malformed LSP header line");
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            if content_length.is_some() {
                bail!("duplicate Content-Length header");
            }
            content_length = Some(value.trim().parse().context("invalid Content-Length")?);
        }
    }
    content_length.context("missing Content-Length header")
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
