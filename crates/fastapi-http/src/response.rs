//! HTTP response writer.

use asupersync::stream::Stream;
use fastapi_core::{BodyStream, Response, ResponseBody, StatusCode};
use std::pin::Pin;
use std::task::{Context, Poll};

/// Serialized response output.
pub enum ResponseWrite {
    /// Fully-buffered response bytes.
    Full(Vec<u8>),
    /// Chunked stream (head + body chunks).
    Stream(ChunkedEncoder),
}

/// Streaming chunked response encoder.
pub struct ChunkedEncoder {
    head: Option<Vec<u8>>,
    body: BodyStream,
    finished: bool,
}

impl ChunkedEncoder {
    fn new(head: Vec<u8>, body: BodyStream) -> Self {
        Self {
            head: Some(head),
            body,
            finished: false,
        }
    }

    fn encode_chunk(chunk: &[u8]) -> Vec<u8> {
        let size = format!("{:x}", chunk.len());
        let mut out = Vec::with_capacity(size.len() + 2 + chunk.len() + 2);
        out.extend_from_slice(size.as_bytes());
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\r\n");
        out
    }
}

impl Stream for ChunkedEncoder {
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(head) = self.head.take() {
            return Poll::Ready(Some(head));
        }

        if self.finished {
            return Poll::Ready(None);
        }

        loop {
            match self.body.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(chunk)) => {
                    if chunk.is_empty() {
                        continue;
                    }
                    return Poll::Ready(Some(Self::encode_chunk(&chunk)));
                }
                Poll::Ready(None) => {
                    self.finished = true;
                    return Poll::Ready(Some(b"0\r\n\r\n".to_vec()));
                }
            }
        }
    }
}

/// Writes HTTP responses to a buffer.
pub struct ResponseWriter {
    buffer: Vec<u8>,
}

impl ResponseWriter {
    /// Create a new response writer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(4096),
        }
    }

    /// Write a response into either a full buffer or a stream.
    #[must_use]
    pub fn write(&mut self, response: Response) -> ResponseWrite {
        let (status, headers, body) = response.into_parts();
        match body {
            ResponseBody::Empty => {
                let bytes = self.write_full(status, &headers, &[]);
                ResponseWrite::Full(bytes)
            }
            ResponseBody::Bytes(body) => {
                let bytes = self.write_full(status, &headers, &body);
                ResponseWrite::Full(bytes)
            }
            ResponseBody::Stream(body) => {
                let head = self.write_stream_head(status, &headers);
                ResponseWrite::Stream(ChunkedEncoder::new(head, body))
            }
        }
    }

    fn write_full(
        &mut self,
        status: StatusCode,
        headers: &[(String, Vec<u8>)],
        body: &[u8],
    ) -> Vec<u8> {
        self.buffer.clear();

        // Status line
        self.buffer.extend_from_slice(b"HTTP/1.1 ");
        self.write_status(status);
        self.buffer.extend_from_slice(b"\r\n");

        // Headers (filter hop-by-hop content-length/transfer-encoding)
        for (name, value) in headers {
            if is_content_length(name) || is_transfer_encoding(name) {
                continue;
            }
            self.buffer.extend_from_slice(name.as_bytes());
            self.buffer.extend_from_slice(b": ");
            self.buffer.extend_from_slice(value);
            self.buffer.extend_from_slice(b"\r\n");
        }

        // Content-Length
        self.buffer.extend_from_slice(b"content-length: ");
        self.buffer
            .extend_from_slice(body.len().to_string().as_bytes());
        self.buffer.extend_from_slice(b"\r\n");

        // End of headers
        self.buffer.extend_from_slice(b"\r\n");

        // Body
        self.buffer.extend_from_slice(body);

        self.take_buffer()
    }

    fn write_stream_head(&mut self, status: StatusCode, headers: &[(String, Vec<u8>)]) -> Vec<u8> {
        self.buffer.clear();

        // Status line
        self.buffer.extend_from_slice(b"HTTP/1.1 ");
        self.write_status(status);
        self.buffer.extend_from_slice(b"\r\n");

        // Headers (filter hop-by-hop content-length/transfer-encoding)
        for (name, value) in headers {
            if is_content_length(name) || is_transfer_encoding(name) {
                continue;
            }
            self.buffer.extend_from_slice(name.as_bytes());
            self.buffer.extend_from_slice(b": ");
            self.buffer.extend_from_slice(value);
            self.buffer.extend_from_slice(b"\r\n");
        }

        // Transfer-Encoding: chunked
        self.buffer
            .extend_from_slice(b"transfer-encoding: chunked\r\n");

        // End of headers
        self.buffer.extend_from_slice(b"\r\n");

        self.take_buffer()
    }

    fn write_status(&mut self, status: StatusCode) {
        let code = status.as_u16();
        self.buffer.extend_from_slice(code.to_string().as_bytes());
        self.buffer.extend_from_slice(b" ");
        self.buffer
            .extend_from_slice(status.canonical_reason().as_bytes());
    }

    fn take_buffer(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        std::mem::swap(&mut out, &mut self.buffer);
        self.buffer = Vec::with_capacity(out.capacity());
        out
    }
}

fn is_content_length(name: &str) -> bool {
    name.eq_ignore_ascii_case("content-length")
}

fn is_transfer_encoding(name: &str) -> bool {
    name.eq_ignore_ascii_case("transfer-encoding")
}

impl Default for ResponseWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asupersync::stream::iter;
    use std::sync::Arc;
    use std::task::{Wake, Waker};

    struct NoopWaker;

    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }

    fn noop_waker() -> Waker {
        Waker::from(Arc::new(NoopWaker))
    }

    fn collect_stream<S: Stream<Item = Vec<u8>> + Unpin>(mut stream: S) -> Vec<u8> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut out = Vec::new();

        loop {
            match Pin::new(&mut stream).poll_next(&mut cx) {
                Poll::Ready(Some(chunk)) => out.extend_from_slice(&chunk),
                Poll::Ready(None) => break,
                Poll::Pending => panic!("unexpected pending stream"),
            }
        }

        out
    }

    #[test]
    fn write_full_sets_content_length() {
        let response = Response::ok()
            .header("content-type", b"text/plain".to_vec())
            .body(ResponseBody::Bytes(b"hello".to_vec()));
        let mut writer = ResponseWriter::new();
        let bytes = match writer.write(response) {
            ResponseWrite::Full(bytes) => bytes,
            ResponseWrite::Stream(_) => panic!("expected full response"),
        };
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("content-length: 5\r\n"));
        assert!(text.contains("\r\n\r\nhello"));
    }

    #[test]
    fn write_stream_uses_chunked_encoding() {
        let stream = iter(vec![b"hello".to_vec(), b"world".to_vec()]);
        let response = Response::ok()
            .header("content-type", b"text/plain".to_vec())
            .body(ResponseBody::stream(stream));
        let mut writer = ResponseWriter::new();
        let bytes = match writer.write(response) {
            ResponseWrite::Stream(stream) => collect_stream(stream),
            ResponseWrite::Full(_) => panic!("expected stream response"),
        };

        let expected = b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n";
        assert_eq!(bytes, expected);
    }
}
