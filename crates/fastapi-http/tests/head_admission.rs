//! Request heads are admitted before their bodies are read.
//!
//! `ServerConfig::pre_body_validators` runs "after parsing headers but before
//! any body is read", and gates `Expect: 100-continue`. These tests drive the
//! three HTTP/1.1 connection loops (App, Handler and the closure-based
//! `process_connection`) over real TCP with a client that sends only the
//! head and then waits, as an `Expect: 100-continue` client does. The keep-
//! alive timeout is 10 s and every client read gives up after 5 s, so a
//! server that waits for body bytes before deciding cannot pass.
//!
//! The last tests cover `ServerConfig::request_read_timeout`: a request that
//! trickles in, each byte inside the keep-alive timeout, still has to finish
//! arriving by the deadline.

use asupersync::Cx;
use asupersync::runtime::{RuntimeBuilder, reactor::create_reactor};
use fastapi_core::{App, Request, RequestContext, Response, ResponseBody};
use fastapi_http::{ExpectHandler, FnValidator, ParseLimits, ServerConfig, TcpServer};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The declared body length the test validator admits.
const DECLARED_CAP: usize = 1024;
/// How long a test client waits for any one read.
const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug)]
enum Mode {
    App,
    Handler,
    Closure,
}

const MODES: [Mode; 3] = [Mode::App, Mode::Handler, Mode::Closure];

struct Running {
    server: Arc<TcpServer>,
    addr: SocketAddr,
    thread: JoinHandle<()>,
    calls: Arc<AtomicUsize>,
}

impl Running {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn stop(self) {
        self.server.shutdown();
        drop(TcpStream::connect(self.addr));
        self.thread.join().expect("server thread join");
    }
}

fn declared_cap(request: &Request) -> Result<(), Response> {
    let declared = request
        .headers()
        .get("content-length")
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.trim().parse::<usize>().ok());
    match declared {
        Some(length) if length > DECLARED_CAP => Err(ExpectHandler::payload_too_large(format!(
            "declared {length} bytes; max {DECLARED_CAP}"
        ))),
        _ => Ok(()),
    }
}

fn config() -> ServerConfig {
    ServerConfig::new("127.0.0.1:0")
        .with_keep_alive_timeout(Duration::from_secs(10))
        .with_allowed_hosts(["localhost"])
        .with_pre_body_validator(FnValidator::new("declared-cap", declared_cap))
}

fn ok_response() -> Response {
    Response::ok().body(ResponseBody::Bytes(b"handled".to_vec()))
}

fn spawn(mode: Mode, config: ServerConfig) -> Running {
    let calls = Arc::new(AtomicUsize::new(0));
    let server = Arc::new(TcpServer::new(config));
    let (addr_tx, addr_rx) = mpsc::channel();
    let thread_server = Arc::clone(&server);
    let thread_calls = Arc::clone(&calls);
    let thread = std::thread::spawn(move || {
        let reactor = create_reactor().expect("reactor");
        let runtime = RuntimeBuilder::current_thread()
            .with_reactor(reactor)
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let cx = Cx::current().expect("runtime Cx");
            let listener = asupersync::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            addr_tx
                .send(listener.local_addr().expect("address"))
                .expect("send address");
            let app_calls = Arc::clone(&thread_calls);
            let app = App::builder()
                .post("/upload", move |_: &RequestContext, _: &mut Request| {
                    app_calls.fetch_add(1, Ordering::SeqCst);
                    std::future::ready(ok_response())
                })
                .build();
            let _ = match mode {
                Mode::App => {
                    thread_server
                        .serve_on_app(&cx, listener, Arc::new(app))
                        .await
                }
                Mode::Handler => {
                    let handler: Arc<dyn fastapi_core::Handler> = Arc::new(app);
                    thread_server.serve_on_handler(&cx, listener, handler).await
                }
                Mode::Closure => {
                    thread_server
                        .serve_on(&cx, listener, move |_ctx, _req| {
                            thread_calls.fetch_add(1, Ordering::SeqCst);
                            std::future::ready(ok_response())
                        })
                        .await
                }
            };
        });
    });
    let addr = addr_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("server address");
    Running {
        server,
        addr,
        thread,
        calls,
    }
}

fn connect(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(CLIENT_READ_TIMEOUT))
        .expect("read timeout");
    stream
}

/// Reads until `done` holds for what arrived, EOF, or a read error (such as
/// the client timeout), and returns everything read.
fn read_until(stream: &mut TcpStream, done: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let mut received = Vec::new();
    let mut buffer = [0_u8; 4096];
    while !done(&received) {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => received.extend_from_slice(&buffer[..read]),
        }
    }
    received
}

fn read_to_end(stream: &mut TcpStream) -> String {
    String::from_utf8_lossy(&read_until(stream, |_| false)).into_owned()
}

fn head_complete(received: &[u8]) -> bool {
    received.windows(4).any(|window| window == b"\r\n\r\n")
}

/// A head declaring more than the validator admits is refused at once,
/// whether or not it asks for `100 Continue`, and never reaches a handler.
#[test]
fn declared_overflow_is_refused_before_its_body_is_sent() {
    for mode in MODES {
        let running = spawn(mode, config());
        for expect in ["", "Expect: 100-continue\r\n"] {
            let mut stream = connect(running.addr);
            let started = Instant::now();
            stream
                .write_all(
                    format!(
                        "POST /upload HTTP/1.1\r\nHost: localhost\r\n\
                         Content-Length: 1000000\r\n{expect}\r\n"
                    )
                    .as_bytes(),
                )
                .expect("head");
            let response = read_to_end(&mut stream);
            assert!(
                response.starts_with("HTTP/1.1 413 "),
                "{mode:?} {expect:?}: a declared overflow needs a 413 before its body: {response:?}"
            );
            assert!(
                !response.contains("100 Continue"),
                "{mode:?} {expect:?}: no 100 for a refused head: {response:?}"
            );
            assert!(
                response.to_ascii_lowercase().contains("connection: close"),
                "{mode:?} {expect:?}: {response:?}"
            );
            assert!(
                started.elapsed() < CLIENT_READ_TIMEOUT,
                "{mode:?} {expect:?}: the refusal must not wait for the body"
            );
        }
        assert_eq!(
            running.calls(),
            0,
            "{mode:?}: no handler for a refused head"
        );
        running.stop();
    }
}

/// A client that waits for `100 Continue` gets it before sending its body,
/// then a single final response from the handler.
#[test]
fn expect_continue_is_answered_before_the_body() {
    for mode in MODES {
        let running = spawn(mode, config());
        let mut stream = connect(running.addr);
        stream
            .write_all(
                b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\
                  Expect: 100-continue\r\nConnection: close\r\n\r\n",
            )
            .expect("head");
        let interim = read_until(&mut stream, head_complete);
        assert_eq!(
            interim, b"HTTP/1.1 100 Continue\r\n\r\n",
            "{mode:?}: 100 Continue must precede the body"
        );
        assert_eq!(running.calls(), 0, "{mode:?}: no handler before the body");
        stream.write_all(b"hello").expect("body");
        let response = read_to_end(&mut stream);
        assert!(
            response.starts_with("HTTP/1.1 200 "),
            "{mode:?}: {response:?}"
        );
        assert_eq!(
            response.matches("100 Continue").count(),
            0,
            "{mode:?}: the 100 is sent once: {response:?}"
        );
        assert!(response.ends_with("handled"), "{mode:?}: {response:?}");
        assert_eq!(running.calls(), 1, "{mode:?}");
        running.stop();
    }
}

/// An unsupported expectation and a disallowed Host are refused from the
/// head alone, without waiting for the declared body.
#[test]
fn other_head_refusals_do_not_wait_for_the_body() {
    for mode in MODES {
        let running = spawn(mode, config());
        for (head, status) in [
            (
                "POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\
                 Expect: 102-processing\r\n\r\n",
                "417 Expectation Failed",
            ),
            (
                "POST /upload HTTP/1.1\r\nHost: elsewhere.example\r\nContent-Length: 5\r\n\r\n",
                "400 Bad Request",
            ),
        ] {
            let mut stream = connect(running.addr);
            stream.write_all(head.as_bytes()).expect("head");
            let response = read_to_end(&mut stream);
            assert!(
                response.starts_with(&format!("HTTP/1.1 {status}\r\n")),
                "{mode:?}: {head:?} needs {status} from its head: {response:?}"
            );
        }
        assert_eq!(running.calls(), 0, "{mode:?}");
        running.stop();
    }
}

/// A body past the parser's request limit gets 413 rather than a bare
/// close, and the server drains the rest so the client can read it.
#[test]
fn a_body_past_the_parser_limit_gets_413() {
    for mode in MODES {
        let limits = ParseLimits {
            max_request_size: 4096,
            ..ParseLimits::default()
        };
        let running = spawn(
            mode,
            ServerConfig::new("127.0.0.1:0")
                .with_keep_alive_timeout(Duration::from_secs(10))
                .with_parse_limits(limits),
        );
        let mut stream = connect(running.addr);
        let mut request =
            b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 65536\r\n\r\n".to_vec();
        request.resize(request.len() + 65536, b'x');
        stream.write_all(&request).expect("request");
        let response = read_to_end(&mut stream);
        assert!(
            response.starts_with("HTTP/1.1 413 "),
            "{mode:?}: a request past the parser limit needs a 413: {response:?}"
        );
        assert_eq!(running.calls(), 0, "{mode:?}");
        running.stop();
    }
}

/// The request read deadline in the next tests.
const READ_DEADLINE: Duration = Duration::from_secs(1);

fn config_with_read_deadline() -> ServerConfig {
    ServerConfig::new("127.0.0.1:0")
        .with_keep_alive_timeout(Duration::from_secs(3))
        .with_request_read_timeout(READ_DEADLINE)
}

/// A body sent a byte every 200 ms, well inside the 3 s keep-alive timeout,
/// would take 20 s. The request gets 408 at the 1 s read deadline and no
/// handler runs. Without the deadline the server keeps reading and the
/// client's 5 s read timeout ends the test with no response.
#[test]
fn a_trickled_request_gets_408_at_the_read_deadline() {
    for mode in MODES {
        let running = spawn(mode, config_with_read_deadline());
        let mut stream = connect(running.addr);
        stream
            .write_all(b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\n\r\n")
            .expect("head");
        let started = Instant::now();
        let mut trickle = stream.try_clone().expect("clone");
        let writer = std::thread::spawn(move || {
            for _ in 0..100 {
                if trickle.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        let response = read_to_end(&mut stream);
        let elapsed = started.elapsed();
        drop(stream.shutdown(std::net::Shutdown::Both));
        writer.join().expect("writer join");
        assert!(
            response.starts_with("HTTP/1.1 408 Request Timeout\r\n"),
            "{mode:?}: a trickled request needs a 408: {response:?}"
        );
        assert!(
            response.to_ascii_lowercase().contains("connection: close"),
            "{mode:?}: {response:?}"
        );
        assert!(
            elapsed + Duration::from_millis(50) >= READ_DEADLINE,
            "{mode:?}: the 408 came before the deadline, after {elapsed:?}"
        );
        assert_eq!(
            running.calls(),
            0,
            "{mode:?}: no handler for a refused request"
        );
        running.stop();
    }
}

/// The deadline counts from a request's first byte, not from the last
/// request: a keep-alive connection idle for longer than the deadline still
/// serves its next request.
#[test]
fn idle_time_between_requests_does_not_count_against_the_deadline() {
    for mode in MODES {
        let running = spawn(mode, config_with_read_deadline());
        let mut stream = connect(running.addr);
        let request = b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello";
        stream.write_all(request).expect("first request");
        let first = read_until(&mut stream, |received| received.ends_with(b"handled"));
        assert!(
            first.starts_with(b"HTTP/1.1 200 "),
            "{mode:?}: {:?}",
            String::from_utf8_lossy(&first)
        );
        std::thread::sleep(READ_DEADLINE + Duration::from_millis(500));
        stream.write_all(request).expect("second request");
        let second = read_until(&mut stream, |received| received.ends_with(b"handled"));
        assert!(
            second.starts_with(b"HTTP/1.1 200 "),
            "{mode:?}: an idle connection must still serve: {:?}",
            String::from_utf8_lossy(&second)
        );
        assert_eq!(running.calls(), 2, "{mode:?}");
        running.stop();
    }
}
