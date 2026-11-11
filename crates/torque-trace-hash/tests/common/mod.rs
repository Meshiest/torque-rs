//! What the tests that join an engine client to a server over the memory link share: the addresses, the link that records
//! every datagram, a client on it, and the text of a session's golden lines.

use std::fmt::Write as _;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use torque::app::client::{ClientBuilder, ClientNet, GameClient};
use torque::app::files::GameFiles;
use torque::app::server::GameServer;
use torque::client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque::engine::Engine;
use torque_client::conn::ConnGame;
use torque_client::conn::engine_wire::EngineNonceKey;
use torque_server::net::NetGame;
use torque_trace_hash::rig::CHALLENGE_KEY;

/// Datagrams kept verbatim in the golden lines.
const VERBATIM: usize = 8;
pub const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
pub const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_001);

/// Every datagram of a session, in the order carried, and which way (true: to the server).
pub type Wire = Vec<(bool, Vec<u8>)>;

/// The memory link between a server at [`SERVER`] and a client at [`CLIENT`], with every datagram it carries kept.
pub struct Link {
    sent: DatagramQueue,
    incoming: DatagramQueue,
    pub wire: Wire,
}

impl Link {
    /// Carries what each side sent to the other.
    pub fn carry<G: NetGame>(&mut self, server: &GameServer<G>) {
        let Some(end) = server.memory_end() else {
            return;
        };
        for (to, data) in self.sent.borrow_mut().drain(..) {
            if SocketAddr::V4(to) == SERVER {
                self.wire.push((true, data.clone()));
                end.inject(SocketAddr::V4(CLIENT), &data);
            }
        }
        for (to, data) in end.take_sent() {
            if to == SocketAddr::V4(CLIENT) {
                self.wire.push((false, data.clone()));
                let SocketAddr::V4(from) = SERVER else {
                    continue;
                };
                self.incoming.borrow_mut().push_back((from, data));
            }
        }
    }

    /// One tick of `server`, then `client`'s step, the link carried around each.
    pub fn step<G: NetGame>(&mut self, server: &mut GameServer<G>, client: impl FnOnce()) {
        self.carry(server);
        server.tick();
        self.carry(server);
        client();
        self.carry(server);
    }
}

/// A client of conn game `C` made by `engine`, its scripts run, on a memory socket at [`CLIENT`] whose link is returned.
pub fn client<C: ConnGame>(
    engine: &Engine,
    files: &GameFiles,
) -> (GameClient<GameServer, C>, Link) {
    let mem = MemoryDatagrams {
        local_port: CLIENT.port(),
        ..MemoryDatagrams::default()
    };
    let link = Link {
        sent: mem.sent.clone(),
        incoming: mem.incoming.clone(),
        wire: Vec::new(),
    };
    let mut builder = ClientBuilder::<C>::from_engine_with_conn(
        engine,
        Some(files.mem_files()),
        ClientNet::External,
    )
    .expect("the client builds");
    builder.set_socket(Box::new(mem), NetPolicy::Loopback);
    builder.exec_entry().expect("the client's script");
    let mut client = builder.build();
    // The client draws its connect sequence and nonces at random; fixed, the datagrams are a function of the inputs.
    client
        .vm_mut()
        .set_extension(Rc::new(EngineNonceKey(CHALLENGE_KEY)));
    (client, link)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn digest(wire: &Wire, up: bool) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for (_, bytes) in wire.iter().filter(|(d, _)| *d == up) {
        h.update((bytes.len() as u32).to_le_bytes());
        h.update(bytes);
    }
    hex(&h.finalize())
}

/// The golden file's lines of the session `name`: the count and a digest each way, and the first datagrams.
pub fn render(name: &str, wire: &Wire) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{name} datagrams {}", wire.len());
    let _ = writeln!(out, "{name} to-server {}", digest(wire, true));
    let _ = writeln!(out, "{name} to-client {}", digest(wire, false));
    for (i, (up, bytes)) in wire.iter().take(VERBATIM).enumerate() {
        let way = if *up { "to-server" } else { "to-client" };
        let _ = writeln!(out, "{name} {i} {way} {}", hex(bytes));
    }
    out
}
