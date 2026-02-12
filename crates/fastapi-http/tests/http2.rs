use asupersync::runtime::RuntimeBuilder;
use fastapi_core::{App, Request, RequestContext, Response, ResponseBody};
use fastapi_http::{ServerConfig, TcpServer};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::{Arc, mpsc};
use std::time::Duration;

const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

fn write_frame(stream: &mut TcpStream, frame_type: u8, flags: u8, stream_id: u32, payload: &[u8]) {
    assert!(stream_id & 0x8000_0000 == 0, "reserved bit must be clear");
    assert!(
        payload.len() <= 0x00FF_FFFF,
        "payload too large for test helper"
    );

    let len = u32::try_from(payload.len()).expect("payload len must fit u32");
    let mut header = [0u8; 9];
    header[0] = ((len >> 16) & 0xff) as u8;
    header[1] = ((len >> 8) & 0xff) as u8;
    header[2] = (len & 0xff) as u8;
    header[3] = frame_type;
    header[4] = flags;
    header[5..9].copy_from_slice(&stream_id.to_be_bytes());

    stream.write_all(&header).expect("write frame header");
    stream.write_all(payload).expect("write frame payload");
}

fn read_exact(stream: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    stream.read_exact(&mut out).expect("read_exact");
    out
}

fn read_frame(stream: &mut TcpStream) -> (u8, u8, u32, Vec<u8>) {
    let hdr = read_exact(stream, 9);
    let len = (u32::from(hdr[0]) << 16) | (u32::from(hdr[1]) << 8) | u32::from(hdr[2]);
    let frame_type = hdr[3];
    let flags = hdr[4];
    let stream_id = u32::from_be_bytes([hdr[5], hdr[6], hdr[7], hdr[8]]) & 0x7FFF_FFFF;
    let payload = read_exact(stream, len as usize);
    (frame_type, flags, stream_id, payload)
}

fn spawn_server(app: App) -> (Arc<TcpServer>, SocketAddr, std::thread::JoinHandle<()>) {
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

    (server, addr, server_thread)
}

fn spawn_server_handler(
    handler: &Arc<dyn fastapi_core::Handler>,
) -> (Arc<TcpServer>, SocketAddr, std::thread::JoinHandle<()>) {
    let server = Arc::new(TcpServer::new(ServerConfig::new("127.0.0.1:0")));
    let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();

    let server_thread = {
        let server = Arc::clone(&server);
        let handler = Arc::clone(handler);
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

                let _ = server.serve_on_handler(&cx, listener, handler).await;
            });
        })
    };

    let addr = addr_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server must report addr");

    (server, addr, server_thread)
}

fn spawn_server_closure() -> (Arc<TcpServer>, SocketAddr, std::thread::JoinHandle<()>) {
    let server = Arc::new(TcpServer::new(ServerConfig::new("127.0.0.1:0")));
    let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();

    let server_thread = {
        let server = Arc::clone(&server);
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

                let _ = server
                    .serve_on(
                        &cx,
                        listener,
                        |_ctx: RequestContext, _req: &mut Request| async move {
                            Response::ok().body(ResponseBody::Bytes(b"closure-path-ok".to_vec()))
                        },
                    )
                    .await;
            });
        })
    };

    let addr = addr_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server must report addr");

    (server, addr, server_thread)
}

fn read_settings_handshake(stream: &mut TcpStream) {
    let mut saw_settings = false;
    let mut saw_ack = false;
    for _ in 0..8 {
        let (ty, flags, sid, _payload) = read_frame(stream);
        if ty == 0x4 && sid == 0 && (flags & 0x1) == 0 {
            saw_settings = true;
        } else if ty == 0x4 && sid == 0 && (flags & 0x1) != 0 {
            saw_ack = true;
        }
        if saw_settings && saw_ack {
            break;
        }
    }
    assert!(saw_settings, "expected server SETTINGS");
    assert!(saw_ack, "expected server SETTINGS ack for client SETTINGS");
}

fn read_header_block(stream: &mut TcpStream, stream_id: u32) -> Vec<u8> {
    let mut block = Vec::new();
    loop {
        let (ty, flags, sid, payload) = read_frame(stream);
        if sid != stream_id {
            continue;
        }
        if ty == 0x1 || ty == 0x9 {
            block.extend_from_slice(&payload);
            if (flags & 0x4) != 0 {
                break;
            }
        }
    }
    block
}

fn read_data_body(stream: &mut TcpStream, stream_id: u32) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let (ty, flags, sid, payload) = read_frame(stream);
        if sid != stream_id {
            continue;
        }
        if ty != 0x0 {
            continue;
        }
        body.extend_from_slice(&payload);
        if (flags & 0x1) != 0 {
            break;
        }
    }
    body
}

fn window_update_payload(increment: u32) -> [u8; 4] {
    assert!(
        (1..=0x7FFF_FFFF).contains(&increment),
        "WINDOW_UPDATE increment must be 1..=2^31-1"
    );
    increment.to_be_bytes()
}

fn rst_stream_payload(error_code: u32) -> [u8; 4] {
    error_code.to_be_bytes()
}

fn priority_payload(dependency_stream_id: u32, weight: u8) -> [u8; 5] {
    assert!(
        dependency_stream_id & 0x8000_0000 == 0,
        "priority dependency reserved bit must be clear"
    );
    let mut payload = [0u8; 5];
    payload[..4].copy_from_slice(&dependency_stream_id.to_be_bytes());
    payload[4] = weight;
    payload
}

fn assert_connection_closed(stream: &mut TcpStream) {
    let mut probe = [0u8; 1];
    let n = stream
        .read(&mut probe)
        .expect("read after protocol violation");
    assert_eq!(n, 0, "expected peer connection close");
}

#[test]
fn http2_h2c_prior_knowledge_get_root() {
    let app = App::builder()
        .get(
            "/",
            |_ctx: &RequestContext, _req: &mut Request| async move {
                Response::ok().body(ResponseBody::Bytes(b"hello".to_vec()))
            },
        )
        .build();

    let (server, addr, server_thread) = spawn_server(app);

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    // Client preface + SETTINGS.
    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]); // SETTINGS

    // Read server SETTINGS and server ACK for our SETTINGS (order may vary).
    read_settings_handshake(&mut stream);

    // ACK server SETTINGS.
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    // HEADERS for GET / using RFC 7541 C.2.1 "First Request" header block.
    // :method=GET, :scheme=http, :path=/, :authority=www.example.com
    let header_block: [u8; 17] = [
        0x82, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x5, 1, &header_block); // HEADERS | END_STREAM | END_HEADERS

    // Read response HEADERS (+ optional CONTINUATION) then DATA until END_STREAM.
    let resp_header_block = read_header_block(&mut stream, 1);

    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200, got: {decoded:?}"
    );

    let body = read_data_body(&mut stream, 1);
    assert_eq!(body, b"hello");

    let _ = stream.shutdown(Shutdown::Both);

    // Stop the server and wake accept() with a dummy connection.
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_handler_path_allows_interleaved_ping_while_reading_body() {
    let app = App::builder()
        .post(
            "/",
            |_ctx: &RequestContext, _req: &mut Request| async move {
                Response::ok().body(ResponseBody::Bytes(b"handler-path-ok".to_vec()))
            },
        )
        .build();

    let handler: Arc<dyn fastapi_core::Handler> = Arc::new(app);
    let (server, addr, server_thread) = spawn_server_handler(&handler);

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);

    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    // :method=POST, :scheme=http, :path=/, :authority=www.example.com
    let header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &header_block); // HEADERS | END_HEADERS (no END_STREAM)

    let ping_payload = b"pingpong";
    write_frame(&mut stream, 0x6, 0x0, 0, ping_payload); // PING
    write_frame(&mut stream, 0x0, 0x1, 1, b"abc"); // DATA | END_STREAM

    let (ty, flags, sid, payload) = read_frame(&mut stream);
    assert_eq!(ty, 0x6, "expected PING ack before response frames");
    assert_eq!(flags & 0x1, 0x1, "PING ack flag must be set");
    assert_eq!(sid, 0, "PING must be on stream 0");
    assert_eq!(payload, ping_payload);

    let resp_header_block = read_header_block(&mut stream, 1);
    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200, got: {decoded:?}"
    );

    let body = read_data_body(&mut stream, 1);
    assert_eq!(body, b"handler-path-ok");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_closure_path_allows_interleaved_ping_while_reading_body() {
    let (server, addr, server_thread) = spawn_server_closure();

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);

    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    // :method=POST, :scheme=http, :path=/, :authority=www.example.com
    let header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &header_block); // HEADERS | END_HEADERS (no END_STREAM)

    let ping_payload = b"pingpong";
    write_frame(&mut stream, 0x6, 0x0, 0, ping_payload); // PING
    write_frame(&mut stream, 0x0, 0x1, 1, b"abc"); // DATA | END_STREAM

    let (ty, flags, sid, payload) = read_frame(&mut stream);
    assert_eq!(ty, 0x6, "expected PING ack before response frames");
    assert_eq!(flags & 0x1, 0x1, "PING ack flag must be set");
    assert_eq!(sid, 0, "PING must be on stream 0");
    assert_eq!(payload, ping_payload);

    let resp_header_block = read_header_block(&mut stream, 1);
    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200, got: {decoded:?}"
    );

    let body = read_data_body(&mut stream, 1);
    assert_eq!(body, b"closure-path-ok");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_app_path_allows_interleaved_window_update_while_reading_body() {
    let app = App::builder()
        .post(
            "/",
            |_ctx: &RequestContext, _req: &mut Request| async move {
                Response::ok().body(ResponseBody::Bytes(b"app-path-ok".to_vec()))
            },
        )
        .build();

    let (server, addr, server_thread) = spawn_server(app);

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);

    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    // :method=POST, :scheme=http, :path=/, :authority=www.example.com
    let header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &header_block); // HEADERS | END_HEADERS (no END_STREAM)

    let wu = window_update_payload(1024);
    write_frame(&mut stream, 0x8, 0x0, 0, &wu); // WINDOW_UPDATE
    write_frame(&mut stream, 0x0, 0x1, 1, b"abc"); // DATA | END_STREAM

    let resp_header_block = read_header_block(&mut stream, 1);
    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200, got: {decoded:?}"
    );

    let body = read_data_body(&mut stream, 1);
    assert_eq!(body, b"app-path-ok");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_handler_path_allows_interleaved_window_update_while_reading_body() {
    let app = App::builder()
        .post(
            "/",
            |_ctx: &RequestContext, _req: &mut Request| async move {
                Response::ok().body(ResponseBody::Bytes(b"handler-path-ok".to_vec()))
            },
        )
        .build();

    let handler: Arc<dyn fastapi_core::Handler> = Arc::new(app);
    let (server, addr, server_thread) = spawn_server_handler(&handler);

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);

    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    // :method=POST, :scheme=http, :path=/, :authority=www.example.com
    let header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &header_block); // HEADERS | END_HEADERS (no END_STREAM)

    let wu = window_update_payload(1024);
    write_frame(&mut stream, 0x8, 0x0, 0, &wu); // WINDOW_UPDATE
    write_frame(&mut stream, 0x0, 0x1, 1, b"abc"); // DATA | END_STREAM

    let resp_header_block = read_header_block(&mut stream, 1);
    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200, got: {decoded:?}"
    );

    let body = read_data_body(&mut stream, 1);
    assert_eq!(body, b"handler-path-ok");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_closure_path_allows_interleaved_window_update_while_reading_body() {
    let (server, addr, server_thread) = spawn_server_closure();

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);

    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    // :method=POST, :scheme=http, :path=/, :authority=www.example.com
    let header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &header_block); // HEADERS | END_HEADERS (no END_STREAM)

    let wu = window_update_payload(1024);
    write_frame(&mut stream, 0x8, 0x0, 0, &wu); // WINDOW_UPDATE
    write_frame(&mut stream, 0x0, 0x1, 1, b"abc"); // DATA | END_STREAM

    let resp_header_block = read_header_block(&mut stream, 1);
    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200, got: {decoded:?}"
    );

    let body = read_data_body(&mut stream, 1);
    assert_eq!(body, b"closure-path-ok");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_app_path_allows_interleaved_priority_while_reading_body() {
    let app = App::builder()
        .post(
            "/",
            |_ctx: &RequestContext, _req: &mut Request| async move {
                Response::ok().body(ResponseBody::Bytes(b"app-path-ok".to_vec()))
            },
        )
        .build();

    let (server, addr, server_thread) = spawn_server(app);

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);

    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    // :method=POST, :scheme=http, :path=/, :authority=www.example.com
    let header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &header_block); // HEADERS | END_HEADERS (no END_STREAM)

    let prio = priority_payload(0, 16);
    write_frame(&mut stream, 0x2, 0x0, 1, &prio); // PRIORITY
    write_frame(&mut stream, 0x0, 0x1, 1, b"abc"); // DATA | END_STREAM

    let resp_header_block = read_header_block(&mut stream, 1);
    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200, got: {decoded:?}"
    );

    let body = read_data_body(&mut stream, 1);
    assert_eq!(body, b"app-path-ok");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_handler_path_allows_interleaved_priority_while_reading_body() {
    let app = App::builder()
        .post(
            "/",
            |_ctx: &RequestContext, _req: &mut Request| async move {
                Response::ok().body(ResponseBody::Bytes(b"handler-path-ok".to_vec()))
            },
        )
        .build();

    let handler: Arc<dyn fastapi_core::Handler> = Arc::new(app);
    let (server, addr, server_thread) = spawn_server_handler(&handler);

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);

    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    // :method=POST, :scheme=http, :path=/, :authority=www.example.com
    let header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &header_block); // HEADERS | END_HEADERS (no END_STREAM)

    let prio = priority_payload(0, 16);
    write_frame(&mut stream, 0x2, 0x0, 1, &prio); // PRIORITY
    write_frame(&mut stream, 0x0, 0x1, 1, b"abc"); // DATA | END_STREAM

    let resp_header_block = read_header_block(&mut stream, 1);
    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200, got: {decoded:?}"
    );

    let body = read_data_body(&mut stream, 1);
    assert_eq!(body, b"handler-path-ok");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_closure_path_allows_interleaved_priority_while_reading_body() {
    let (server, addr, server_thread) = spawn_server_closure();

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);

    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    // :method=POST, :scheme=http, :path=/, :authority=www.example.com
    let header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &header_block); // HEADERS | END_HEADERS (no END_STREAM)

    let prio = priority_payload(0, 16);
    write_frame(&mut stream, 0x2, 0x0, 1, &prio); // PRIORITY
    write_frame(&mut stream, 0x0, 0x1, 1, b"abc"); // DATA | END_STREAM

    let resp_header_block = read_header_block(&mut stream, 1);
    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200, got: {decoded:?}"
    );

    let body = read_data_body(&mut stream, 1);
    assert_eq!(body, b"closure-path-ok");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_app_path_rejects_non_empty_settings_ack_while_reading_body() {
    let app = App::builder()
        .post(
            "/",
            |_ctx: &RequestContext, _req: &mut Request| async move {
                Response::ok().body(ResponseBody::Bytes(b"app-path-ok".to_vec()))
            },
        )
        .build();

    let (server, addr, server_thread) = spawn_server(app);

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);
    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    let post_header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &post_header_block); // HEADERS | END_HEADERS
    write_frame(&mut stream, 0x4, 0x1, 0, &[0, 0, 0, 0, 0, 0]); // invalid SETTINGS ACK payload

    assert_connection_closed(&mut stream);

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_handler_path_rejects_non_empty_settings_ack_while_reading_body() {
    let app = App::builder()
        .post(
            "/",
            |_ctx: &RequestContext, _req: &mut Request| async move {
                Response::ok().body(ResponseBody::Bytes(b"handler-path-ok".to_vec()))
            },
        )
        .build();

    let handler: Arc<dyn fastapi_core::Handler> = Arc::new(app);
    let (server, addr, server_thread) = spawn_server_handler(&handler);

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);
    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    let post_header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &post_header_block); // HEADERS | END_HEADERS
    write_frame(&mut stream, 0x4, 0x1, 0, &[0, 0, 0, 0, 0, 0]); // invalid SETTINGS ACK payload

    assert_connection_closed(&mut stream);

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_closure_path_rejects_non_empty_settings_ack_while_reading_body() {
    let (server, addr, server_thread) = spawn_server_closure();

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);
    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    let post_header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &post_header_block); // HEADERS | END_HEADERS
    write_frame(&mut stream, 0x4, 0x1, 0, &[0, 0, 0, 0, 0, 0]); // invalid SETTINGS ACK payload

    assert_connection_closed(&mut stream);

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_app_path_allows_interleaved_rst_stream_and_continues_next_stream() {
    let app = App::builder()
        .get(
            "/",
            |_ctx: &RequestContext, _req: &mut Request| async move {
                Response::ok().body(ResponseBody::Bytes(b"app-path-ok".to_vec()))
            },
        )
        .build();

    let (server, addr, server_thread) = spawn_server(app);

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);

    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    let post_header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &post_header_block); // HEADERS | END_HEADERS

    let rst = rst_stream_payload(0x8); // CANCEL
    write_frame(&mut stream, 0x3, 0x0, 1, &rst); // RST_STREAM stream=1

    let get_header_block: [u8; 17] = [
        0x82, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x5, 3, &get_header_block); // HEADERS | END_STREAM | END_HEADERS

    let resp_header_block = read_header_block(&mut stream, 3);
    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200 on stream 3, got: {decoded:?}"
    );
    let body = read_data_body(&mut stream, 3);
    assert_eq!(body, b"app-path-ok");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_handler_path_allows_interleaved_rst_stream_and_continues_next_stream() {
    let app = App::builder()
        .get(
            "/",
            |_ctx: &RequestContext, _req: &mut Request| async move {
                Response::ok().body(ResponseBody::Bytes(b"handler-path-ok".to_vec()))
            },
        )
        .build();

    let handler: Arc<dyn fastapi_core::Handler> = Arc::new(app);
    let (server, addr, server_thread) = spawn_server_handler(&handler);

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);

    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    let post_header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &post_header_block); // HEADERS | END_HEADERS

    let rst = rst_stream_payload(0x8); // CANCEL
    write_frame(&mut stream, 0x3, 0x0, 1, &rst); // RST_STREAM stream=1

    let get_header_block: [u8; 17] = [
        0x82, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x5, 3, &get_header_block); // HEADERS | END_STREAM | END_HEADERS

    let resp_header_block = read_header_block(&mut stream, 3);
    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200 on stream 3, got: {decoded:?}"
    );
    let body = read_data_body(&mut stream, 3);
    assert_eq!(body, b"handler-path-ok");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}

#[test]
fn http2_closure_path_allows_interleaved_rst_stream_and_continues_next_stream() {
    let (server, addr, server_thread) = spawn_server_closure();

    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set write timeout");

    stream.write_all(PREFACE).expect("write preface");
    write_frame(&mut stream, 0x4, 0x0, 0, &[]);

    read_settings_handshake(&mut stream);
    write_frame(&mut stream, 0x4, 0x1, 0, &[]);

    let post_header_block: [u8; 17] = [
        0x83, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x4, 1, &post_header_block); // HEADERS | END_HEADERS

    let rst = rst_stream_payload(0x8); // CANCEL
    write_frame(&mut stream, 0x3, 0x0, 1, &rst); // RST_STREAM stream=1

    let get_header_block: [u8; 17] = [
        0x82, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90,
        0xf4, 0xff,
    ];
    write_frame(&mut stream, 0x1, 0x5, 3, &get_header_block); // HEADERS | END_STREAM | END_HEADERS

    let resp_header_block = read_header_block(&mut stream, 3);
    let mut dec = fastapi_http::http2::HpackDecoder::new();
    let decoded = dec
        .decode(&resp_header_block)
        .expect("decode response headers");
    assert!(
        decoded.contains(&(b":status".to_vec(), b"200".to_vec())),
        "expected :status 200 on stream 3, got: {decoded:?}"
    );
    let body = read_data_body(&mut stream, 3);
    assert_eq!(body, b"closure-path-ok");

    let _ = stream.shutdown(Shutdown::Both);
    server.shutdown();
    drop(TcpStream::connect(addr));
    server_thread.join().expect("server thread join");
}
