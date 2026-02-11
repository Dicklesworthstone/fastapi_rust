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
const MAX_TEXT_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const CLOSE_CODE_PROTOCOL_ERROR: u16 = 1002;
const CLOSE_CODE_UNSUPPORTED_DATA: u16 = 1003;
const CLOSE_CODE_INVALID_PAYLOAD: u16 = 1007;
const CLOSE_CODE_MESSAGE_TOO_BIG: u16 = 1009;

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

    let decoded = base64_decode(key).ok_or(WebSocketHandshakeError::InvalidKeyBase64)?;
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
    MessageTooLarge { size: usize, limit: usize },
}

impl std::fmt::Display for WebSocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "websocket I/O error: {e}"),
            Self::Protocol(msg) => write!(f, "websocket protocol error: {msg}"),
            Self::Utf8(e) => write!(f, "invalid utf-8 in websocket text frame: {e}"),
            Self::MessageTooLarge { size, limit } => {
                write!(
                    f,
                    "websocket message too large: {size} bytes (limit {limit})"
                )
            }
        }
    }
}

impl std::error::Error for WebSocketError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Utf8(e) => Some(e),
            Self::Protocol(_) | Self::MessageTooLarge { .. } => None,
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
        Self {
            stream,
            rx: buffered,
        }
    }

    /// Read the next frame.
    pub async fn read_frame(&mut self) -> Result<Frame, WebSocketError> {
        let header = self.read_exact_buf(2).await?;
        let b0 = header[0];
        let b1 = header[1];

        let fin = (b0 & 0x80) != 0;
        let rsv = (b0 >> 4) & 0x07;
        if rsv != 0 {
            return Err(WebSocketError::Protocol(
                "reserved bits must be 0 (no extensions negotiated)",
            ));
        }
        let opcode =
            OpCode::from_u8(b0 & 0x0f).ok_or(WebSocketError::Protocol("invalid opcode"))?;
        let masked = (b1 & 0x80) != 0;
        let mut len7 = u64::from(b1 & 0x7f);

        if opcode.is_control() && !fin {
            return Err(WebSocketError::Protocol(
                "control frames must not be fragmented",
            ));
        }

        if len7 == 126 {
            let b = self.read_exact_buf(2).await?;
            len7 = u64::from(u16::from_be_bytes([b[0], b[1]]));
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
        let payload_len = usize::try_from(len7).map_err(|_| WebSocketError::MessageTooLarge {
            size: usize::MAX,
            limit: MAX_TEXT_MESSAGE_BYTES,
        })?;

        if opcode.is_control() && payload_len > 125 {
            return Err(WebSocketError::Protocol("control frame too large"));
        }
        if payload_len > MAX_TEXT_MESSAGE_BYTES {
            return Err(WebSocketError::MessageTooLarge {
                size: payload_len,
                limit: MAX_TEXT_MESSAGE_BYTES,
            });
        }

        let mask = self.read_exact_buf(4).await?;

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

        let len = u64::try_from(frame.payload.len())
            .map_err(|_| WebSocketError::Protocol("len too large"))?;
        if len <= 125 {
            out.push(len as u8);
        } else if let Ok(len16) = u16::try_from(len) {
            out.push(126);
            out.extend_from_slice(&len16.to_be_bytes());
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
        self.read_text_or_close()
            .await?
            .ok_or(WebSocketError::Protocol("websocket closed"))
    }

    /// Convenience: read a text message, transparently handling ping/pong/close.
    ///
    /// Behavior:
    /// - `Ping` frames are answered with `Pong` (same payload) and ignored.
    /// - `Pong` frames are ignored.
    /// - `Close` frames are replied to with a `Close` echo and return `Ok(None)`.
    /// - Any non-text data frame returns a protocol error.
    pub async fn read_text_or_close(&mut self) -> Result<Option<String>, WebSocketError> {
        let mut text_fragments: Vec<u8> = Vec::new();
        let mut collecting_text_fragments = false;

        loop {
            let frame = match self.read_frame().await {
                Ok(frame) => frame,
                Err(err @ WebSocketError::MessageTooLarge { .. }) => {
                    let _ = self.send_close_code(CLOSE_CODE_MESSAGE_TOO_BIG).await;
                    return Err(err);
                }
                Err(err @ WebSocketError::Protocol(_)) => {
                    // Malformed frames should trigger a protocol close frame.
                    let _ = self.send_close_code(CLOSE_CODE_PROTOCOL_ERROR).await;
                    return Err(err);
                }
                Err(err) => return Err(err),
            };
            match frame.opcode {
                OpCode::Text => {
                    if collecting_text_fragments {
                        let _ = self.send_close_code(CLOSE_CODE_PROTOCOL_ERROR).await;
                        return Err(WebSocketError::Protocol(
                            "new text frame before fragmented text completed",
                        ));
                    }
                    if frame.fin {
                        match std::str::from_utf8(&frame.payload) {
                            Ok(s) => return Ok(Some(s.to_string())),
                            Err(err) => {
                                let _ = self.send_close_code(CLOSE_CODE_INVALID_PAYLOAD).await;
                                return Err(WebSocketError::Utf8(err));
                            }
                        }
                    }

                    if frame.payload.len() > MAX_TEXT_MESSAGE_BYTES {
                        let _ = self.send_close_code(CLOSE_CODE_MESSAGE_TOO_BIG).await;
                        return Err(WebSocketError::Protocol("text message too large"));
                    }
                    text_fragments.extend_from_slice(&frame.payload);
                    collecting_text_fragments = true;
                }
                OpCode::Ping => {
                    self.send_pong(&frame.payload).await?;
                }
                OpCode::Pong => {}
                OpCode::Close => {
                    if !is_valid_close_payload(&frame.payload) {
                        // RFC 6455: malformed close payload is a protocol error.
                        let _ = self.send_close_code(CLOSE_CODE_PROTOCOL_ERROR).await;
                        return Err(WebSocketError::Protocol("invalid close frame payload"));
                    }
                    // Echo the close payload (if any) and let the caller exit cleanly.
                    let close = Frame {
                        fin: true,
                        opcode: OpCode::Close,
                        payload: frame.payload,
                    };
                    let _ = self.write_frame(&close).await;
                    return Ok(None);
                }
                OpCode::Binary => {
                    let _ = self.send_close_code(CLOSE_CODE_UNSUPPORTED_DATA).await;
                    return Err(WebSocketError::Protocol(
                        "expected text frame, got binary frame",
                    ));
                }
                OpCode::Continuation => {
                    if !collecting_text_fragments {
                        let _ = self.send_close_code(CLOSE_CODE_PROTOCOL_ERROR).await;
                        return Err(WebSocketError::Protocol("unexpected continuation frame"));
                    }

                    let next_size = text_fragments.len().saturating_add(frame.payload.len());
                    if next_size > MAX_TEXT_MESSAGE_BYTES {
                        let _ = self.send_close_code(CLOSE_CODE_MESSAGE_TOO_BIG).await;
                        return Err(WebSocketError::Protocol("text message too large"));
                    }
                    text_fragments.extend_from_slice(&frame.payload);

                    if frame.fin {
                        match std::str::from_utf8(&text_fragments) {
                            Ok(s) => return Ok(Some(s.to_string())),
                            Err(err) => {
                                let _ = self.send_close_code(CLOSE_CODE_INVALID_PAYLOAD).await;
                                return Err(WebSocketError::Utf8(err));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Send a `Pong` control frame (server-side, unmasked).
    pub async fn send_pong(&mut self, payload: &[u8]) -> Result<(), WebSocketError> {
        if payload.len() > 125 {
            return Err(WebSocketError::Protocol("pong payload too large"));
        }
        let frame = Frame {
            fin: true,
            opcode: OpCode::Pong,
            payload: payload.to_vec(),
        };
        self.write_frame(&frame).await
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

    async fn send_close_code(&mut self, close_code: u16) -> Result<(), WebSocketError> {
        let frame = Frame {
            fin: true,
            opcode: OpCode::Close,
            payload: close_code.to_be_bytes().to_vec(),
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
    let padded_len = (data.len() + 9).div_ceil(64) * 64;
    let mut msg = Vec::with_capacity(padded_len);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut words = [0u32; 80];
        for (word_index, word) in words.iter_mut().take(16).enumerate() {
            let byte_index = word_index * 4;
            *word = u32::from_be_bytes([
                chunk[byte_index],
                chunk[byte_index + 1],
                chunk[byte_index + 2],
                chunk[byte_index + 3],
            ]);
        }
        for i in 16..80 {
            words[i] = (words[i - 3] ^ words[i - 8] ^ words[i - 14] ^ words[i - 16]).rotate_left(1);
        }

        let mut state_a = h0;
        let mut state_b = h1;
        let mut state_c = h2;
        let mut state_d = h3;
        let mut state_e = h4;

        for (round, &word) in words.iter().enumerate() {
            let (mix, constant) = match round {
                0..=19 => ((state_b & state_c) | ((!state_b) & state_d), 0x5A827999),
                20..=39 => (state_b ^ state_c ^ state_d, 0x6ED9EBA1),
                40..=59 => (
                    (state_b & state_c) | (state_b & state_d) | (state_c & state_d),
                    0x8F1BBCDC,
                ),
                _ => (state_b ^ state_c ^ state_d, 0xCA62C1D6),
            };
            let temp = state_a
                .rotate_left(5)
                .wrapping_add(mix)
                .wrapping_add(state_e)
                .wrapping_add(constant)
                .wrapping_add(word);
            state_e = state_d;
            state_d = state_c;
            state_c = state_b.rotate_left(30);
            state_b = state_a;
            state_a = temp;
        }

        h0 = h0.wrapping_add(state_a);
        h1 = h1.wrapping_add(state_b);
        h2 = h2.wrapping_add(state_c);
        h3 = h3.wrapping_add(state_d);
        h4 = h4.wrapping_add(state_e);
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
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut idx = 0;
    while idx + 3 <= data.len() {
        let b0 = u32::from(data[idx]);
        let b1 = u32::from(data[idx + 1]);
        let b2 = u32::from(data[idx + 2]);
        let word24 = (b0 << 16) | (b1 << 8) | b2;

        out.push(B64[((word24 >> 18) & 0x3f) as usize] as char);
        out.push(B64[((word24 >> 12) & 0x3f) as usize] as char);
        out.push(B64[((word24 >> 6) & 0x3f) as usize] as char);
        out.push(B64[(word24 & 0x3f) as usize] as char);
        idx += 3;
    }

    let rem = data.len() - idx;
    if rem == 1 {
        let b0 = u32::from(data[idx]);
        let word24 = b0 << 16;
        out.push(B64[((word24 >> 18) & 0x3f) as usize] as char);
        out.push(B64[((word24 >> 12) & 0x3f) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let b0 = u32::from(data[idx]);
        let b1 = u32::from(data[idx + 1]);
        let word24 = (b0 << 16) | (b1 << 8);
        out.push(B64[((word24 >> 18) & 0x3f) as usize] as char);
        out.push(B64[((word24 >> 12) & 0x3f) as usize] as char);
        out.push(B64[((word24 >> 6) & 0x3f) as usize] as char);
        out.push('=');
    }

    out
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let input = input.trim();
    if input.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity((input.len() / 4) * 3);
    let bytes = input.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let is_last = idx + 4 == bytes.len();

        let v0 = decode_b64(bytes[idx])?;
        let v1 = decode_b64(bytes[idx + 1])?;
        let b2 = bytes[idx + 2];
        let b3 = bytes[idx + 3];

        let v2 = if b2 == b'=' {
            if !is_last || b3 != b'=' {
                return None;
            }
            64u32
        } else {
            u32::from(decode_b64(b2)?)
        };

        let v3 = if b3 == b'=' {
            if !is_last {
                return None;
            }
            64u32
        } else {
            u32::from(decode_b64(b3)?)
        };

        let word24 = (u32::from(v0) << 18) | (u32::from(v1) << 12) | (v2 << 6) | v3;
        out.push(((word24 >> 16) & 0xff) as u8);
        if b2 != b'=' {
            out.push(((word24 >> 8) & 0xff) as u8);
        }
        if b3 != b'=' {
            out.push((word24 & 0xff) as u8);
        }

        idx += 4;
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

fn is_valid_close_payload(payload: &[u8]) -> bool {
    if payload.is_empty() {
        return true;
    }
    if payload.len() < 2 {
        return false;
    }

    let code = u16::from_be_bytes([payload[0], payload[1]]);
    if !is_valid_close_code(code) {
        return false;
    }

    if payload.len() == 2 {
        return true;
    }

    std::str::from_utf8(&payload[2..]).is_ok()
}

fn is_valid_close_code(code: u16) -> bool {
    matches!(
        code,
        1000 | 1001 | 1002 | 1003 | 1007 | 1008 | 1009 | 1010 | 1011 | 1012 | 1013 | 1014 | 3000
            ..=4999
    )
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

    #[test]
    fn close_payload_validation() {
        assert!(is_valid_close_payload(&[]));
        assert!(!is_valid_close_payload(&[0x03]));
        assert!(!is_valid_close_payload(&[0x03, 0xEE])); // 1006 cannot be sent
        assert!(is_valid_close_payload(&[0x03, 0xE8])); // 1000
        assert!(is_valid_close_payload(&[0x03, 0xE8, b'o', b'k']));
        assert!(!is_valid_close_payload(&[0x03, 0xE8, 0xFF])); // invalid utf-8 reason
    }
}
