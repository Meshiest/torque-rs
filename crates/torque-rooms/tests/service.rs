//! The room service on the loopback address, over real sockets: a host and a joiner find each other by code and their
//! signaling crosses byte for byte; hostile sockets (text frames, bytes that are not UTF-8, messages over the cap, frame
//! floods, game datagrams posing as signaling, a page from elsewhere) are closed or refused while a pass stays short.

use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use torque_rooms::{Ice, RoomLimits, Rooms, Service, service_limits};
use torque_server::net::room::{FromService, MAX_MESSAGE, ToService};
use torque_server::net::transport::{BindPolicy, LanRules};
use torque_server::net::web::http::base64;
use torque_server::net::web::{WEB_SOCKET_PATH, WebSetup, accept_key};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::{Error as WsError, Message};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn service() -> (Service, SocketAddr) {
    let policy = BindPolicy::new(LOOPBACK, false, &LanRules::PLAIN).unwrap();
    let ice = Ice {
        stun: vec!["stun:192.0.2.1:3478".into()],
        turn: vec!["turn:192.0.2.1:3478".into()],
        secret: Some(b"test-secret".to_vec()),
        ttl: Duration::from_secs(600),
    };
    // Every socket of the tests comes from one address: more new connections a second than a player makes.
    let setup = WebSetup {
        limits: torque_server::net::web::WebLimits {
            connects_per_ip_per_s: 64,
            ..service_limits()
        },
        ..WebSetup::default()
    };
    let mut s = Service::new(policy, setup, Rooms::new(RoomLimits::default(), ice));
    s.bind(LOOPBACK, 0).unwrap();
    let addr = s.local_addr().unwrap();
    (s, addr)
}

/// Opens a socket while the service is passed (the handshake blocks), then makes it non-blocking.
fn open(
    s: &mut Service,
    addr: SocketAddr,
    origin: Option<&str>,
) -> Result<WebSocket<TcpStream>, String> {
    let origin = origin
        .map(|o| format!("Origin: {o}\r\n"))
        .unwrap_or_default();
    let h = thread::spawn(move || -> Result<TcpStream, String> {
        let mut t = TcpStream::connect(addr).unwrap();
        t.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let key = base64(&[9; 16]);
        write!(
            t,
            "GET {WEB_SOCKET_PATH} HTTP/1.1\r\nHost: 127.0.0.1\r\n{origin}Upgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n"
        )
        .unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if t.read(&mut b).unwrap_or(0) != 1 {
                break;
            }
            head.push(b[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        if !head.starts_with("HTTP/1.1 101 ") {
            return Err(head);
        }
        assert!(head.contains(&accept_key(&key)));
        Ok(t)
    });
    while !h.is_finished() {
        s.pass();
        sleep(Duration::from_millis(1));
    }
    let t = h.join().unwrap()?;
    t.set_nonblocking(true).unwrap();
    Ok(WebSocket::from_raw_socket(t, Role::Client, None))
}

/// Something a test socket does.
type Act = Box<dyn Fn(&mut WebSocket<TcpStream>)>;

fn send(ws: &mut WebSocket<TcpStream>, text: &str) {
    let _ = ws.send(Message::binary(text.as_bytes().to_vec()));
}

/// What a socket got: its messages, and whether it closed.
#[derive(Default, Debug)]
struct Got {
    messages: Vec<String>,
    closed: bool,
}

fn read(ws: &mut WebSocket<TcpStream>, got: &mut Got) {
    loop {
        match ws.read() {
            Ok(Message::Binary(b)) => got.messages.push(String::from_utf8(b.to_vec()).unwrap()),
            Ok(Message::Close(_)) => got.closed = true,
            Ok(_) => {}
            Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => break,
            Err(_) => {
                got.closed = true;
                break;
            }
        }
    }
}

/// Passes the service until `done` holds for what `ws` got (at most 5 s).
fn until(
    s: &mut Service,
    ws: &mut WebSocket<TcpStream>,
    got: &mut Got,
    done: impl Fn(&Got) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(got) && Instant::now() < deadline {
        s.pass();
        read(ws, got);
        sleep(Duration::from_millis(1));
    }
}

fn last(got: &Got) -> &str {
    got.messages.last().map_or("", String::as_str)
}

#[test]
fn a_host_and_a_joiner_signal_through_the_service() {
    let (mut s, addr) = service();
    let mut host = open(&mut s, addr, None).unwrap();
    let mut joiner = open(&mut s, addr, None).unwrap();
    let (mut h, mut j) = (Got::default(), Got::default());
    send(&mut host, "host");
    until(&mut s, &mut host, &mut h, |g| !g.messages.is_empty());
    let Some(FromService::Room { code, seconds, ice }) = FromService::parse(last(&h)) else {
        panic!("{h:?}");
    };
    assert_eq!(seconds, 6 * 3600);
    assert!(
        ice.contains("stun:192.0.2.1:3478") && ice.contains("\"credential\":"),
        "{ice}"
    );
    let code = code.to_string();
    send(&mut joiner, &ToService::Join(&code.to_lowercase()).write());
    until(&mut s, &mut joiner, &mut j, |g| !g.messages.is_empty());
    let Some(FromService::Joined { peer, ice }) = FromService::parse(last(&j)) else {
        panic!("{j:?}");
    };
    assert!(ice.contains("\"username\":"));
    until(&mut s, &mut host, &mut h, |g| g.messages.len() >= 2);
    assert_eq!(last(&h), format!("peer {peer} 127.0.0.1"));
    // A long offer of the kinds of bytes an SDP may hold here (printable ASCII and tabs) crosses as it was sent, and the
    // answer back.
    let offer: String = format!(
        "offer\nv=0\r\no=- 4611731400430051336 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0\r\n{}",
        "a=x:\t~!@#$%^&*()\r\n".repeat(200)
    );
    assert!(offer.len() > 3500 && offer.len() < 4096);
    send(&mut joiner, &ToService::Signal(&offer).write());
    until(&mut s, &mut host, &mut h, |g| g.messages.len() >= 3);
    assert_eq!(last(&h), FromService::From { peer, body: &offer }.write());
    send(&mut host, &ToService::To(peer, "answer\nv=0\r\n").write());
    until(&mut s, &mut joiner, &mut j, |g| g.messages.len() >= 2);
    assert_eq!(last(&j), "signal\nanswer\nv=0\r\n");
    // The host goes: the joiner is told and closed, and the room is gone.
    drop(host);
    until(&mut s, &mut joiner, &mut j, |g| g.closed);
    assert!(j.messages.iter().any(|m| m == "closed host-left"), "{j:?}");
    assert_eq!(s.rooms().rooms(), 0);
}

#[test]
fn hostile_sockets_are_closed_and_a_pass_stays_short() {
    let (mut s, addr) = service();
    let mut host = open(&mut s, addr, None).unwrap();
    let mut h = Got::default();
    send(&mut host, "host");
    until(&mut s, &mut host, &mut h, |g| !g.messages.is_empty());
    let code = match FromService::parse(last(&h)) {
        Some(FromService::Room { code, .. }) => code.to_string(),
        _ => panic!("{h:?}"),
    };
    // Each of these closes its socket; the host is untouched.
    let cases: Vec<(&str, Act)> = vec![
        (
            "a text frame",
            Box::new(|ws| drop(ws.send(Message::text("host")))),
        ),
        (
            "bytes that are not UTF-8",
            Box::new(|ws| drop(ws.send(Message::binary(vec![0xc3, 0x28])))),
        ),
        ("an unknown verb", Box::new(|ws| send(ws, "GET / HTTP/1.1"))),
        (
            "a message over the cap",
            Box::new(|ws| drop(ws.send(Message::binary(vec![b'a'; MAX_MESSAGE + 1])))),
        ),
        (
            "a game datagram as a joiner's signal",
            Box::new(|ws| send(ws, "signal\n\x01\x02\x03")),
        ),
        (
            "a joiner's message from no joiner",
            Box::new(|ws| send(ws, "signal\noffer\nv=0")),
        ),
        (
            "a host's message from no host",
            Box::new(|ws| send(ws, "to 1\nanswer\nv=0")),
        ),
    ];
    for (what, act) in cases {
        let mut ws = open(&mut s, addr, None).unwrap();
        let mut got = Got::default();
        act(&mut ws);
        until(&mut s, &mut ws, &mut got, |g| g.closed);
        assert!(got.closed, "{what}: {got:?}");
    }
    // A joiner that sends its game's datagrams through the room once it is in is closed too.
    let mut joiner = open(&mut s, addr, None).unwrap();
    let mut j = Got::default();
    send(&mut joiner, &format!("join {code}"));
    until(&mut s, &mut joiner, &mut j, |g| !g.messages.is_empty());
    assert!(last(&j).starts_with("joined "), "{j:?}");
    send(&mut joiner, "signal\n\u{1}game");
    until(&mut s, &mut joiner, &mut j, |g| g.closed);
    assert!(j.closed);
    // A flood of frames (empty ones, and pings) is cut by the socket's meter, and no pass takes long meanwhile.
    let mut floods: Vec<WebSocket<TcpStream>> =
        (0..4).map(|_| open(&mut s, addr, None).unwrap()).collect();
    let mut longest = Duration::ZERO;
    let started = Instant::now();
    let mut closed = 0;
    while started.elapsed() < Duration::from_secs(3) && closed < floods.len() {
        for ws in &mut floods {
            for _ in 0..200 {
                if ws.write(Message::binary(Vec::new())).is_err()
                    || ws.write(Message::Ping(Vec::new().into())).is_err()
                {
                    break;
                }
            }
            let _ = ws.flush();
        }
        let t = Instant::now();
        s.pass();
        longest = longest.max(t.elapsed());
        closed = 0;
        for ws in &mut floods {
            let mut got = Got::default();
            read(ws, &mut got);
            if got.closed || !ws.can_write() {
                closed += 1;
            }
        }
    }
    assert_eq!(closed, floods.len(), "every flooding socket is closed");
    assert!(
        longest < Duration::from_millis(50),
        "a pass took {longest:?}"
    );
    assert!(
        s.web().counters().closed_rate >= 4,
        "{:?}",
        s.web().counters()
    );
    // The host was never touched.
    send(&mut host, "ping");
    until(&mut s, &mut host, &mut h, |g| last(g) == "pong");
    assert_eq!(last(&h), "pong");
    // A page from elsewhere may not open a socket (the web transport's Origin check).
    assert!(
        open(&mut s, addr, Some("http://elsewhere.example"))
            .unwrap_err()
            .starts_with("HTTP/1.1 403")
    );
}

#[test]
fn guessing_codes_is_refused_after_a_few_tries() {
    let (mut s, addr) = service();
    let mut ws = open(&mut s, addr, None).unwrap();
    let mut got = Got::default();
    for n in 0..25 {
        send(&mut ws, &format!("join {:08}", n));
        until(&mut s, &mut ws, &mut got, |g| g.messages.len() > n);
    }
    let refused: Vec<&str> = got.messages.iter().map(String::as_str).collect();
    assert!(
        refused[..20].iter().all(|m| *m == "refused no-room"),
        "{refused:?}"
    );
    assert!(
        refused[20..].iter().all(|m| *m == "refused rate"),
        "{refused:?}"
    );
    assert!(!got.closed, "a refusal keeps the socket");
}

/// The TURN secret file's rules: a regular file, not much longer than a secret, and on Unix closed to other users.
#[test]
fn the_turn_secret_file_is_read_with_care() {
    use torque_rooms::ice::{MAX_SECRET, SECRET_SPACE, read_secret_file};
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("torque-rooms-secret");
    std::fs::create_dir_all(&dir).unwrap();
    let good = dir.join("good");
    std::fs::write(&good, b"s3cret\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&good, std::fs::Permissions::from_mode(0o640)).unwrap();
    }
    assert_eq!(read_secret_file(&good).unwrap(), b"s3cret");
    let huge = dir.join("huge");
    std::fs::write(&huge, vec![b'a'; MAX_SECRET + SECRET_SPACE + 1]).unwrap();
    // Closed to other users, so on Unix the length is what refuses it (the permission check comes first).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&huge, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(read_secret_file(&huge).unwrap_err().contains("longer than"));
    assert!(read_secret_file(&dir).is_err(), "a folder");
    assert!(read_secret_file(&dir.join("missing")).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&good, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_secret_file(&good).unwrap_err().contains("other users"));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
