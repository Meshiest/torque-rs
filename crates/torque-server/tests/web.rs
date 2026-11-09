//! The web transport on the loopback address: files over HTTP, game sockets over WebSocket (plain and under TLS), UDP and the
//! web on one port, and hostile peers: requests past the caps, other methods, paths that climb, heads that never end,
//! too many connections, oversized and flooding messages, text messages, quiet sockets, slow readers, and TLS records that
//! carry nothing. Each hostile peer is closed, and a pass's time stays bounded.

use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use torque_server::net::transport::{
    BindError, BindPolicy, LanRules, PolicyRefusal, RECV_BUFFER_BYTES, Recv, Transport,
    UdpTransport,
};
use torque_server::net::web::http::base64;
use torque_server::net::web::is_stand_in;
use torque_server::net::web::{
    Access, MultiTransport, Origins, Tls, WEB_SOCKET_PATH, WebFiles, WebLimits, WebSetup,
    WebTransport, accept_key,
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
    site_with(limits, tls, files, Access::default())
}

fn site_with(
    limits: WebLimits,
    tls: Option<Tls>,
    files: Option<WebFiles>,
    access: Access,
) -> (WebTransport, SocketAddr) {
    #[allow(
        clippy::needless_update,
        reason = "the setup has more fields with the webrtc feature"
    )]
    let setup = WebSetup {
        files,
        tls,
        access,
        limits,
        ..WebSetup::default()
    };
    let mut t = WebTransport::new(policy(), setup);
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
        "GET {WEB_SOCKET_PATH} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
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
        s.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let page = response(&mut s);
        s.write_all(b"GET /game_bg.wasm HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        let wasm = response(&mut s);
        s.write_all(b"GET /nothing HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
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
    let (from, data) = got.expect("a datagram");
    assert_eq!(data, [1, 2, 3]);
    assert!(is_stand_in(from), "a stand-in address: {from}");
    assert_eq!(t.peer_of(from), Some(local));
    // The host's caps count the TCP peer behind the stand-in, and a socket's link is a validated one.
    assert_eq!(t.peer_ip(from), local.ip());
    assert!(t.link_kind(from).facts.validated);
    let elsewhere = SocketAddr::from(([192, 168, 3, 4], 5_000));
    assert_eq!(
        t.peer_ip(elsewhere),
        elsewhere.ip(),
        "an address that is no stand-in is its own"
    );
    assert!(t.holds(from) && !t.holds(local));
    assert!(t.send_to(from, &[9; 1400]));
    assert!(
        !t.send_to(local, &[9; 1400]),
        "the peer's own address is not the socket's"
    );
    assert!(!t.send_to(from, &[9; 1501]), "larger than a datagram");
    assert_eq!(ws.read().unwrap(), Message::binary(vec![9; 1400]));
    // With no files, a page is not found.
    let status = serve_while(&mut t, move || {
        let mut s = connect(addr);
        s.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
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
    slow.send(Message::binary(vec![5])).unwrap();
    let mut first = None;
    pump_until(&mut t, 2000, |t| {
        first = recv(t);
        first.is_some()
    });
    let local = first.expect("the slow reader's first datagram").0;
    let start = Instant::now();
    let mut sent = 0usize;
    let mut talked = start;
    while t.holds(local) && start.elapsed() < Duration::from_secs(20) {
        let pass = Instant::now();
        // The slow reader keeps talking, so it is not closed as quiet; well inside the message rate, since the loop runs
        // until the kernel's buffers fill, and Linux's on loopback hold megabytes (thousands of passes).
        if talked.elapsed() >= Duration::from_millis(50) {
            let _ = slow.send(Message::binary(vec![5]));
            talked = Instant::now();
        }
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
    let c = t.counters();
    assert_eq!(c.closed_stalled, 1, "after {sent} datagrams: {c:?}");
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
    let ws_peer = ws.get_ref().local_addr().unwrap();
    ws.send(Message::binary(b"\x01web".to_vec())).unwrap();
    let mut got = Vec::new();
    let start = Instant::now();
    while got.len() < 2 && start.elapsed() < Duration::from_secs(5) {
        match recv(&mut t) {
            Some(d) => got.push(d),
            None => sleep(Duration::from_millis(2)),
        }
    }
    got.sort_by_key(|(from, _)| is_stand_in(*from));
    assert_eq!(got.len(), 2);
    assert_eq!(got[0], (udp.local_addr().unwrap(), b"\x01udp".to_vec()));
    assert_eq!(got[1].1, b"\x01web");
    let ws_addr = got[1].0;
    assert!(is_stand_in(ws_addr));
    assert_eq!(t.web().peer_of(ws_addr), Some(ws_peer));
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

#[test]
fn a_udp_peer_at_a_sockets_tcp_address_is_never_taken_for_it() {
    let mut t = MultiTransport::new(
        UdpTransport::new(policy()),
        WebTransport::new(policy(), WebSetup::default()),
    );
    t.bind(LOOPBACK, 0).unwrap();
    let addr = t.local_addr().unwrap();
    let h = thread::spawn(move || ws_open(addr));
    while !h.is_finished() {
        t.service();
        sleep(Duration::from_millis(1));
    }
    let mut ws = h.join().unwrap();
    // A UDP peer on the socket's own IP and port number (UDP and TCP ports are apart).
    let twin_addr = ws.get_ref().local_addr().unwrap();
    let twin = UdpSocket::bind(twin_addr).unwrap();
    twin.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    twin.send_to(b"\x01twin", addr).unwrap();
    ws.send(Message::binary(b"\x01sock".to_vec())).unwrap();
    let mut got = Vec::new();
    let start = Instant::now();
    while got.len() < 2 && start.elapsed() < Duration::from_secs(5) {
        match recv(&mut t) {
            Some(d) => got.push(d),
            None => sleep(Duration::from_millis(2)),
        }
    }
    got.sort_by_key(|(from, _)| is_stand_in(*from));
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(
        got[0],
        (twin_addr, b"\x01twin".to_vec()),
        "the UDP peer keeps its own address"
    );
    let sock = got[1].0;
    assert!(is_stand_in(sock) && sock != twin_addr);
    // Each address reaches its own peer.
    assert!(t.send_to(twin_addr, b"udp"));
    assert!(t.send_to(sock, b"web"));
    let mut b = [0u8; 16];
    let (n, _) = twin.recv_from(&mut b).unwrap();
    assert_eq!(&b[..n], b"udp");
    assert_eq!(ws.read().unwrap(), Message::binary(b"web".to_vec()));
}

#[test]
fn the_per_address_socket_cap_is_reserved_when_the_answer_is_queued() {
    let limits = WebLimits {
        sockets_per_ip: 1,
        ..WebLimits::default()
    };
    let (mut t, addr) = site(limits, None, None);
    // Two upgrades from one address arrive in one pass: one socket, one refusal.
    let mut a = connect(addr);
    let mut b = connect(addr);
    let key = base64(&[3; 16]);
    a.write_all(upgrade_request(&key).as_bytes()).unwrap();
    b.write_all(upgrade_request(&key).as_bytes()).unwrap();
    sleep(Duration::from_millis(50));
    t.pump();
    pump_until(&mut t, 500, |t| {
        t.counters().upgrades + t.counters().refusals == 2
    });
    assert_eq!(t.counters().upgrades, 1);
    assert_eq!(t.counters().refusals, 1);
    assert_eq!(t.sockets_from(LOOPBACK), 1);
    let statuses: Vec<u16> = [&mut a, &mut b]
        .into_iter()
        .map(|s| {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && s.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            String::from_utf8_lossy(&head[9..12]).parse().unwrap()
        })
        .collect();
    let mut sorted = statuses.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, [101, 429], "{statuses:?}");
    // The socket closes: its place is free again, and a new socket opens.
    drop(a);
    drop(b);
    pump_until(&mut t, 2000, |t| t.connections() == 0);
    assert_eq!(t.sockets_from(LOOPBACK), 0);
    let _again = serve_while(&mut t, move || ws_open(addr));
    assert_eq!(t.sockets_from(LOOPBACK), 1);
    // An answer that is never written (the peer gone before it) gives its place back too.
    let mut c = connect(addr);
    let limits_hit = t.counters().refusals;
    c.write_all(upgrade_request(&key).as_bytes()).unwrap();
    drop(c);
    pump_until(&mut t, 1000, |t| {
        t.counters().refusals > limits_hit || t.connections() == 1
    });
    assert_eq!(t.sockets_from(LOOPBACK), 1);
}

#[test]
fn hosts_and_origins_are_checked() {
    let access = Access {
        hosts: vec!["games.example".to_string()],
        origins: Origins::SameHost,
    };
    let (mut t, addr) = site_with(WebLimits::default(), None, Some(files()), access);
    let port = addr.port();
    let ask = move |host: &str, origin: Option<&str>, upgrade: bool| -> u16 {
        let mut s = connect(addr);
        let origin = origin.map_or(String::new(), |o| format!("Origin: {o}\r\n"));
        let request = if upgrade {
            format!(
                "GET {WEB_SOCKET_PATH} HTTP/1.1\r\nHost: {host}\r\n{origin}Upgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {}\r\n\r\n",
                base64(&[3; 16])
            )
        } else {
            format!("GET / HTTP/1.1\r\nHost: {host}\r\n{origin}\r\n")
        };
        s.write_all(request.as_bytes()).unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") && s.read(&mut byte).unwrap_or(0) == 1 {
            head.push(byte[0]);
        }
        String::from_utf8_lossy(&head[9..12]).parse().unwrap()
    };
    let got = serve_while(&mut t, move || {
        let own = format!("127.0.0.1:{port}");
        let page = format!("http://127.0.0.1:{port}");
        vec![
            ask(&own, None, false),
            ask(&format!("localhost:{port}"), None, false),
            ask(&format!("games.example:{port}"), None, false),
            // A name of someone else's, pointed at this server's address.
            ask(&format!("evil.example:{port}"), None, false),
            ask(
                &format!("evil.example:{port}"),
                Some(&format!("http://evil.example:{port}")),
                true,
            ),
            // The server's own page.
            ask(&own, Some(&page), true),
            // Another site's page.
            ask(&own, Some("https://evil.example"), true),
            ask(&own, Some("null"), true),
            ask(&own, Some(&format!("https://127.0.0.1:{port}")), true),
            // Not a browser's page.
            ask(&own, None, true),
        ]
    });
    assert_eq!(got, [200, 200, 200, 421, 421, 101, 403, 403, 403, 101]);
    // A page served elsewhere, by the list.
    let access = Access {
        hosts: Vec::new(),
        origins: Origins::List(vec!["https://games.example".to_string()]),
    };
    let (mut t, addr) = site_with(WebLimits::default(), None, None, access);
    let got = serve_while(&mut t, move || {
        let own = format!("127.0.0.1:{}", addr.port());
        let mut out = Vec::new();
        for origin in [
            "https://games.example/",
            "https://other.example",
            &format!("http://{own}"),
        ] {
            let mut s = connect(addr);
            let request = format!(
                "GET {WEB_SOCKET_PATH} HTTP/1.1\r\nHost: {own}\r\nOrigin: {origin}\r\nUpgrade: websocket\r\n\
                 Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {}\r\n\r\n",
                base64(&[3; 16])
            );
            s.write_all(request.as_bytes()).unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && s.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            out.push(
                String::from_utf8_lossy(&head[9..12])
                    .parse::<u16>()
                    .unwrap(),
            );
        }
        out
    });
    assert_eq!(got, [101, 403, 403]);
}

/// A raw game socket: the handshake made, the stream left for hand-made frames.
fn raw_socket(t: &mut WebTransport, addr: SocketAddr) -> TcpStream {
    serve_while(t, move || {
        let mut s = connect(addr);
        s.write_all(upgrade_request(&base64(&[3; 16])).as_bytes())
            .unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            assert_eq!(s.read(&mut b).unwrap(), 1);
            head.push(b[0]);
        }
        assert!(head.starts_with(b"HTTP/1.1 101 "));
        s
    })
}

/// Floods `s` from a thread with a binary frame left open and empty continuation frames after it (6 bytes each, masked),
/// for at most `ms`; returns the bytes it got out.
fn flood_continuations(mut s: TcpStream, ms: u64) -> thread::JoinHandle<usize> {
    thread::spawn(move || {
        let _ = s.set_write_timeout(Some(Duration::from_millis(200)));
        let mut sent = 0;
        if s.write_all(&[0x02, 0x80, 1, 2, 3, 4]).is_err() {
            return sent;
        }
        let chunk: Vec<u8> = [0x00, 0x80, 1, 2, 3, 4].repeat(4096);
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(ms) {
            match s.write(&chunk) {
                Ok(n) => sent += n,
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => break,
            }
        }
        sent
    })
}

#[test]
fn endless_continuation_frames_are_metered_and_closed() {
    // As the limits are: the message's frame count closes it.
    let (mut t, addr) = site(WebLimits::default(), None, None);
    let s = raw_socket(&mut t, addr);
    let h = flood_continuations(s, 3000);
    let longest = pump_until(&mut t, 3000, |t| t.sockets() == 0);
    assert_eq!(t.sockets(), 0);
    assert_eq!(t.counters().closed_protocol, 1, "{:?}", t.counters());
    assert!(
        longest < Duration::from_millis(20),
        "a pass took {longest:?}"
    );
    h.join().unwrap();

    // With no frame cap per message, the frames a second close it, and no pass reads more than its byte budget.
    let limits = WebLimits {
        frames_per_message: u32::MAX,
        ..WebLimits::default()
    };
    let budget = limits.bytes_per_pass;
    let (mut t, addr) = site(limits, None, None);
    let s = raw_socket(&mut t, addr);
    let h = flood_continuations(s, 3000);
    let longest = pump_until(&mut t, 3000, |t| t.sockets() == 0);
    assert_eq!(t.sockets(), 0);
    assert_eq!(t.counters().closed_rate, 1, "{:?}", t.counters());
    assert!(
        longest < Duration::from_millis(20),
        "a pass took {longest:?}"
    );
    let sent = h.join().unwrap();
    assert!(
        sent > budget,
        "the flood outran one pass's budget ({sent} bytes)"
    );
}

#[test]
fn pings_alone_do_not_keep_a_socket() {
    let limits = WebLimits {
        idle_timeout: Duration::from_millis(300),
        ..WebLimits::default()
    };
    let (mut t, addr) = site(limits, None, None);
    let mut ws = serve_while(&mut t, move || ws_open(addr));
    ws.get_mut().set_nonblocking(true).unwrap();
    let start = Instant::now();
    while t.sockets() == 1 && start.elapsed() < Duration::from_secs(3) {
        let _ = ws.send(Message::Ping(vec![1].into()));
        let _ = ws.read();
        t.pump();
        sleep(Duration::from_millis(20));
    }
    assert_eq!(t.sockets(), 0);
    assert_eq!(t.counters().closed_timeout, 1);
    // The host hears of it, to drop the connection.
    assert_eq!(t.take_gone().len(), 1);
    assert!(t.take_gone().is_empty());
}

#[test]
fn an_idle_connection_makes_room_when_every_place_is_taken() {
    let limits = WebLimits {
        max_connections: 3,
        ..WebLimits::default()
    };
    let (mut t, addr) = site(limits, None, Some(files()));
    let mut idle: Vec<TcpStream> = (0..3).map(|_| connect(addr)).collect();
    pump_until(&mut t, 500, |t| t.connections() == 3);
    let page = serve_while(&mut t, move || {
        let mut s = connect(addr);
        s.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        response(&mut s).0
    });
    assert_eq!(page, 200, "the new connection is served");
    assert_eq!(t.counters().evicted, 1);
    assert_eq!(t.counters().refused, 0);
    assert_eq!(
        idle.iter_mut().map(closed).filter(|&c| c).count(),
        1,
        "the oldest idle one was closed"
    );
}

#[test]
fn a_response_read_too_slowly_is_closed() {
    let limits = WebLimits {
        stall_timeout: Duration::from_millis(200),
        min_send_rate: 16 << 20,
        ..WebLimits::default()
    };
    let mut big = files();
    big.insert("big.bin", vec![9; 32 << 20]);
    let (mut t, addr) = site(limits, None, Some(big));
    let read = serve_while(&mut t, move || {
        let mut s = connect(addr);
        s.write_all(b"GET /big.bin HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .unwrap();
        // A trickle: 16 KiB every 20 ms, always some progress, far below the rate allowed.
        let mut total = 0usize;
        let mut b = vec![0u8; 16 << 10];
        loop {
            match s.read(&mut b) {
                Ok(0) | Err(_) => return total,
                Ok(n) => total += n,
            }
            sleep(Duration::from_millis(20));
        }
    });
    assert!(
        read < 32 << 20,
        "the reader got {read} bytes before it was cut"
    );
    assert_eq!(t.counters().closed_stalled, 1);
}

#[test]
fn a_tls_handshake_has_a_deadline() {
    let made = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let tls = Tls::from_pem(
        made.cert.pem().as_bytes(),
        made.key_pair.serialize_pem().as_bytes(),
    )
    .unwrap();
    let limits = WebLimits {
        handshake_timeout: Duration::from_millis(300),
        ..WebLimits::default()
    };
    let (mut t, addr) = site(limits, Some(tls), Some(files()));
    // A peer that never says hello.
    let mut s = connect(addr);
    pump_until(&mut t, 2000, |t| t.counters().accepted == 1);
    let start = Instant::now();
    pump_until(&mut t, 2000, |t| t.connections() == 0);
    assert!(
        start.elapsed() >= Duration::from_millis(250),
        "closed by the deadline"
    );
    assert_eq!(t.connections(), 0);
    assert_eq!(t.counters().closed_timeout, 1);
    assert!(closed(&mut s));
}

#[test]
fn frames_sent_with_the_upgrade_request_are_refused() {
    let (mut t, addr) = site(WebLimits::default(), None, None);
    let status = serve_while(&mut t, move || {
        let mut s = connect(addr);
        let mut raw = upgrade_request(&base64(&[3; 16])).into_bytes();
        // A binary frame left open and empty continuations, pipelined behind the head.
        raw.extend_from_slice(&[0x02, 0x80, 1, 2, 3, 4]);
        raw.extend_from_slice(&[0x00, 0x80, 1, 2, 3, 4].repeat(100));
        s.write_all(&raw).unwrap();
        let (status, _, said_close) = response(&mut s);
        (status, said_close && closed(&mut s))
    });
    assert_eq!(status, (400, true));
    assert_eq!(t.counters().upgrades, 0);
    assert_eq!(t.sockets_from(LOOPBACK), 0, "no place was reserved");
}

/// A test certificate and the TLS settings made from it.
fn test_tls() -> (Tls, rustls::pki_types::CertificateDer<'static>) {
    let made = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let tls = Tls::from_pem(
        made.cert.pem().as_bytes(),
        made.key_pair.serialize_pem().as_bytes(),
    )
    .unwrap();
    (tls, made.cert.der().clone())
}

/// The client's first application traffic secret, as rustls logs it.
#[derive(Debug, Default)]
struct ClientSecret(Mutex<Option<Vec<u8>>>);

impl rustls::KeyLog for ClientSecret {
    fn log(&self, label: &str, _client_random: &[u8], secret: &[u8]) {
        if label == "CLIENT_TRAFFIC_SECRET_0" {
            *self.0.lock().unwrap() = Some(secret.to_vec());
        }
    }
}

struct HkdfLen(usize);

impl ring::hkdf::KeyType for HkdfLen {
    fn len(&self) -> usize {
        self.0
    }
}

/// TLS 1.3's HKDF-Expand-Label with an empty context (SHA-256).
fn expand_label(secret: &[u8], label: &str, len: usize) -> Vec<u8> {
    let full = format!("tls13 {label}");
    let info = [
        &(len as u16).to_be_bytes()[..],
        &[full.len() as u8],
        full.as_bytes(),
        &[0],
    ]
    .concat();
    let prk = ring::hkdf::Prk::new_less_safe(ring::hkdf::HKDF_SHA256, secret);
    let mut out = vec![0; len];
    prk.expand(&[&info], HkdfLen(len))
        .unwrap()
        .fill(&mut out)
        .unwrap();
    out
}

/// A TLS 1.3 client's sending side sealed by hand (TLS_AES_128_GCM_SHA256), for records a TLS library never sends.
struct RawSender {
    secret: Vec<u8>,
    key: ring::aead::LessSafeKey,
    iv: [u8; 12],
    seq: u64,
}

impl RawSender {
    fn new(secret: Vec<u8>) -> RawSender {
        let key = expand_label(&secret, "key", 16);
        let key = ring::aead::UnboundKey::new(&ring::aead::AES_128_GCM, &key).unwrap();
        RawSender {
            iv: expand_label(&secret, "iv", 12).try_into().unwrap(),
            key: ring::aead::LessSafeKey::new(key),
            secret,
            seq: 0,
        }
    }

    /// One record of `content_type` holding `content`, sealed onto `out`.
    fn record(&mut self, content_type: u8, content: &[u8], out: &mut Vec<u8>) {
        let mut inner = content.to_vec();
        inner.push(content_type);
        let len = inner.len() + 16;
        let header = [0x17, 3, 3, (len >> 8) as u8, len as u8];
        let mut nonce = self.iv;
        for (n, b) in nonce[4..].iter_mut().zip(self.seq.to_be_bytes()) {
            *n ^= b;
        }
        self.key
            .seal_in_place_append_tag(
                ring::aead::Nonce::assume_unique_for_key(nonce),
                ring::aead::Aad::from(header),
                &mut inner,
            )
            .unwrap();
        out.extend_from_slice(&header);
        out.extend_from_slice(&inner);
        self.seq += 1;
    }

    /// A key update that asks for none back, then the keys after it.
    fn key_update(&mut self, out: &mut Vec<u8>) {
        self.record(22, &[24, 0, 0, 1, 0], out);
        *self = RawSender::new(expand_label(&self.secret, "traffic upd", 32));
    }
}

/// A TLS 1.3 connection made with rustls, its sending side then taken over by hand: the stream and the sender. With
/// `upgrade`, a game socket is opened over it first (its request sealed by hand, the 101 read through rustls).
fn raw_tls(
    addr: SocketAddr,
    der: rustls::pki_types::CertificateDer<'static>,
    upgrade: bool,
) -> (TcpStream, RawSender) {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(der).unwrap();
    let provider = rustls::crypto::CryptoProvider {
        cipher_suites: vec![rustls::crypto::ring::cipher_suite::TLS13_AES_128_GCM_SHA256],
        ..rustls::crypto::ring::default_provider()
    };
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let secret = Arc::new(ClientSecret::default());
    config.key_log = Arc::clone(&secret) as Arc<dyn rustls::KeyLog>;
    let name = "localhost".try_into().unwrap();
    let mut conn = rustls::ClientConnection::new(Arc::new(config), name).unwrap();
    let mut tcp = connect(addr);
    while conn.is_handshaking() || conn.wants_write() {
        conn.complete_io(&mut tcp).unwrap();
    }
    let secret = secret.0.lock().unwrap().take().expect("the traffic secret");
    let mut sender = RawSender::new(secret);
    if upgrade {
        let mut out = Vec::new();
        sender.record(23, upgrade_request(&base64(&[3; 16])).as_bytes(), &mut out);
        tcp.write_all(&out).unwrap();
        let mut head = Vec::new();
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            assert!(
                conn.read_tls(&mut tcp).unwrap() > 0,
                "the handshake's answer"
            );
            conn.process_new_packets().unwrap();
            let _ = conn.reader().read_to_end(&mut head);
        }
        assert!(head.starts_with(b"HTTP/1.1 101 "));
    }
    (tcp, sender)
}

/// Floods `tcp` from a thread with key updates and empty application data records, one after the other, for at most `ms`
/// (rustls's own limits on runs of either never trip); returns the bytes it got out.
fn flood_key_updates(
    mut tcp: TcpStream,
    mut sender: RawSender,
    ms: u64,
) -> thread::JoinHandle<usize> {
    thread::spawn(move || {
        let _ = tcp.set_write_timeout(Some(Duration::from_secs(1)));
        let mut sent = 0;
        let mut out = Vec::new();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(ms) {
            out.clear();
            for _ in 0..256 {
                sender.key_update(&mut out);
                sender.record(23, &[], &mut out);
            }
            if tcp.write_all(&out).is_err() {
                break;
            }
            sent += out.len();
        }
        sent
    })
}

#[test]
fn tls_records_without_plaintext_are_metered_and_closed() {
    let (tls, der) = test_tls();
    // Over https before any request, and over wss on a game socket.
    for upgrade in [false, true] {
        let limits = WebLimits::default();
        let cap = if upgrade {
            limits.bytes_per_s
        } else {
            limits.http_bytes_per_s
        };
        let (mut t, addr) = site(limits, Some(tls.clone()), Some(files()));
        let peer_der = der.clone();
        let (tcp, sender) = serve_while(&mut t, move || raw_tls(addr, peer_der, upgrade));
        assert_eq!(t.sockets(), usize::from(upgrade));
        let h = flood_key_updates(tcp, sender, 5000);
        let longest = pump_until(&mut t, 5000, |t| t.connections() == 0);
        let sent = h.join().unwrap();
        assert_eq!(t.connections(), 0, "upgrade {upgrade}: still open");
        assert_eq!(
            t.counters().closed_rate,
            1,
            "upgrade {upgrade}: {:?}",
            t.counters()
        );
        assert!(
            longest < Duration::from_millis(20),
            "upgrade {upgrade}: a pass took {longest:?}"
        );
        assert!(sent > cap, "upgrade {upgrade}: the flood sent {sent} bytes");
    }
}
