//! The web transport on the loopback address: files over HTTP, game sockets over WebSocket (plain and under TLS), UDP and the
//! web on one port, and hostile peers: requests past the caps, other methods, paths that climb, heads that never end,
//! too many connections, oversized and flooding messages, text messages, quiet sockets and slow readers. Each hostile peer
//! is closed, and a pass's time stays bounded.

use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use torque_server::net::transport::{
    BindError, BindPolicy, LanRules, PolicyRefusal, RECV_BUFFER_BYTES, Recv, Transport,
    UdpTransport,
};
use torque_server::net::web::http::base64;
use torque_server::net::web::{
    MultiTransport, Tls, WEB_SOCKET_PATH, WebFiles, WebLimits, WebSetup, WebTransport, accept_key,
};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::{Message, Utf8Bytes};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const PAGE: &[u8] = b"<!doctype html><p>the game</p>";

fn policy() -> BindPolicy {
    BindPolicy::new(LOOPBACK, false, &LanRules::PLAIN).unwrap()
}

fn files() -> WebFiles {
    let mut files = WebFiles::new();
    files.insert("index.html", PAGE.to_vec());
    files.insert("game_bg.wasm", vec![7; 3 << 20]);
    files
}

fn site(
    limits: WebLimits,
    tls: Option<Tls>,
    files: Option<WebFiles>,
) -> (WebTransport, SocketAddr) {
    let mut t = WebTransport::new(policy(), WebSetup { files, tls, limits });
    t.bind(LOOPBACK, 0).unwrap();
    let addr = t.local_addr().unwrap();
    (t, addr)
}

/// Runs `client` on a thread while the transport is pumped here, and returns what it returned.
fn serve_while<T: Send + 'static>(
    t: &mut WebTransport,
    client: impl FnOnce() -> T + Send + 'static,
) -> T {
    let h = thread::spawn(client);
    let start = Instant::now();
    while !h.is_finished() {
        t.pump();
        sleep(Duration::from_millis(1));
        assert!(start.elapsed() < Duration::from_secs(30), "the client hung");
    }
    h.join().unwrap()
}

/// Pumps until `done` or `ms` pass; returns the longest pass.
fn pump_until(
    t: &mut WebTransport,
    ms: u64,
    mut done: impl FnMut(&mut WebTransport) -> bool,
) -> Duration {
    let start = Instant::now();
    let mut longest = Duration::ZERO;
    while !done(t) && start.elapsed() < Duration::from_millis(ms) {
        let pass = Instant::now();
        t.pump();
        longest = longest.max(pass.elapsed());
        sleep(Duration::from_millis(1));
    }
    longest
}

fn connect(addr: SocketAddr) -> TcpStream {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s
}

/// Reads one response: its status, its body (by Content-Length), and whether the server then closed the connection.
fn response(s: &mut impl Read) -> (u16, Vec<u8>, bool) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 65536];
    let head_end = loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return (0, buf, true),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let status: u16 = head[9..12].parse().unwrap();
    let len: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .map_or(0, |v| v.trim().parse().unwrap());
    while buf.len() < head_end + len {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let body = buf[head_end..].to_vec();
    let closed = head.contains("Connection: close");
    (status, body, closed)
}

/// Whether the server closed `s` (a read sees the end or a reset within the timeout).
fn closed(s: &mut TcpStream) -> bool {
    let mut b = [0u8; 4096];
    loop {
        match s.read(&mut b) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return false;
            }
            Err(_) => return true,
        }
    }
}

fn upgrade_request(key: &str) -> String {
    format!(
        "GET {WEB_SOCKET_PATH} HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n"
    )
}

/// A client's game socket over `stream` (the handshake made by hand).
fn ws_over<S: Read + Write>(mut stream: S) -> WebSocket<S> {
    let key = base64(&[3; 16]);
    stream.write_all(upgrade_request(&key).as_bytes()).unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(stream.read(&mut b).unwrap(), 1, "the handshake's answer");
        head.push(b[0]);
    }
    let head = String::from_utf8(head).unwrap();
    assert!(head.starts_with("HTTP/1.1 101 "), "{head}");
    assert!(head.contains(&format!("Sec-WebSocket-Accept: {}", accept_key(&key))));
    WebSocket::from_raw_socket(stream, Role::Client, None)
}

fn ws_open(addr: SocketAddr) -> WebSocket<TcpStream> {
    ws_over(connect(addr))
}

fn recv(t: &mut impl Transport) -> Option<(SocketAddr, Vec<u8>)> {
    let mut buf = vec![0u8; RECV_BUFFER_BYTES];
    match t.recv(&mut buf) {
        Recv::Datagram { len, from } => Some((from, buf[..len].to_vec())),
        _ => None,
    }
}

#[test]
fn files_are_served_and_a_connection_is_kept_for_the_next_request() {
    let (mut t, addr) = site(WebLimits::default(), None, Some(files()));
    let (page, wasm, missing) = serve_while(&mut t, move || {
        let mut s = connect(addr);
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let page = response(&mut s);
        s.write_all(b"GET /game_bg.wasm HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let wasm = response(&mut s);
        s.write_all(b"GET /nothing HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let missing = response(&mut s);
        (page, wasm, missing)
    });
    assert_eq!((page.0, page.1.as_slice(), page.2), (200, PAGE, false));
    assert_eq!((wasm.0, wasm.1.len(), wasm.2), (200, 3 << 20, false));
    assert!(wasm.1.iter().all(|&b| b == 7));
    assert_eq!((missing.0, missing.2), (404, true));
    assert_eq!(t.counters().files, 2);
}

#[test]
fn hostile_requests_are_refused_and_closed() {
    let (mut t, addr) = site(WebLimits::default(), None, Some(files()));
    let cases: Vec<(Vec<u8>, u16)> = vec![
        (
            b"POST / HTTP/1.1\r\nContent-Length: 1\r\n\r\nx".to_vec(),
            405,
        ),
        (b"PUT /index.html HTTP/1.1\r\n\r\n".to_vec(), 405),
        (b"OPTIONS * HTTP/1.1\r\n\r\n".to_vec(), 405),
        (b"GET /../Cargo.toml HTTP/1.1\r\n\r\n".to_vec(), 400),
        (b"GET /%2e%2e/%2e%2e/secret HTTP/1.1\r\n\r\n".to_vec(), 400),
        (b"GET /..\\..\\x HTTP/1.1\r\n\r\n".to_vec(), 400),
        (
            b"GET / HTTP/1.1\r\nContent-Length: 4\r\n\r\nbody".to_vec(),
            400,
        ),
        (
            b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n".to_vec(),
            400,
        ),
        (
            format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(4000)).into_bytes(),
            414,
        ),
        (
            format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "b".repeat(9000)).into_bytes(),
            431,
        ),
        (
            format!("GET / HTTP/1.1\r\n{}\r\n", "X: y\r\n".repeat(40)).into_bytes(),
            431,
        ),
        (b"\x16\x03\x01\x02\x00 not http\r\n\r\n".to_vec(), 400),
        (upgrade_request("not-a-key").into_bytes(), 400),
    ];
    let expected: Vec<u16> = cases.iter().map(|(_, s)| *s).collect();
    let got = serve_while(&mut t, move || {
        cases
            .into_iter()
            .map(|(raw, _)| {
                let mut s = connect(addr);
                let _ = s.write_all(&raw);
                let (status, _, said_close) = response(&mut s);
                (status, said_close && closed(&mut s))
            })
            .collect::<Vec<_>>()
    });
    for (i, ((status, shut), want)) in got.iter().zip(&expected).enumerate() {
        assert_eq!(*status, *want, "case {i}");
        assert!(
            *shut,
            "case {i}: the connection is closed after the refusal"
        );
    }
    assert_eq!(t.counters().files, 0);
    pump_until(&mut t, 500, |t| t.connections() == 0);
    assert_eq!(t.connections(), 0);
}

#[test]
fn a_head_that_never_ends_is_closed_and_connections_are_capped_per_address() {
    let limits = WebLimits {
        head_timeout: Duration::from_millis(300),
        connections_per_ip: 3,
        connects_per_ip_per_s: 100,
        ..WebLimits::default()
    };
    let (mut t, addr) = site(limits, None, Some(files()));
    // A slow peer: a byte at a time, never ending its head.
    let mut slow = connect(addr);
    slow.write_all(b"GET / HTTP/1.1\r\nX-Slow: ").unwrap();
    let mut held: Vec<TcpStream> = (0..2).map(|_| connect(addr)).collect();
    pump_until(&mut t, 500, |t| t.connections() == 3);
    assert_eq!(t.connections(), 3);
    // The fourth from the same address is turned away at once.
    let mut over = connect(addr);
    pump_until(&mut t, 300, |t| t.counters().refused == 1);
    assert_eq!(t.counters().refused, 1);
    assert!(closed(&mut over));
    let longest = pump_until(&mut t, 2000, |t| t.counters().closed_timeout >= 3);
    assert_eq!(
        t.counters().closed_timeout,
        3,
        "the slow head and the two idle ones"
    );
    assert!(closed(&mut slow));
    for s in &mut held {
        assert!(closed(s));
    }
    assert!(
        longest < Duration::from_millis(50),
        "a pass took {longest:?}"
    );
}

#[test]
fn new_connections_are_rate_limited_per_address() {
    let limits = WebLimits {
        connects_per_ip_per_s: 5,
        ..WebLimits::default()
    };
    let (mut t, addr) = site(limits, None, Some(files()));
    let _held: Vec<TcpStream> = (0..8).map(|_| connect(addr)).collect();
    pump_until(&mut t, 500, |t| {
        t.counters().accepted + t.counters().refused == 8
    });
    assert_eq!(t.counters().accepted, 5);
    assert_eq!(t.counters().refused, 3);
}

#[test]
fn a_game_socket_carries_datagrams_both_ways() {
    let (mut t, addr) = site(WebLimits::default(), None, None);
    let mut ws = serve_while(&mut t, move || ws_open(addr));
    let local = ws.get_ref().local_addr().unwrap();
    ws.send(Message::binary(vec![1, 2, 3])).unwrap();
    let mut got = None;
    pump_until(&mut t, 2000, |t| {
        got = recv(t);
        got.is_some()
    });
    assert_eq!(got, Some((local, vec![1, 2, 3])), "from the peer's address");
    assert!(t.holds(local));
    assert!(t.send_to(local, &[9; 1400]));
    assert!(!t.send_to(local, &[9; 1501]), "larger than a datagram");
    assert_eq!(ws.read().unwrap(), Message::binary(vec![9; 1400]));
    // With no files, a page is not found.
    let status = serve_while(&mut t, move || {
        let mut s = connect(addr);
        s.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        response(&mut s).0
    });
    assert_eq!(status, 404);
}

#[test]
fn oversized_flooding_and_text_messages_close_the_socket() {
    let (mut t, addr) = site(WebLimits::default(), None, None);
    let mut big = serve_while(&mut t, move || ws_open(addr));
    let mut flood = serve_while(&mut t, move || ws_open(addr));
    let mut text = serve_while(&mut t, move || ws_open(addr));
    let mut fragments = serve_while(&mut t, move || ws_open(addr));
    assert_eq!(t.sockets(), 4);
    // A frame larger than a datagram (the server stops at its header).
    big.send(Message::binary(vec![0; 64 << 10])).unwrap();
    // Twice the messages a second the cap allows.
    for _ in 0..1100 {
        if flood.send(Message::binary(vec![1; 8])).is_err() {
            break;
        }
    }
    text.send(Message::Text(Utf8Bytes::from_static("hello")))
        .unwrap();
    // A message of small frames that adds up past a datagram.
    for i in 0..4 {
        let frame = tungstenite::protocol::frame::Frame::message(
            vec![2; 600],
            if i == 0 {
                tungstenite::protocol::frame::coding::OpCode::Data(
                    tungstenite::protocol::frame::coding::Data::Binary,
                )
            } else {
                tungstenite::protocol::frame::coding::OpCode::Data(
                    tungstenite::protocol::frame::coding::Data::Continue,
                )
            },
            i == 3,
        );
        let _ = fragments.write(Message::Frame(frame));
    }
    let _ = fragments.flush();
    let longest = pump_until(&mut t, 3000, |t| t.sockets() == 0);
    let c = t.counters();
    assert_eq!(t.sockets(), 0, "{c:?}");
    assert_eq!(c.closed_oversize, 2, "{c:?}");
    assert_eq!(c.closed_rate, 1, "{c:?}");
    assert_eq!(c.closed_protocol, 1, "{c:?}");
    assert!(
        c.messages <= 512,
        "at most a second's worth went to the host: {c:?}"
    );
    assert!(
        longest < Duration::from_millis(50),
        "a pass took {longest:?}"
    );
    for ws in [&mut big, &mut flood, &mut text, &mut fragments] {
        let mut ended = false;
        for _ in 0..2000 {
            match ws.read() {
                Ok(Message::Close(_)) | Err(_) => {
                    ended = true;
                    break;
                }
                Ok(_) => {}
            }
        }
        assert!(ended, "the client sees its socket closed");
    }
}

#[test]
fn quiet_sockets_and_slow_readers_are_closed() {
    let limits = WebLimits {
        idle_timeout: Duration::from_millis(300),
        stall_timeout: Duration::from_millis(300),
        write_buffer: 64 << 10,
        ..WebLimits::default()
    };
    let (mut t, addr) = site(limits, None, None);
    let quiet = serve_while(&mut t, move || ws_open(addr));
    pump_until(&mut t, 2000, |t| t.counters().closed_timeout == 1);
    assert_eq!(t.counters().closed_timeout, 1);
    drop(quiet);
    // A reader that never reads while the server sends: its unsent bytes reach the cap and it is closed.
    let mut slow = serve_while(&mut t, move || ws_open(addr));
    let local = slow.get_ref().local_addr().unwrap();
    let start = Instant::now();
    let mut sent = 0usize;
    while t.holds(local) && start.elapsed() < Duration::from_secs(20) {
        let pass = Instant::now();
        // The slow reader keeps talking, so it is not closed as quiet.
        let _ = slow.send(Message::binary(vec![5]));
        if t.send_to(local, &[0; 1400]) {
            sent += 1;
        }
        t.pump();
        assert!(pass.elapsed() < Duration::from_millis(50));
    }
    assert!(
        !t.holds(local),
        "the slow reader is closed after {sent} datagrams"
    );
    assert_eq!(t.counters().closed_stalled, 1);
}

#[test]
fn plain_web_listens_on_private_addresses_only() {
    let public =
        BindPolicy::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), true, &LanRules::PLAIN).unwrap();
    let mut t = WebTransport::new(public, WebSetup::default());
    assert_eq!(
        t.bind(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        Err(BindError::Policy(PolicyRefusal::PlainWeb(IpAddr::V4(
            Ipv4Addr::UNSPECIFIED
        ))))
    );
    assert!(t.local_addr().is_none(), "nothing was opened");
}

#[test]
fn udp_and_the_web_share_one_port() {
    let mut t = MultiTransport::new(
        UdpTransport::new(policy()),
        WebTransport::new(policy(), WebSetup::default()),
    );
    t.bind(LOOPBACK, 0).unwrap();
    let addr = t.local_addr().unwrap();
    assert_eq!(t.web().local_addr().map(|a| a.port()), Some(addr.port()));
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    udp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    udp.send_to(b"\x01udp", addr).unwrap();
    // The handshake runs while the transport is serviced on this thread.
    let h = thread::spawn(move || ws_open(addr));
    while !h.is_finished() {
        t.service();
        sleep(Duration::from_millis(1));
    }
    let mut ws = h.join().unwrap();
    let ws_addr = ws.get_ref().local_addr().unwrap();
    ws.send(Message::binary(b"\x01web".to_vec())).unwrap();
    let mut got = Vec::new();
    let start = Instant::now();
    while got.len() < 2 && start.elapsed() < Duration::from_secs(5) {
        match recv(&mut t) {
            Some(d) => got.push(d),
            None => sleep(Duration::from_millis(2)),
        }
    }
    got.sort();
    let mut want = vec![
        (udp.local_addr().unwrap(), b"\x01udp".to_vec()),
        (ws_addr, b"\x01web".to_vec()),
    ];
    want.sort();
    assert_eq!(got, want);
    assert!(t.send_to(ws_addr, b"to the browser"));
    assert!(t.send_to(udp.local_addr().unwrap(), b"to the desktop"));
    assert_eq!(
        ws.read().unwrap(),
        Message::binary(b"to the browser".to_vec())
    );
    let mut b = [0u8; 64];
    let (n, _) = udp.recv_from(&mut b).unwrap();
    assert_eq!(&b[..n], b"to the desktop");
}

#[test]
fn tls_serves_files_and_game_sockets() {
    let made = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let tls = Tls::from_pem(
        made.cert.pem().as_bytes(),
        made.key_pair.serialize_pem().as_bytes(),
    )
    .unwrap();
    let (mut t, addr) = site(WebLimits::default(), Some(tls), Some(files()));
    let der = made.cert.der().clone();
    let client = move || {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(der).unwrap();
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        Arc::new(config)
    };
    let config = client();
    let (page, mut ws) = serve_while(&mut t, move || {
        let open = || {
            let name = "localhost".try_into().unwrap();
            let conn = rustls::ClientConnection::new(Arc::clone(&config), name).unwrap();
            rustls::StreamOwned::new(conn, connect(addr))
        };
        let mut s = open();
        s.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let page = response(&mut s);
        (page, ws_over(open()))
    });
    assert_eq!((page.0, page.1.as_slice()), (200, PAGE));
    ws.send(Message::binary(vec![4, 5])).unwrap();
    let mut got = None;
    pump_until(&mut t, 2000, |t| {
        got = recv(t);
        got.is_some()
    });
    let (from, data) = got.expect("a datagram over wss");
    assert_eq!(data, [4, 5]);
    assert!(t.send_to(from, b"back"));
    pump_until(&mut t, 50, |_| false);
    assert_eq!(ws.read().unwrap(), Message::binary(b"back".to_vec()));
    // Plain HTTP to the TLS port is not answered as a page.
    let shut = serve_while(&mut t, move || {
        let mut s = connect(addr);
        s.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        let (status, _, _) = response(&mut s);
        status != 200
    });
    assert!(shut);
}
