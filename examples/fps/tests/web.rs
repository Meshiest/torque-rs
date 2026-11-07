//! Browsers and desktops in one game, over the loopback address: a dedicated server on `--transport both`, a client over
//! UDP and a client over a game socket (WebSocket, as a browser's). Each sees the other's figure, chat goes both ways and
//! shots land both ways. Then hostile peers while a game runs (oversized and flooding messages, text, HTTP abuse, a crowd
//! of connections): each is closed and the server's ticks keep their time. A web peer keeps its address when it comes
//! back (a ban holds), and the per-address limits hold across its sockets.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use torque::app::server::GameServer;
use torque::client::net::Datagrams;
use torque::client::net::web_socket::WebSocketDatagrams;
use torque::server::net::web::WebSetup;
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;
use torque_example_fps::server::{ArenaServer, ServerOptions, Transports};

fn server(transports: Transports) -> GameServer {
    ArenaServer::start_dedicated_on(
        &GameFiles::bundled().unwrap(),
        &ServerOptions {
            address: Ipv4Addr::LOCALHOST,
            port: 0,
            name: "Arena".to_string(),
            mission: "fixtures/floor.mis".to_string(),
            public: false,
        },
        transports,
        WebSetup::default(),
    )
    .unwrap()
}

fn options(name: &str) -> ClientOptions {
    ClientOptions {
        net: NetMode::Loopback,
        name: name.to_string(),
        port: 0,
        ..ClientOptions::default()
    }
}

/// A game socket to `server`, opened on a thread while the server's transport is serviced here.
fn socket(server: &mut GameServer) -> WebSocketDatagrams {
    let addr = server.address().unwrap();
    let SocketAddr::V4(v4) = addr else {
        panic!("an IPv4 server");
    };
    let h = thread::spawn(move || WebSocketDatagrams::connect(addr, v4));
    while !h.is_finished() {
        server.service();
        thread::sleep(Duration::from_millis(1));
    }
    h.join().unwrap().unwrap()
}

fn udp_client(server: &GameServer, name: &str) -> ArenaGame {
    let mut c = ArenaGame::new(GameFiles::bundled().unwrap(), options(name)).unwrap();
    let addr = server.address().unwrap().to_string();
    c.call("connectTo", &[&addr]).unwrap();
    c
}

fn web_client(server: &mut GameServer, name: &str) -> ArenaGame {
    let socket = socket(server);
    let addr = socket.server().to_string();
    let mut c = ArenaGame::with_socket(
        GameFiles::bundled().unwrap(),
        options(name),
        Box::new(socket),
    )
    .unwrap();
    c.call("connectTo", &[&addr]).unwrap();
    c
}

/// `ms` of the server's ticks and the clients' steps.
fn run(server: &mut GameServer, clients: &mut [&mut ArenaGame], ms: u32) {
    for _ in 0..ms / 32 {
        server.tick();
        server.service();
        for c in clients.iter_mut() {
            c.step(32);
        }
    }
}

fn players(game: &ArenaGame) -> usize {
    game.with_world(|w| {
        w.records
            .ghosts
            .values()
            .filter(|g| g.class == Some(torque::ghosts::TgeClass::Player))
            .count()
    })
    .unwrap_or(0)
}

fn status(game: &mut ArenaGame) -> String {
    game.call("eval", &["return MainStatus.getValue();"])
        .unwrap()
}

fn shoot(server: &mut GameServer, from: &mut ArenaGame, other: &mut ArenaGame) {
    from.call("fire", &["1"]).unwrap();
    run(server, &mut [from, other], 96);
    from.call("fire", &["0"]).unwrap();
    run(server, &mut [from, other], 1500);
}

#[test]
fn a_browser_and_a_desktop_play_one_game() {
    let mut s = server(Transports::Both);
    let mut ann = udp_client(&s, "Ann");
    for _ in 0..200 {
        run(&mut s, &mut [&mut ann], 32);
        if ann.in_game() {
            break;
        }
    }
    assert!(ann.in_game(), "the desktop player joined over UDP");
    let mut bob = web_client(&mut s, "Bob");
    for _ in 0..200 {
        run(&mut s, &mut [&mut ann, &mut bob], 32);
        if bob.in_game() {
            break;
        }
    }
    assert!(bob.in_game(), "the browser player joined over WebSocket");
    run(&mut s, &mut [&mut ann, &mut bob], 1000);
    assert_eq!(players(&ann), 2, "the desktop sees both figures");
    assert_eq!(players(&bob), 2, "the browser sees both figures");
    assert!(ann.own_look().is_some() && bob.own_look().is_some());

    ann.call("say", &["hello from the desktop"]).unwrap();
    bob.call("say", &["hello from the browser"]).unwrap();
    run(&mut s, &mut [&mut ann, &mut bob], 500);
    let lines = (ann.global("$Chat::Last"), bob.global("$Chat::Last"));
    // Each saw both lines; the last is whichever the server sent last.
    for (who, last) in [("Ann", &lines.0), ("Bob", &lines.1)] {
        assert!(
            last == "Ann: hello from the desktop" || last == "Bob: hello from the browser",
            "{who} saw {last:?}"
        );
    }
    bob.call("say", &["over"]).unwrap();
    run(&mut s, &mut [&mut ann, &mut bob], 500);
    assert_eq!(
        ann.global("$Chat::Last"),
        "Bob: over",
        "the browser's chat reached the desktop"
    );
    ann.call("say", &["out"]).unwrap();
    run(&mut s, &mut [&mut ann, &mut bob], 500);
    assert_eq!(
        bob.global("$Chat::Last"),
        "Ann: out",
        "the desktop's chat reached the browser"
    );

    // The two face each other: each shot lands.
    shoot(&mut s, &mut ann, &mut bob);
    assert_eq!(
        ann.global("$Arena::Hits"),
        "1",
        "the desktop's bolt hit the browser's figure"
    );
    let health: i32 = bob.global("$Arena::Health").parse().unwrap();
    assert!(health < 100, "the browser player was hurt: {health}");
    shoot(&mut s, &mut bob, &mut ann);
    assert_eq!(
        bob.global("$Arena::Hits"),
        "1",
        "the browser's bolt hit the desktop's figure"
    );
    let health: i32 = ann.global("$Arena::Health").parse().unwrap();
    assert!(health < 100, "the desktop player was hurt: {health}");
}

/// Hostile peers, each on its own thread: what they saw of their connections being closed.
fn attack(addr: SocketAddr, until: Instant) -> Vec<String> {
    let SocketAddr::V4(v4) = addr else {
        return vec!["no IPv4".to_string()];
    };
    let mut seen = Vec::new();
    let mut ws = |what: &str, f: &dyn Fn(&mut WebSocketDatagrams)| {
        let Ok(mut s) = WebSocketDatagrams::connect(addr, v4) else {
            seen.push(format!("{what}: refused"));
            return;
        };
        f(&mut s);
        let start = Instant::now();
        while s.is_open() && start.elapsed() < Duration::from_secs(5) {
            let _ = s.recv();
            s.send_raw(&[0]);
            thread::sleep(Duration::from_millis(5));
        }
        seen.push(format!(
            "{what}: {}",
            if s.is_open() { "open" } else { "closed" }
        ));
    };
    ws("oversized", &|s| {
        s.send_raw(&vec![1; 64 << 10]);
    });
    ws("flood", &|s| {
        for _ in 0..3000 {
            s.send_raw(&[1; 32]);
        }
    });
    ws("text", &|s| {
        s.send_text("hello");
    });
    // A binary frame left open, then empty continuation frames as fast as the link takes them.
    seen.push(format!("continuations: {}", continuations(addr)));
    let http = |raw: &[u8]| -> String {
        let Ok(mut t) = TcpStream::connect(addr) else {
            return "refused".to_string();
        };
        let _ = t.set_read_timeout(Some(Duration::from_secs(15)));
        let _ = t.write_all(raw);
        let mut out = Vec::new();
        let _ = t.read_to_end(&mut out);
        String::from_utf8_lossy(out.get(..12).unwrap_or(&out)).into_owned()
    };
    seen.push(format!(
        "huge head: {}",
        http(format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(20_000)).as_bytes())
    ));
    seen.push(format!(
        "post: {}",
        http(b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 3\r\n\r\nabc")
    ));
    seen.push(format!(
        "climb: {}",
        http(b"GET /../../Cargo.toml HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
    ));
    // A crowd of connections that say nothing, held open.
    let crowd: Vec<TcpStream> = (0..40)
        .filter_map(|_| TcpStream::connect(addr).ok())
        .collect();
    while Instant::now() < until {
        thread::sleep(Duration::from_millis(20));
    }
    drop(crowd);
    seen
}

/// Opens a game socket by hand and floods it with empty continuation frames for up to two seconds: "closed" when the server
/// closed it.
fn continuations(addr: SocketAddr) -> &'static str {
    let Ok(mut s) = TcpStream::connect(addr) else {
        return "refused";
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = s.set_write_timeout(Some(Duration::from_millis(200)));
    let request = "GET /ws HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                   Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: AwMDAwMDAwMDAwMDAwMDAw==\r\n\r\n";
    if s.write_all(request.as_bytes()).is_err() {
        return "refused";
    }
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match s.read(&mut b) {
            Ok(1) => head.push(b[0]),
            _ => return "refused",
        }
    }
    if !head.starts_with(b"HTTP/1.1 101 ") {
        eprintln!("continuations: {}", String::from_utf8_lossy(&head));
        return "refused";
    }
    let _ = s.write_all(&[0x02, 0x80, 1, 2, 3, 4]);
    let chunk: Vec<u8> = [0x00, 0x80, 1, 2, 3, 4].repeat(4096);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        match s.write(&chunk) {
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return "closed",
        }
    }
    let mut rest = [0u8; 256];
    loop {
        match s.read(&mut rest) {
            Ok(0) | Err(_) => return "closed",
            Ok(_) => {}
        }
    }
}

/// One tick of the server, timed, then its wait serviced as a dedicated server's is (2 ms steps to the tick's end).
fn paced_tick(
    s: &mut GameServer,
    client: &mut ArenaGame,
    longest_service: &mut Duration,
) -> Duration {
    let start = Instant::now();
    s.tick();
    let took = start.elapsed();
    client.step(32);
    while start.elapsed() < Duration::from_millis(32) {
        let pass = Instant::now();
        s.service();
        *longest_service = (*longest_service).max(pass.elapsed());
        thread::sleep(Duration::from_millis(2));
    }
    took
}

#[test]
fn hostile_peers_are_closed_and_the_ticks_keep_their_time() {
    let mut s = server(Transports::Both);
    let mut ann = udp_client(&s, "Ann");
    for _ in 0..200 {
        run(&mut s, &mut [&mut ann], 32);
        if ann.in_game() {
            break;
        }
    }
    assert!(ann.in_game());
    let mut service = Duration::ZERO;
    let mut quiet: Vec<Duration> = (0..60)
        .map(|_| paced_tick(&mut s, &mut ann, &mut service))
        .collect();
    let mut busy_service = Duration::ZERO;
    let sim_before = s.vm().sim_time();
    let addr = s.address().unwrap();
    let until = Instant::now() + Duration::from_secs(4);
    let h = thread::spawn(move || attack(addr, until));
    let mut busy = Vec::new();
    while !h.is_finished() {
        busy.push(paced_tick(&mut s, &mut ann, &mut busy_service));
    }
    let seen = h.join().unwrap();
    let ticks = busy.len() as u32;
    assert_eq!(
        s.vm().sim_time().wrapping_sub(sim_before),
        32 * ticks,
        "every tick is 32 ms of the game"
    );
    for line in &seen {
        assert!(
            line.ends_with("closed") || line.ends_with(": refused") || line.contains("HTTP/1.1 4"),
            "{seen:?}"
        );
    }
    assert!(
        seen.iter().filter(|l| l.ends_with("closed")).count() == 4,
        "{seen:?}"
    );
    quiet.sort_unstable();
    busy.sort_unstable();
    let median = |v: &[Duration]| v[v.len() / 2];
    let p90 = |v: &[Duration]| v[v.len() * 9 / 10];
    println!(
        "tick median {:?} -> {:?}, 90th {:?} -> {:?} over {} ticks",
        median(&quiet),
        median(&busy),
        p90(&quiet),
        p90(&busy),
        busy.len()
    );
    assert!(
        median(&busy) <= median(&quiet) * 2 + Duration::from_millis(2),
        "the median tick went from {:?} to {:?}",
        median(&quiet),
        median(&busy)
    );
    assert!(
        p90(&busy) < Duration::from_millis(32),
        "the 90th tick took {:?}",
        p90(&busy)
    );
    println!("longest service pass {service:?} -> {busy_service:?}");
    assert!(
        busy_service < Duration::from_millis(16),
        "a pass between ticks took {busy_service:?}"
    );
    run(&mut s, &mut [&mut ann], 500);
    assert!(ann.in_game(), "the desktop player is still in the game");
}

#[test]
fn a_banned_browser_that_comes_back_is_still_refused() {
    let mut s = server(Transports::Web);
    let mut bob = web_client(&mut s, "Bob");
    for _ in 0..200 {
        run(&mut s, &mut [&mut bob], 32);
        if bob.in_game() {
            break;
        }
    }
    assert!(bob.in_game());
    let address = s
        .call("eval", &["return ClientGroup.getObject(0).getAddress();"])
        .unwrap();
    // `IP:a.b.c.d:port`: the ban is on the address without its port.
    let ip = address
        .rsplit_once(':')
        .map(|(ip, _)| ip.to_string())
        .unwrap();
    assert!(
        ip.starts_with("IP:24") || ip.starts_with("IP:25"),
        "{address}"
    );
    s.call(
        "eval",
        &[&format!(
            "$Banned = \"{ip}:\"; function GameConnection::onConnectRequest(%client, %address, %name) \
             {{ if (strstr(%address, $Banned) == 0) return \"CR_BANNED\"; return \"\"; }}"
        )],
    )
    .unwrap();
    // The browser leaves: its socket closes, and it comes back on a new one.
    drop(bob);
    run(&mut s, &mut [], 320);
    assert_eq!(
        s.call("eval", &["return ClientGroup.getCount();"]).unwrap(),
        "0",
        "the connection went with its socket, not at its timeout"
    );
    let mut again = web_client(&mut s, "Bob");
    for _ in 0..200 {
        run(&mut s, &mut [&mut again], 32);
        if status(&mut again).contains("turned us away") {
            break;
        }
    }
    assert!(!again.in_game());
    assert!(
        status(&mut again).contains("CR_BANNED"),
        "{}",
        status(&mut again)
    );
}

#[test]
fn the_per_address_limits_hold_across_a_peers_sockets() {
    let mut s = server(Transports::Web);
    run(&mut s, &mut [], 64);
    let sim_start = s.vm().sim_time();
    let types = torque::net::handshake::HandshakeTypes::TORQUE;
    let mut replies = 0;
    let mut seq = 0u32;
    // Socket after socket from one address, each closed before the next: within one second of the game, the server answers
    // no more challenges than its per-address cap.
    for _ in 0..6 {
        let mut ws = socket(&mut s);
        let to = ws.server();
        for _ in 0..3 {
            seq += 1;
            assert!(ws.send_to(to, &types.challenge_request(seq)));
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(30) {
            s.service();
            thread::sleep(Duration::from_millis(1));
        }
        s.tick();
        s.service();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(30) {
            if ws.recv().is_some() {
                replies += 1;
            }
            s.service();
            thread::sleep(Duration::from_millis(1));
        }
    }
    assert!(
        s.vm().sim_time().wrapping_sub(sim_start) < 1000,
        "the sockets all came within a second of the game"
    );
    assert_eq!(
        replies,
        torque::net::cap::NET_CHALLENGES_PER_ADDR_PER_S,
        "18 challenges over 6 sockets of one address"
    );
}
