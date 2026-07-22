//! Request-timeout enforcement tests (mea-ym5r5).
//!
//! The server must answer 504 at the configured request deadline by racing the
//! handler future against the deadline — not by letting the handler run to
//! completion and substituting 504 afterwards. A handler that never completes
//! must still produce a 504 at the deadline, the abandoned request's
//! connection must close, and in-budget handlers must be untouched. The server
//! must also publish the deadline on `RequestContext` so middleware with
//! externally visible side effects can refuse to publish abandoned responses.

use asupersync::runtime::{RuntimeBuilder, reactor::create_reactor};
use asupersync::{Cx, Time};
use fastapi_core::{App, Request, RequestContext, Response, ResponseBody, StatusCode};
use fastapi_http::{ServerConfig, TcpServer};
use std::io::Read;
use std::io::Write as _;
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

fn spawn_app_server(
    app: App,
    config: ServerConfig,
) -> (Arc<TcpServer>, SocketAddr, std::thread::JoinHandle<()>) {
    let server = Arc::new(TcpServer::new(config));
    let app = Arc::new(app);
    let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();

    let server_thread = {
        let server = Arc::clone(&server);
        let app = Arc::clone(&app);
        std::thread::spawn(move || {
            let reactor = create_reactor().expect("test reactor must build");
            let rt = RuntimeBuilder::current_thread()
                .with_reactor(reactor)
                .build()
                .expect("test runtime must build");
            rt.block_on(async move {
                let cx = Cx::current().expect("test runtime must install an ambient Cx");
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

fn get_response(addr: SocketAddr, path: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(addr).expect("connect must succeed");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set_read_timeout must succeed");
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("request write must succeed");

    let mut response = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(err) => panic!("response read must not time out or fail: {err}"),
        }
    }
    response
}

/// A handler that never completes must be abandoned at the request deadline
/// and answered with 504 — under the pre-fix behavior (post-hoc elapsed
/// check after awaiting the handler to completion) this request would hang
/// until the client read timeout instead.
#[test]
fn stuck_handler_gets_504_at_deadline_and_connection_closes() {
    let app = App::builder()
        .get("/stuck", |_ctx: &RequestContext, _req: &mut Request| {
            std::future::pending::<Response>()
        })
        .build();
    let config = ServerConfig::new("127.0.0.1:0").with_request_timeout(Time::from_millis(300));
    let (_server, addr, _server_thread) = spawn_app_server(app, config);

    let started = Instant::now();
    let response = get_response(addr, "/stuck");
    let elapsed = started.elapsed();

    let head = String::from_utf8_lossy(&response);
    assert!(
        head.starts_with("HTTP/1.1 504"),
        "stuck handler must be answered with 504 at the deadline, got: {head}"
    );
    assert!(
        head.to_ascii_lowercase().contains("connection: close"),
        "an abandoned request must close its connection, got: {head}"
    );
    assert!(
        elapsed >= Duration::from_millis(200),
        "504 must come from the deadline race, not an instant failure ({elapsed:?})"
    );
}

/// An in-budget handler is delivered unchanged, and it observes the request
/// deadline via `RequestContext` (set, on the runtime clock, not yet
/// exceeded).
#[test]
fn fast_handler_unaffected_and_observes_deadline() {
    let app = App::builder()
        .get("/fast", |ctx: &RequestContext, _req: &mut Request| {
            let deadline_visible = ctx.deadline().is_some() && !ctx.deadline_exceeded();
            async move {
                let body = if deadline_visible {
                    "deadline-ok"
                } else {
                    "deadline-missing"
                };
                Response::with_status(StatusCode::OK)
                    .body(ResponseBody::Bytes(body.as_bytes().to_vec()))
            }
        })
        .build();
    let config = ServerConfig::new("127.0.0.1:0").with_request_timeout(Time::from_secs(30));
    let (_server, addr, _server_thread) = spawn_app_server(app, config);

    let response = get_response(addr, "/fast");
    let head = String::from_utf8_lossy(&response);
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "in-budget handler must be delivered unchanged, got: {head}"
    );
    assert!(
        head.contains("deadline-ok"),
        "handler must observe an unexceeded request deadline on RequestContext, got: {head}"
    );
}
