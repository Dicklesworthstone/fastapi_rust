//! WebSocket protocol support (RFC 6455).
//!
//! This module provides:
//! - WebSocket handshake helpers (`Sec-WebSocket-Accept`)
//! - A minimal frame codec (mask/unmask, ping/pong/close, text/binary)
//!
//! Design constraints for this project:
//! - No Tokio
//! - Minimal dependencies (implement SHA1 + base64 locally)
//! - Cancel-correct: all I/O is async and can be cancelled via asupersync

use asupersync::io::{AsyncRead, AsyncWrite, ReadBuf};
use asupersync::net::TcpStream;
use std::future::poll_fn;
use std::io;
use std::pin::Pin;
use std::task::Poll;

/// The GUID used for computing `Sec-WebSocket-Accept`.
pub const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// WebSocket handshake error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketHandshakeError {
    /// Missing required header.
    MissingHeader(&'static str),
    /// Invalid base64 in `Sec-WebSocket-Key`.
    InvalidKeyBase64,
    /// Invalid key length (decoded bytes must be 16).
    InvalidKeyLength { decoded_len: usize },
}

impl std::fmt::Display for WebSocketHandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHeader(h) => write!(f, "missing required websocket header: {h}"),
            Self::InvalidKeyBase64 => write!(f, "invalid Sec-WebSocket-Key (base64 decode failed)"),
            Self::InvalidKeyLength { decoded_len } => write!(
                f,
                "invalid Sec-WebSocket-Key (decoded length {decoded_len}, expected 16)"
            ),
        }
    }
}

impl std::error::Error for WebSocketHandshakeError {}

/// Compute `Sec-WebSocket-Accept` from `Sec-WebSocket-Key` (RFC 6455).
///
/// Validates that the key is base64 and decodes to 16 bytes (as required by RFC 6455).
pub fn websocket_accept_from_key(key: &str) -> Result<String, WebSocketHandshakeError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(WebSocketHandshakeError::MissingHeader("sec-websocket-key"));
    }

    let decoded =
        base64_decode(key).ok_or(WebSocketHandshakeError::InvalidKeyBase64)?;
    if decoded.len() != 16 {
        return Err(WebSocketHandshakeError::InvalidKeyLength {
            decoded_len: decoded.len(),
        });
    }

    let mut input = Vec::with_capacity(key.len() + WS_GUID.len());
    input.extend_from_slice(key.as_bytes());
    input.extend_from_slice(WS_GUID.as_bytes());

    let digest = sha1(&input);
    Ok(base64_encode(&digest))
}

/// WebSocket opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    Continuation = 0x0,
    Text = 0x1,
    Binary = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

impl OpCode {
    fn from_u8(b: u8) -> Option<Self> {
        match b {
            0x0 => Some(Self::Continuation),
            0x1 => Some(Self::Text),
            0x2 => Some(Self::Binary),
            0x8 => Some(Self::Close),
            0x9 => Some(Self::Ping),
            0xA => Some(Self::Pong),
            _ => None,
        }
    }

    fn is_control(self) -> bool {
        matches!(self, Self::Close | Self::Ping | Self::Pong)
    }
}

/// A single WebSocket frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub fin: bool,
    pub opcode: OpCode,
    pub payload: Vec<u8>,
}

/// WebSocket protocol error.
#[derive(Debug)]
pub enum WebSocketError {
    Io(io::Error),
    Protocol(&'static str),
    Utf8(std::str::Utf8Error),
}

impl std::fmt::Display for WebSocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "websocket I/O error: {e}"),
            Self::Protocol(msg) => write!(f, "websocket protocol error: {msg}"),
            Self::Utf8(e) => write!(f, "invalid utf-8 in websocket text frame: {e}"),
        }
    }
}

impl std::error::Error for WebSocketError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Utf8(e) => Some(e),
            Self::Protocol(_) => None,
        }
    }
}

impl From<io::Error> for WebSocketError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<std::str::Utf8Error> for WebSocketError {
    fn from(e: std::str::Utf8Error) -> Self {
        Self::Utf8(e)
    }
}

/// A WebSocket connection (server-side).
///
/// Notes:
/// - Server -> client frames are not masked.
/// - Client -> server frames must be masked (enforced).
#[derive(Debug)]
pub struct WebSocket {
    stream: TcpStream,
    rx: Vec<u8>,
}

impl WebSocket {
    /// Create a websocket from a TCP stream and an optional prefix of already-buffered bytes.
    #[must_use]
    pub fn new(stream: TcpStream, buffered: Vec<u8>) -> Self {
        Self { stream, rx: buffered }
    }

    /// Read the next frame.
    pub async fn read_frame(&mut self) -> Result<Frame, WebSocketError> {
        let header = self.read_exact_buf(2).await?;
        let b0 = header[0];
        let b1 = header[1];

        let fin = (b0 & 0x80) != 0;
        let opcode = OpCode::from_u8(b0 & 0x0f).ok_or(WebSocketError::Protocol("invalid opcode"))?;
        let masked = (b1 & 0x80) != 0;
        let mut len7 = (b1 & 0x7f) as u64;

        if opcode.is_control() && !fin {
            return Err(WebSocketError::Protocol("control frames must not be fragmented"));
        }

        if len7 == 126 {
            let b = self.read_exact_buf(2).await?;
            len7 = u16::from_be_bytes([b[0], b[1]]) as u64;
        } else if len7 == 127 {
            let b = self.read_exact_buf(8).await?;
            len7 = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
            // Most implementations reject lengths with the high bit set (non-minimal encoding).
            if (len7 >> 63) != 0 {
                return Err(WebSocketError::Protocol("invalid 64-bit length"));
            }
        }

        if !masked {
            return Err(WebSocketError::Protocol(
                "client->server frames must be masked",
            ));
        }
        let mask = self.read_exact_buf(4).await?;
        let payload_len = usize::try_from(len7).map_err(|_| WebSocketError::Protocol("len too large"))?;

        if opcode.is_control() && payload_len > 125 {
            return Err(WebSocketError::Protocol("control frame too large"));
        }

        let mut payload = self.read_exact_buf(payload_len).await?;
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i & 3];
        }

        Ok(Frame {
            fin,
            opcode,
            payload,
        })
    }

    /// Write a frame to the peer (server-side, unmasked).
    pub async fn write_frame(&mut self, frame: &Frame) -> Result<(), WebSocketError> {
        let mut out = Vec::with_capacity(2 + frame.payload.len() + 8);
        let b0 = (if frame.fin { 0x80 } else { 0 }) | (frame.opcode as u8);
        out.push(b0);

        let len = frame.payload.len() as u64;
        if len <= 125 {
            out.push(len as u8);
        } else if len <= u16::MAX as u64 {
            out.push(126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(127);
            out.extend_from_slice(&len.to_be_bytes());
        }

        out.extend_from_slice(&frame.payload);
        write_all(&mut self.stream, &out).await?;
        flush(&mut self.stream).await?;
        Ok(())
    }

    /// Convenience: read a text message.
    pub async fn read_text(&mut self) -> Result<String, WebSocketError> {
        let frame = self.read_frame().await?;
        if frame.opcode != OpCode::Text {
            return Err(WebSocketError::Protocol("expected text frame"));
        }
        let s = std::str::from_utf8(&frame.payload)?;
        Ok(s.to_string())
    }

    /// Convenience: send a text message.
    pub async fn send_text(&mut self, text: &str) -> Result<(), WebSocketError> {
        let frame = Frame {
            fin: true,
            opcode: OpCode::Text,
            payload: text.as_bytes().to_vec(),
        };
        self.write_frame(&frame).await
    }

    async fn read_exact_buf(&mut self, n: usize) -> Result<Vec<u8>, WebSocketError> {
        while self.rx.len() < n {
            let mut tmp = vec![0u8; 8192];
            let read = read_once(&mut self.stream, &mut tmp).await?;
            if read == 0 {
                return Err(WebSocketError::Protocol("unexpected EOF"));
            }
            self.rx.extend_from_slice(&tmp[..read]);
        }

        let out = self.rx.drain(..n).collect();
        Ok(out)
    }
}

async fn read_once(stream: &mut TcpStream, buffer: &mut [u8]) -> io::Result<usize> {
    poll_fn(|cx| {
        let mut read_buf = ReadBuf::new(buffer);
        match Pin::new(&mut *stream).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buf.filled().len())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

async fn write_all(stream: &mut TcpStream, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let n = poll_fn(|cx| Pin::new(&mut *stream).poll_write(cx, buf)).await?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "write zero"));
        }
        buf = &buf[n..];
    }
    Ok(())
}

async fn flush(stream: &mut TcpStream) -> io::Result<()> {
    poll_fn(|cx| Pin::new(&mut *stream).poll_flush(cx)).await
}

// =============================================================================
// SHA1 (RFC 3174) - minimal implementation
// =============================================================================

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bit_len = (data.len() as u64) * 8;
    let mut msg = Vec::with_capacity(((data.len() + 9 + 63) / 64) * 64);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let j = i * 4;
            *word = u32::from_be_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

// =============================================================================
// Base64 (RFC 4648) - minimal (no alloc-free tricks; small and deterministic)
// =============================================================================

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = Vec::with_capacity(((data.len() + 2) / 3) * 4);
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        out.push(B64[((n >> 18) & 0x3f) as usize]);
        out.push(B64[((n >> 12) & 0x3f) as usize]);
        out.push(B64[((n >> 6) & 0x3f) as usize]);
        out.push(B64[(n & 0x3f) as usize]);
        i += 3;
    }

    let rem = data.len() - i;
    if rem == 1 {
        let n = (data[i] as u32) << 16;
        out.push(B64[((n >> 18) & 0x3f) as usize]);
        out.push(B64[((n >> 12) & 0x3f) as usize]);
        out.push(b'=');
        out.push(b'=');
    } else if rem == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        out.push(B64[((n >> 18) & 0x3f) as usize]);
        out.push(B64[((n >> 12) & 0x3f) as usize]);
        out.push(B64[((n >> 6) & 0x3f) as usize]);
        out.push(b'=');
    }

    // ASCII-only; safe unwrap.
    String::from_utf8(out).unwrap()
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity((s.len() / 4) * 3);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let a = decode_b64(bytes[i])?;
        let b = decode_b64(bytes[i + 1])?;
        let c = bytes[i + 2];
        let d = bytes[i + 3];

        let c_val = if c == b'=' { 64 } else { decode_b64(c)? as u32 };
        let d_val = if d == b'=' { 64 } else { decode_b64(d)? as u32 };

        let n = ((a as u32) << 18) | ((b as u32) << 12) | (c_val << 6) | d_val;
        out.push(((n >> 16) & 0xff) as u8);
        if c != b'=' {
            out.push(((n >> 8) & 0xff) as u8);
        }
        if d != b'=' {
            out.push((n & 0xff) as u8);
        }

        i += 4;
    }
    Some(out)
}

fn decode_b64(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_key_known_vector() {
        // RFC 6455 example
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let accept = websocket_accept_from_key(key).unwrap();
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn base64_roundtrip_small() {
        let data = b"hello world";
        let enc = base64_encode(data);
        let dec = base64_decode(&enc).unwrap();
        assert_eq!(dec, data);
    }
}

