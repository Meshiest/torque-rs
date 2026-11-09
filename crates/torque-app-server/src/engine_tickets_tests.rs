//! Engine-mode tickets: bound to the challenge the server chose, and refused when presented again.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr};

use torque_engine::{Engine, EngineBuilder, Plugin, ProtocolKind};
use torque_net::connect::{AuthContext, AuthProvider, ConnectBlock, SharedSecret};
use torque_net::engine::bridge::{connect_of, ticket_context};
use torque_net::engine::{
    CloseReason, Event, Link, LinkFacts, NoRoom, NoStreams, Session, SessionConfig, TableLimits,
};
use torque_net::profile::HandshakeProfile;
use torque_server::net::transport::MemoryEnd;
use torque_ts_vm::host::MemFiles;

use crate::plugins::{MinimalPlugins, set_protocol};
use crate::server::{GameServer, Listen, ServerStart};

const LOCAL: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const SECRET: [u8; 16] = [5; 16];

/// Tickets that are bearer tokens: the provider checks nothing about the connection, so only the server's replay cache keeps
/// a copy from working twice.
#[derive(Debug)]
struct Bearer;

impl AuthProvider for Bearer {
    fn ticket(&self, _cx: &AuthContext) -> Result<Vec<u8>, String> {
        Ok(b"bearer".to_vec())
    }

    fn verify(
        &self,
        _from: SocketAddr,
        _cx: &AuthContext,
        ticket: &[u8],
    ) -> Result<Vec<u8>, String> {
        if ticket == b"bearer" {
            Ok(b"someone".to_vec())
        } else {
            Err("no such ticket".to_string())
        }
    }
}

/// The engine's auth: tickets bound to the connection (`true`, the shared secret) or bearer tokens.
#[derive(torque_engine::PluginId)]
#[plugin(id = "test.ticket-auth")]
struct TicketAuth(bool);

impl Plugin for TicketAuth {
    fn build(&self, engine: &mut EngineBuilder) {
        if self.0 {
            engine.net().auth(SharedSecret::new(SECRET, "player"));
        } else {
            engine.net().auth(Bearer);
        }
    }
}

fn engine(bound: bool) -> Engine {
    let mut builder = EngineBuilder::new();
    builder.add_plugins((MinimalPlugins, TicketAuth(bound)));
    set_protocol(&mut builder, ProtocolKind::Engine);
    builder.build().expect("the engine builds")
}

fn start(engine: &Engine, listen: Listen) -> GameServer {
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
    GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen,
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
    .expect("the server starts")
}

/// Where a test client's datagrams go, and where it reads the server's from.
trait Wire {
    fn send(&mut self, data: &[u8]);
    fn take(&mut self) -> Vec<Vec<u8>>;
}

struct MemoryWire<'a> {
    end: &'a MemoryEnd,
    from: SocketAddr,
}

impl Wire for MemoryWire<'_> {
    fn send(&mut self, data: &[u8]) {
        self.end.inject(self.from, data);
    }

    fn take(&mut self) -> Vec<Vec<u8>> {
        self.end
            .take_sent()
            .into_iter()
            .filter(|(to, _)| *to == self.from)
            .map(|(_, d)| d)
            .collect()
    }
}

struct UdpWire {
    socket: std::net::UdpSocket,
    to: SocketAddr,
}

impl UdpWire {
    fn new(to: SocketAddr) -> UdpWire {
        let socket = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("a socket");
        socket.set_nonblocking(true).expect("non-blocking");
        UdpWire { socket, to }
    }
}

impl Wire for UdpWire {
    fn send(&mut self, data: &[u8]) {
        let _ = self.socket.send_to(data, self.to);
    }

    fn take(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = vec![0; 2_048];
        while let Ok((n, _)) = self.socket.recv_from(&mut buf) {
            out.push(buf[..n].to_vec());
        }
        out
    }
}

struct WireLink<'a>(&'a mut dyn Wire, LinkFacts);

impl Link for WireLink<'_> {
    fn max_datagram(&self) -> usize {
        1_200
    }

    fn send(&mut self, datagram: &[u8]) -> Result<(), NoRoom> {
        self.0.send(datagram);
        Ok(())
    }

    fn facts(&self) -> LinkFacts {
        self.1
    }
}

/// A test client's connect: its link, what its connect carries early (a ticket made before any challenge), and its answer to
/// the challenge.
struct Try<'a> {
    wire: &'a mut dyn Wire,
    facts: LinkFacts,
    nonce: u64,
    early: Vec<u8>,
    answer: &'a dyn Fn(u64) -> Vec<u8>,
}

/// A client of `engine` whose connect carries `early` as its ticket.
fn client(engine: &Engine, facts: LinkFacts, nonce: u64, early: Vec<u8>) -> Session {
    let profile = HandshakeProfile::default();
    let block = ConnectBlock {
        manifest: Some(engine.net_manifest(&profile)),
        ..Default::default()
    };
    let mut connect = connect_of(&profile, Some(&block), b"", &[]).expect("a connect");
    connect.ticket = early;
    let game = String::from_utf8_lossy(&profile.game).into_owned();
    Session::client(SessionConfig::new(&game), facts, connect, nonce, 0).expect("a client")
}

/// Connects a client of `engine` as `t` says: how the session ended, or `None` when the server accepted.
fn connect(server: &mut GameServer, engine: &Engine, t: Try<'_>) -> Option<CloseReason> {
    let mut client = client(engine, t.facts, t.nonce, t.early);
    for tick in 0..300u32 {
        let now = tick * 32;
        client.poll(&mut WireLink(&mut *t.wire, t.facts), &mut NoStreams, now);
        server.tick();
        if !t.facts.validated {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        for d in t.wire.take() {
            client.receive(&d, now);
        }
        while let Some(event) = client.next_event() {
            match event {
                Event::Challenge(n) => client.answer((t.answer)(n)).expect("an answer"),
                Event::Accepted(_) => return None,
                Event::Closed(why) => return Some(why),
                _ => {}
            }
        }
    }
    panic!("the connect neither ended nor was accepted");
}

fn refused(why: Option<CloseReason>, text: &str) {
    match why {
        Some(CloseReason::Rejected { text: got, .. }) => assert!(got.contains(text), "{got}"),
        other => panic!("not refused: {other:?}"),
    }
}

/// The shared secret's ticket for the server at `at` and challenge `n`.
fn bound_ticket(at: SocketAddr, n: u64) -> Vec<u8> {
    SharedSecret::new(SECRET, "player")
        .ticket(&ticket_context(at, n, None))
        .expect("a ticket")
}

#[test]
fn over_a_validated_link_a_ticket_captured_on_one_connection_fails_on_another() {
    let engine = engine(true);
    let mut server = start(&engine, Listen::Memory(LOCAL));
    let at = server.address().expect("bound");
    let end = server.memory_end().expect("memory").clone();
    let captured = RefCell::new(Vec::new());
    let honest = |n: u64| {
        let t = bound_ticket(at, n);
        captured.borrow_mut().clone_from(&t);
        t
    };
    let mut a = MemoryWire {
        end: &end,
        from: SocketAddr::from(([10, 0, 0, 1], 4_000)),
    };
    let try_a = Try {
        wire: &mut a,
        facts: LinkFacts::MEMORY,
        nonce: 1,
        early: Vec::new(),
        answer: &honest,
    };
    assert_eq!(
        connect(&mut server, &engine, try_a),
        None,
        "a ticket made for its challenge"
    );
    // Another connection presents the captured ticket for its own challenge, with the victim's own nonce: what a client
    // chooses binds nothing.
    let stolen = captured.borrow().clone();
    let mut b = MemoryWire {
        end: &end,
        from: SocketAddr::from(([10, 0, 0, 2], 4_000)),
    };
    let try_b = Try {
        wire: &mut b,
        facts: LinkFacts::MEMORY,
        nonce: 1,
        early: Vec::new(),
        answer: &|_| stolen.clone(),
    };
    refused(connect(&mut server, &engine, try_b), "");
    // A ticket in the connect itself, made before any challenge, counts for nothing.
    let mut c = MemoryWire {
        end: &end,
        from: SocketAddr::from(([10, 0, 0, 3], 4_000)),
    };
    let early = bound_ticket(at, 0);
    let try_c = Try {
        wire: &mut c,
        facts: LinkFacts::MEMORY,
        nonce: 3,
        early,
        answer: &|_| Vec::new(),
    };
    refused(connect(&mut server, &engine, try_c), "");
}

#[test]
fn over_a_validated_link_a_ticket_presented_again_within_the_window_is_refused() {
    let engine = engine(false);
    let mut server = start(&engine, Listen::Memory(LOCAL));
    let end = server.memory_end().expect("memory").clone();
    let bearer = |_: u64| b"bearer".to_vec();
    let mut a = MemoryWire {
        end: &end,
        from: SocketAddr::from(([10, 0, 0, 1], 4_000)),
    };
    let try_a = Try {
        wire: &mut a,
        facts: LinkFacts::MEMORY,
        nonce: 1,
        early: Vec::new(),
        answer: &bearer,
    };
    assert_eq!(connect(&mut server, &engine, try_a), None);
    let mut b = MemoryWire {
        end: &end,
        from: SocketAddr::from(([10, 0, 0, 2], 4_000)),
    };
    let try_b = Try {
        wire: &mut b,
        facts: LinkFacts::MEMORY,
        nonce: 2,
        early: Vec::new(),
        answer: &bearer,
    };
    refused(connect(&mut server, &engine, try_b), "used already");
}

#[test]
fn over_plain_udp_a_captured_ticket_fails_and_a_replay_is_refused() {
    let udp = Listen::Udp {
        address: Ipv4Addr::LOCALHOST,
        port: 0,
        public: false,
    };
    for bound in [true, false] {
        let engine = engine(bound);
        let mut server = start(&engine, udp.clone());
        let at = server.address().expect("bound");
        let captured = RefCell::new(Vec::new());
        let honest = |n: u64| {
            let t = if bound {
                bound_ticket(at, n)
            } else {
                b"bearer".to_vec()
            };
            captured.borrow_mut().clone_from(&t);
            t
        };
        let mut a = UdpWire::new(at);
        let try_a = Try {
            wire: &mut a,
            facts: LinkFacts::PLAIN,
            nonce: 1,
            early: Vec::new(),
            answer: &honest,
        };
        assert_eq!(connect(&mut server, &engine, try_a), None, "bound: {bound}");
        let stolen = captured.borrow().clone();
        let mut b = UdpWire::new(at);
        let try_b = Try {
            wire: &mut b,
            facts: LinkFacts::PLAIN,
            nonce: 1,
            early: Vec::new(),
            answer: &|_| stolen.clone(),
        };
        refused(
            connect(&mut server, &engine, try_b),
            if bound { "" } else { "used already" },
        );
    }
}

#[test]
fn sources_that_never_answer_their_challenge_keep_the_waiting_connects_bounded() {
    let engine = engine(true);
    let mut server = start(&engine, Listen::Memory(LOCAL));
    let end = server.memory_end().expect("memory").clone();
    let limit = TableLimits::default().pending;
    for k in 0..(limit * 3) {
        // Every source from a network of its own, so only the cap on all pending sessions holds them.
        let from = SocketAddr::from(([10, (k / 250) as u8, (k % 250) as u8, 1], 4_000));
        let mut wire = MemoryWire { end: &end, from };
        let mut c = client(&engine, LinkFacts::MEMORY, k as u64, Vec::new());
        c.poll(
            &mut WireLink(&mut wire, LinkFacts::MEMORY),
            &mut NoStreams,
            0,
        );
        server.tick();
        drop(wire.take());
        let sh = server.shared.borrow();
        let host = sh.engine_host.as_ref().expect("an engine host");
        assert!(
            host.challenged() <= limit,
            "{} connects wait",
            host.challenged()
        );
        assert!(
            host.table.pending() <= limit,
            "{} sessions pending",
            host.table.pending()
        );
    }
    let sh = server.shared.borrow();
    assert_eq!(
        sh.engine_host
            .as_ref()
            .expect("an engine host")
            .challenged(),
        limit
    );
}
