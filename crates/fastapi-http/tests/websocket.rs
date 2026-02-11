use asupersync::runtime::RuntimeBuilder;
use fastapi_core::{App, Request, RequestContext, WebSocket, WebSocketError};
use fastapi_http::{ServerConfig, TcpServer};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::{Arc, mpsc};
use std::time::Duration;

fn read_until_double_crlf(stream: &mut TcpStream, limit: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    while buf.len() < limit {
        let n = stream.read(&mut tmp).expect("read must succeed");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    buf
}

fn ws_masked_frame_with_fin(fin: bool, opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    assert!(
        payload.len() <= 125,
        "test helper only supports small payloads"
    );
    let mut out = Vec::with_capacity(2 + 4 + payload.len());
    let fin_bit = if fin { 0x80 } else { 0x00 };
    out.push(fin_bit | (opcode & 0x0f));
    let len_u8 = u8::try_from(payload.len()).expect("payload len must fit u8");
    out.push(0x80 | len_u8); // MASK=1
    out.extend_from_slice(&mask);
    for (i, &b) in payload.iter().enumerate() {
        out.push(b ^ mask[i & 3]);
    }
    out
}

fn ws_masked_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    ws_masked_frame_with_fin(true, opcode, payload, mask)
}

fn ws_masked_frame_with_first_byte(first_byte: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    assert!(
        payload.len() <= 125,
        "test helper only supports small payloads"
    );
    let mut out = Vec::with_capacity(2 + 4 + payload.len());
    let len_u8 = u8::try_from(payload.len()).expect("payload len must fit u8");
    out.push(first_byte);
    out.push(0x80 | len_u8);
    out.extend_from_slice(&mask);
    for (i, &b) in payload.iter().enumerate() {
        out.push(b ^ mask[i & 3]);
    }
    out
}

fn ws_read_unmasked_frame(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).expect("read header");
    let b0 = header[0];
    let b1 = header[1];

    let opcode = b0 & 0x0f;
    let masked = (b1 & 0x80) != 0;
    assert!(!masked, "server->client frames must not be masked");

    let mut len = u64::from(b1 & 0x7f);
    if len == 126 {
        let mut ext = [0u8; 2];
        stream.read_exact(&mut ext).expect("read ext16");
        len = u64::from(u16::from_be_bytes(ext));
    } else if len == 127 {
        let mut ext = [0u8; 8];
        stream.read_exact(&mut ext).expect("read ext64");
        len = u64::from_be_bytes(ext);
    }

    let len = usize::try_from(len).expect("len fits usize for test");
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).expect("read payload");
    (opcode, payload)
}

#[test]
fn websocket_upgrade_and_echo_text() {
    let app = App::builder()
        .websocket(
            "/ws",
            |_ctx: &RequestContext, _req: &mut Request, mut ws: WebSocket| async move {
                let msg = ws.read_text().await?;
                ws.send_text(&msg).await?;
                Ok::<(), WebSocketError>(())
            },
        )
        .build();

    let server = Arc::new(TcpServer::new(ServerConfig::new("127.0.0.1:0")));
    let app = Arc::new(app);
    let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();

    let server_thread = {
        let server = Arc::clone(&server);
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            let rt = RuntimeBuilder::current_thread()
                .build()
                .expect("test runtime must build");
            rt.block_on(async move {
                let cx = asupersync::Cx::for_testing();
                let listener = asupersync::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind must succeed");
                let local_addr = listener.local_addr().expect("local_addr must work");
                addr_tx.send(local_addr).expect("addr send must succeed");

                let _ = server.serve_on_app(&cx, listener, app).await;
            });
        })
    };

    let addr = addr_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server must report addr");

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let accept_expected = fastapi_core::websocket_accept_from_key(key).expect("accept compute");

    let req = format!(
        "GET /ws HTTP/1.1\r\n\
Host: {addr}\r\n\
Upgrade: websocket\r\n\
Connection: Upgrade\r\n\
Sec-WebSocket-Version: 13\r\n\
Sec-WebSocket-Key: {key}\r\n\
\r\n"
    );
    stream.write_all(req.as_bytes()).expect("write handshake");

    let resp = read_until_double_crlf(&mut stream, 16 * 1024);
    let resp_str = std::str::from_utf8(&resp).expect("utf8 response");
    assert!(
        resp_str.starts_with("HTTP/1.1 101"),
        "expected 101 switching protocols, got:\n{resp_str}"
    );
    assert!(
        resp_str.to_ascii_lowercase().contains("upgrade: websocket"),
        "missing upgrade header:\n{resp_str}"
    );
    assert!(
        resp_str
            .to_ascii_lowercase()
            .contains("connection: upgrade"),
        "missing connection header:\n{resp_str}"
    );
    assert!(
        resp_str
            .to_ascii_lowercase()
            .contains(&format!("sec-websocket-accept: {accept_expected}").to_ascii_lowercase()),
        "missing/incorrect accept header:\n{resp_str}"
    );

    // Send a masked ping (expect pong), then a masked text frame (expect echo).
    let ping = ws_masked_frame(0x9, b"abc", [0x01, 0x02, 0x03, 0x04]);
    stream.write_all(&ping).expect("write ping");
    let msg = ws_masked_frame(0x1, b"hello", [0x05, 0x06, 0x07, 0x08]);
    stream.write_all(&msg).expect("write ws frame");

    let (opcode, payload) = ws_read_unmasked_frame(&mut stream);
    assert_eq!(opcode, 0xA, "expected pong opcode");
    assert_eq!(&payload, b"abc");

    let (opcode, payload) = ws_read_unmasked_frame(&mut stream);
    assert_eq!(opcode, 0x1, "expected text opcode");
    assert_eq!(&payload, b"hello");

    // Close client.
    let _ = stream.shutdown(Shutdown::Both);

    // Stop the server and wake accept() with a dummy connection (then immediate close).
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn websocket_fragmented_text_with_interleaved_ping_echoes_full_message() {
    let app = App::builder()
        .websocket(
            "/ws",
            |_ctx: &RequestContext, _req: &mut Request, mut ws: WebSocket| async move {
                let msg = ws.read_text().await?;
                ws.send_text(&msg).await?;
                Ok::<(), WebSocketError>(())
            },
        )
        .build();

    let server = Arc::new(TcpServer::new(ServerConfig::new("127.0.0.1:0")));
    let app = Arc::new(app);
    let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();

    let server_thread = {
        let server = Arc::clone(&server);
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            let rt = RuntimeBuilder::current_thread()
                .build()
                .expect("test runtime must build");
            rt.block_on(async move {
                let cx = asupersync::Cx::for_testing();
                let listener = asupersync::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind must succeed");
                let local_addr = listener.local_addr().expect("local_addr must work");
                addr_tx.send(local_addr).expect("addr send must succeed");

                let _ = server.serve_on_app(&cx, listener, app).await;
            });
        })
    };

    let addr = addr_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server must report addr");

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let accept_expected = fastapi_core::websocket_accept_from_key(key).expect("accept compute");

    let req = format!(
        "GET /ws HTTP/1.1\r\n\
Host: {addr}\r\n\
Upgrade: websocket\r\n\
Connection: Upgrade\r\n\
Sec-WebSocket-Version: 13\r\n\
Sec-WebSocket-Key: {key}\r\n\
\r\n"
    );
    stream.write_all(req.as_bytes()).expect("write handshake");

    let resp = read_until_double_crlf(&mut stream, 16 * 1024);
    let resp_str = std::str::from_utf8(&resp).expect("utf8 response");
    assert!(
        resp_str.starts_with("HTTP/1.1 101"),
        "expected 101 switching protocols, got:\n{resp_str}"
    );
    assert!(
        resp_str
            .to_ascii_lowercase()
            .contains(&format!("sec-websocket-accept: {accept_expected}").to_ascii_lowercase()),
        "missing/incorrect accept header:\n{resp_str}"
    );

    // Send fragmented text message with interleaved ping.
    // 1) TEXT(fin=0,"he"), 2) PING("x"), 3) CONT(fin=1,"llo")
    let text_start = ws_masked_frame_with_fin(false, 0x1, b"he", [0x11, 0x12, 0x13, 0x14]);
    stream
        .write_all(&text_start)
        .expect("write initial fragment");

    let ping = ws_masked_frame(0x9, b"x", [0x21, 0x22, 0x23, 0x24]);
    stream.write_all(&ping).expect("write ping");

    let text_cont = ws_masked_frame_with_fin(true, 0x0, b"llo", [0x31, 0x32, 0x33, 0x34]);
    stream
        .write_all(&text_cont)
        .expect("write continuation fragment");

    let (opcode, payload) = ws_read_unmasked_frame(&mut stream);
    assert_eq!(opcode, 0xA, "expected pong opcode");
    assert_eq!(&payload, b"x");

    let (opcode, payload) = ws_read_unmasked_frame(&mut stream);
    assert_eq!(opcode, 0x1, "expected text opcode");
    assert_eq!(&payload, b"hello");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn websocket_rejects_frames_with_reserved_bits_set() {
    let app = App::builder()
        .websocket(
            "/ws",
            |_ctx: &RequestContext, _req: &mut Request, mut ws: WebSocket| async move {
                let msg = ws.read_text().await?;
                ws.send_text(&msg).await?;
                Ok::<(), WebSocketError>(())
            },
        )
        .build();

    let server = Arc::new(TcpServer::new(ServerConfig::new("127.0.0.1:0")));
    let app = Arc::new(app);
    let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();

    let server_thread = {
        let server = Arc::clone(&server);
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            let rt = RuntimeBuilder::current_thread()
                .build()
                .expect("test runtime must build");
            rt.block_on(async move {
                let cx = asupersync::Cx::for_testing();
                let listener = asupersync::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind must succeed");
                let local_addr = listener.local_addr().expect("local_addr must work");
                addr_tx.send(local_addr).expect("addr send must succeed");

                let _ = server.serve_on_app(&cx, listener, app).await;
            });
        })
    };

    let addr = addr_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server must report addr");

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let req = format!(
        "GET /ws HTTP/1.1\r\n\
Host: {addr}\r\n\
Upgrade: websocket\r\n\
Connection: Upgrade\r\n\
Sec-WebSocket-Version: 13\r\n\
Sec-WebSocket-Key: {key}\r\n\
\r\n"
    );
    stream.write_all(req.as_bytes()).expect("write handshake");
    let resp = read_until_double_crlf(&mut stream, 16 * 1024);
    let resp_str = std::str::from_utf8(&resp).expect("utf8 response");
    assert!(
        resp_str.starts_with("HTTP/1.1 101"),
        "expected 101 switching protocols, got:\n{resp_str}"
    );

    // RSV1 set without extensions negotiated is a protocol violation.
    let invalid = ws_masked_frame_with_first_byte(0xC1, b"boom", [0x0A, 0x0B, 0x0C, 0x0D]);
    stream.write_all(&invalid).expect("write invalid frame");

    // The server should answer malformed protocol frames with CLOSE 1002.
    let (opcode, payload) = ws_read_unmasked_frame(&mut stream);
    assert_eq!(opcode, 0x8, "expected close opcode");
    assert_eq!(
        payload,
        1002u16.to_be_bytes().to_vec(),
        "expected protocol-error close code (1002)"
    );

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn websocket_echoes_empty_close_frame_payload() {
    let app = App::builder()
        .websocket(
            "/ws",
            |_ctx: &RequestContext, _req: &mut Request, mut ws: WebSocket| async move {
                let _ = ws.read_text_or_close().await?;
                Ok::<(), WebSocketError>(())
            },
        )
        .build();

    let server = Arc::new(TcpServer::new(ServerConfig::new("127.0.0.1:0")));
    let app = Arc::new(app);
    let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();

    let server_thread = {
        let server = Arc::clone(&server);
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            let rt = RuntimeBuilder::current_thread()
                .build()
                .expect("test runtime must build");
            rt.block_on(async move {
                let cx = asupersync::Cx::for_testing();
                let listener = asupersync::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind must succeed");
                let local_addr = listener.local_addr().expect("local_addr must work");
                addr_tx.send(local_addr).expect("addr send must succeed");

                let _ = server.serve_on_app(&cx, listener, app).await;
            });
        })
    };

    let addr = addr_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server must report addr");

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let req = format!(
        "GET /ws HTTP/1.1\r\n\
Host: {addr}\r\n\
Upgrade: websocket\r\n\
Connection: Upgrade\r\n\
Sec-WebSocket-Version: 13\r\n\
Sec-WebSocket-Key: {key}\r\n\
\r\n"
    );
    stream.write_all(req.as_bytes()).expect("write handshake");

    let resp = read_until_double_crlf(&mut stream, 16 * 1024);
    let resp_str = std::str::from_utf8(&resp).expect("utf8 response");
    assert!(
        resp_str.starts_with("HTTP/1.1 101"),
        "expected 101 switching protocols, got:\n{resp_str}"
    );

    // Send masked CLOSE with empty payload. Server should echo CLOSE with empty payload.
    let close = ws_masked_frame(0x8, b"", [0x09, 0x08, 0x07, 0x06]);
    stream.write_all(&close).expect("write close frame");

    let (opcode, payload) = ws_read_unmasked_frame(&mut stream);
    assert_eq!(opcode, 0x8, "expected close opcode");
    assert!(
        payload.is_empty(),
        "expected echoed close payload to be empty"
    );

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn websocket_invalid_close_payload_len_one_gets_protocol_error_close() {
    let app = App::builder()
        .websocket(
            "/ws",
            |_ctx: &RequestContext, _req: &mut Request, mut ws: WebSocket| async move {
                let _ = ws.read_text_or_close().await?;
                Ok::<(), WebSocketError>(())
            },
        )
        .build();

    let server = Arc::new(TcpServer::new(ServerConfig::new("127.0.0.1:0")));
    let app = Arc::new(app);
    let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();

    let server_thread = {
        let server = Arc::clone(&server);
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            let rt = RuntimeBuilder::current_thread()
                .build()
                .expect("test runtime must build");
            rt.block_on(async move {
                let cx = asupersync::Cx::for_testing();
                let listener = asupersync::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind must succeed");
                let local_addr = listener.local_addr().expect("local_addr must work");
                addr_tx.send(local_addr).expect("addr send must succeed");

                let _ = server.serve_on_app(&cx, listener, app).await;
            });
        })
    };

    let addr = addr_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server must report addr");

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let req = format!(
        "GET /ws HTTP/1.1\r\n\
Host: {addr}\r\n\
Upgrade: websocket\r\n\
Connection: Upgrade\r\n\
Sec-WebSocket-Version: 13\r\n\
Sec-WebSocket-Key: {key}\r\n\
\r\n"
    );
    stream.write_all(req.as_bytes()).expect("write handshake");

    let resp = read_until_double_crlf(&mut stream, 16 * 1024);
    let resp_str = std::str::from_utf8(&resp).expect("utf8 response");
    assert!(
        resp_str.starts_with("HTTP/1.1 101"),
        "expected 101 switching protocols, got:\n{resp_str}"
    );

    // Invalid CLOSE payload length (=1 byte) must trigger protocol close (1002).
    let invalid_close = ws_masked_frame(0x8, &[0x03], [0x04, 0x03, 0x02, 0x01]);
    stream
        .write_all(&invalid_close)
        .expect("write invalid close frame");

    let (opcode, payload) = ws_read_unmasked_frame(&mut stream);
    assert_eq!(opcode, 0x8, "expected close opcode");
    assert_eq!(
        payload,
        1002u16.to_be_bytes().to_vec(),
        "expected protocol-error close code (1002)"
    );

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn websocket_invalid_close_code_gets_protocol_error_close() {
    let app = App::builder()
        .websocket(
            "/ws",
            |_ctx: &RequestContext, _req: &mut Request, mut ws: WebSocket| async move {
                let _ = ws.read_text_or_close().await?;
                Ok::<(), WebSocketError>(())
            },
        )
        .build();

    let server = Arc::new(TcpServer::new(ServerConfig::new("127.0.0.1:0")));
    let app = Arc::new(app);
    let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();

    let server_thread = {
        let server = Arc::clone(&server);
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            let rt = RuntimeBuilder::current_thread()
                .build()
                .expect("test runtime must build");
            rt.block_on(async move {
                let cx = asupersync::Cx::for_testing();
                let listener = asupersync::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind must succeed");
                let local_addr = listener.local_addr().expect("local_addr must work");
                addr_tx.send(local_addr).expect("addr send must succeed");

                let _ = server.serve_on_app(&cx, listener, app).await;
            });
        })
    };

    let addr = addr_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server must report addr");

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let req = format!(
        "GET /ws HTTP/1.1\r\n\
Host: {addr}\r\n\
Upgrade: websocket\r\n\
Connection: Upgrade\r\n\
Sec-WebSocket-Version: 13\r\n\
Sec-WebSocket-Key: {key}\r\n\
\r\n"
    );
    stream.write_all(req.as_bytes()).expect("write handshake");

    let resp = read_until_double_crlf(&mut stream, 16 * 1024);
    let resp_str = std::str::from_utf8(&resp).expect("utf8 response");
    assert!(
        resp_str.starts_with("HTTP/1.1 101"),
        "expected 101 switching protocols, got:\n{resp_str}"
    );

    // Invalid CLOSE code 1006 must trigger protocol close (1002).
    let invalid_close = ws_masked_frame(0x8, &1006u16.to_be_bytes(), [0x0E, 0x0D, 0x0C, 0x0B]);
    stream
        .write_all(&invalid_close)
        .expect("write invalid close frame");

    let (opcode, payload) = ws_read_unmasked_frame(&mut stream);
    assert_eq!(opcode, 0x8, "expected close opcode");
    assert_eq!(
        payload,
        1002u16.to_be_bytes().to_vec(),
        "expected protocol-error close code (1002)"
    );

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}
