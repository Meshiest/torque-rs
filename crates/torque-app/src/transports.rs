//! The transports a client can connect over, each registered by a plugin under an id, the same ids as the server's
//! ([`torque_app_server::transports`]).
//!
//! | Plugin | Id | Plugin id | Needs | Connects with |
//! |--------|----|-----------|-------|---------------|
//! | [`UdpClientTransportPlugin`] | `udp` | `torque.transport-udp-client` | native | [`UdpTarget`]: a UDP socket, on the loopback address or as TGE binds one |
//! | [`MemoryClientTransportPlugin`] | `memory` | `torque.transport-memory-client` | - | [`MemoryTarget`]: an in-memory socket linked to a server in this process |
//! | `WebClientTransportPlugin` | `web` | `torque.transport-web-client` | `web` feature | `WebTarget`: a game socket over WebSocket (a data channel beside it with the `webrtc` feature) |
//! | `RoomsClientTransportPlugin` | `rooms` | `torque.transport-rooms-client` | `p2p` feature, browser | `web_join::Target::Room`: a player's host, found by its room's code |
//! | `RelayClientTransportPlugin` | `relay` | `torque.transport-relay-client` | `p2p` feature, browser | `web_join::Target::Local`, `Page` and `WebTransport`: the links the page holds |
//! | [`InstantNetClientTransportPlugin`] | `instant-net` | `torque.transport-instant-net-client` | - | [`InstantNet`]: an in-process network every datagram crosses at once |
//! | `QuicClientTransportPlugin` | `quic` | `torque.transport-quic-client` | `quic` feature, native | `QuicTarget`: a QUIC connection to a server's QUIC endpoint, its identity pinned or trusted on first use; a WebTransport session on it when asked |
//!
//! [`ClientTransportPlugins`] is the group of the ones a build has; [`DefaultPlugins`](crate::plugins::DefaultPlugins) has it,
//! with the server's ([`ServerTransportPlugins`]). The in-process network's pair ([`InstantNetClientTransportPlugin`],
//! [`InstantNetServerTransportPlugin`]) is in neither: a harness whose pictures must not depend on the system's loopback adds
//! it to the engine it builds (see [`instant_net`]).
//!
//! **Connecting.** A client's socket is opened by the transport registered under an id, given a target of the type that
//! transport takes. [`ClientNet`](crate::client::ClientNet) picks the engine's own: `Loopback` and `Lan` are `udp`,
//! `Memory` is `memory`. A game's own transport (or a browser's link) is used with [`ClientBuilder::connect`] on a client
//! made with [`ClientNet::External`](crate::client::ClientNet::External), which names the id and hands the transport its
//! target. An id nobody registered, such as one whose plugin was turned off, is refused with the ids that are registered
//! and, for the engine's own, the plugin that registers it.
//!
//! [`ClientBuilder::connect`]: crate::client::ClientBuilder::connect

use std::any::{Any, type_name};
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

pub use torque_app_server::transports::{
    BoundTransport, CustomListen, EngineServerTransports, MemoryServerTransportPlugin, ServerLink,
    ServerTransportPlugins, UdpServerTransportPlugin, bound_transport, open_listen, unregistered,
};
use torque_client::net::{DatagramQueue, Datagrams, MemoryDatagrams, NetPolicy, UdpDatagrams};
use torque_engine::{Engine, EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder};

use crate::server::GameServer;
use torque_server::net::NetGame;

#[path = "instant_net.rs"]
pub mod instant_net;
pub use instant_net::{
    INSTANT_NET, InstantNet, InstantNetClientTransportPlugin, InstantNetServerTransportPlugin,
};

/// What a transport opened for a client: the socket, the policy it runs under, and what else it has to say.
pub struct ClientLink {
    pub socket: Box<dyn Datagrams>,
    pub policy: NetPolicy,
    pub(crate) link: Option<MemoryLink>,
    extra: Option<Box<dyn Any>>,
}

impl ClientLink {
    /// A link that is `socket` under `policy`.
    pub fn new(socket: Box<dyn Datagrams>, policy: NetPolicy) -> ClientLink {
        ClientLink {
            socket,
            policy,
            link: None,
            extra: None,
        }
    }

    /// The link with `extra`, something the transport hands back to whoever connected ([`LinkInfo::take`]): a browser's
    /// link state, for the page to show.
    pub fn with_extra(mut self, extra: impl Any) -> ClientLink {
        self.extra = Some(Box::new(extra));
        self
    }
}

/// What a transport handed back when it connected ([`ClientLink::with_extra`]).
#[derive(Default)]
pub struct LinkInfo(Option<Box<dyn Any>>);

impl LinkInfo {
    /// The value of type `T` the transport handed back, if it did.
    pub fn take<T: Any>(self) -> Option<T> {
        self.0?.downcast().ok().map(|b| *b)
    }
}

impl fmt::Debug for LinkInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("LinkInfo").field(&self.0.is_some()).finish()
    }
}

impl From<Option<Box<dyn Any>>> for LinkInfo {
    fn from(extra: Option<Box<dyn Any>>) -> LinkInfo {
        LinkInfo(extra)
    }
}

impl ClientLink {
    pub(crate) fn split(self) -> (Box<dyn Datagrams>, NetPolicy, Option<MemoryLink>, LinkInfo) {
        (self.socket, self.policy, self.link, LinkInfo(self.extra))
    }
}

/// Opens a registered transport for a target of the type it takes.
pub type OpenClientTransport = dyn Fn(&dyn Any) -> Result<ClientLink, String>;

/// A transport registered on the engine: its id, the target it takes and how it is opened.
#[derive(Clone)]
pub struct ClientTransportEntry {
    id: String,
    target: &'static str,
    open: Rc<OpenClientTransport>,
}

impl ClientTransportEntry {
    /// Its id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The name of the target type it takes.
    pub fn target(&self) -> &'static str {
        self.target
    }
}

impl fmt::Debug for ClientTransportEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientTransportEntry")
            .field("id", &self.id)
            .field("target", &self.target)
            .finish()
    }
}

/// The engine builder's call that registers a transport a client can connect over.
pub trait EngineClientTransports {
    /// Registers the transport `open` opens under `id`, for targets of type `T`. A second transport under an id already
    /// registered is skipped, with a warning on each client's console.
    fn add_client_transport<T: Any>(
        &mut self,
        id: &str,
        open: impl Fn(&T) -> Result<ClientLink, String> + 'static,
    ) -> &mut EngineBuilder;
}

impl EngineClientTransports for EngineBuilder {
    fn add_client_transport<T: Any>(
        &mut self,
        id: &str,
        open: impl Fn(&T) -> Result<ClientLink, String> + 'static,
    ) -> &mut EngineBuilder {
        let name = id.to_owned();
        let open: Rc<OpenClientTransport> =
            Rc::new(move |target| match target.downcast_ref::<T>() {
                Some(target) => open(target),
                None => Err(format!(
                    "the transport {name:?} connects to a {}",
                    type_name::<T>()
                )),
            });
        self.push(ClientTransportEntry {
            id: id.to_owned(),
            target: type_name::<T>(),
            open,
        })
    }
}

/// The ids of the transports `engine` has for clients, in the order they were registered.
pub fn client_transport_ids(engine: &Engine) -> Vec<&str> {
    engine
        .list::<ClientTransportEntry>()
        .iter()
        .map(ClientTransportEntry::id)
        .collect()
}

/// The lines for the console of a client made by `engine`: a transport registered under an id that was already taken.
pub fn client_transport_warnings(engine: &Engine) -> Vec<String> {
    let entries = engine.list::<ClientTransportEntry>();
    let mut out = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if entries[..i].iter().any(|e| e.id == entry.id) {
            out.push(format!(
                "a second transport named {:?} was skipped: the first registered serves that id",
                entry.id
            ));
        }
    }
    out
}

/// Opens the transport `id` for a client made by `engine`, to `target`.
pub fn open_connect(engine: &Engine, id: &str, target: &dyn Any) -> Result<ClientLink, String> {
    let Some(entry) = engine
        .list::<ClientTransportEntry>()
        .iter()
        .find(|e| e.id == id)
    else {
        return Err(unregistered(
            "connect over",
            id,
            builtin_client_plugin(id),
            &client_transport_ids(engine),
        ));
    };
    (entry.open)(target)
}

/// The plugin of the engine's own that registers `id` for clients.
fn builtin_client_plugin(id: &str) -> Option<&'static str> {
    Some(match id {
        "udp" => "torque.transport-udp-client (not in a browser)",
        "memory" => "torque.transport-memory-client",
        "web" => "torque.transport-web-client (the `web` feature)",
        "rooms" => "torque.transport-rooms-client (the `p2p` feature, in a browser)",
        "relay" => "torque.transport-relay-client (the `p2p` feature, in a browser)",
        "instant-net" => "torque.transport-instant-net-client",
        "quic" => "torque.transport-quic-client (the `quic` feature, not in a browser)",
        _ => return None,
    })
}

/// A client that can connect over a registered transport: a [`ClientBuilder`](crate::client::ClientBuilder) being made, or a
/// [`GameClient`](crate::client::GameClient) already running (not while it plays a game alone).
pub trait Connect {
    /// Connects over the transport registered under `id`, to `target` (of the type that transport takes).
    fn connect_dyn(&mut self, id: &str, target: &dyn Any) -> Result<LinkInfo, String>;
}

/// The name a client gives its server over a transport whose link goes to one server only.
pub const SERVER_NAME: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_000);

// ---- udp
// ----------------------------------------------------------------------------------------------------------------

/// Where a client's UDP socket binds and what it reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpTarget {
    /// A socket on `127.0.0.1` that reads only loopback datagrams (tests, a game for one).
    Loopback,
    /// A socket bound as TGE binds one (every address, with a loopback twin so a server on `127.0.0.1` hears its LAN
    /// query), under the engine's internet policy.
    Lan,
}

/// UDP datagrams: the socket the server query and the connection share. Registers `udp`, for [`UdpTarget`].
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-udp-client")]
pub struct UdpClientTransportPlugin;

impl Plugin for UdpClientTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_client_transport("udp", |target: &UdpTarget| {
            let (ip, policy, twin) = match target {
                UdpTarget::Loopback => (Ipv4Addr::LOCALHOST, NetPolicy::Loopback, false),
                UdpTarget::Lan => (Ipv4Addr::UNSPECIFIED, NetPolicy::Internet, true),
            };
            let socket =
                UdpDatagrams::open(ip, 0, twin).map_err(|e| format!("the client's socket: {e}"))?;
            Ok(ClientLink::new(Box::new(socket), policy))
        });
    }
}

// ---- memory
// ----------------------------------------------------------------------------------------------------------------

/// The server an in-memory socket at `client` is linked to: one in this process, listening at `server`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryTarget {
    pub client: SocketAddrV4,
    pub server: SocketAddr,
}

/// The datagrams between an in-memory client socket and the transport of a server in the same process.
pub(crate) struct MemoryLink {
    sent: DatagramQueue,
    incoming: DatagramQueue,
    client: SocketAddrV4,
    server: SocketAddr,
}

impl MemoryLink {
    /// An in-memory socket at `client` and its link to a server at `server`.
    pub(crate) fn new(client: SocketAddrV4, server: SocketAddr) -> (MemoryLink, MemoryDatagrams) {
        let link = MemoryLink {
            sent: DatagramQueue::default(),
            incoming: DatagramQueue::default(),
            client,
            server,
        };
        let socket = MemoryDatagrams {
            sent: Rc::clone(&link.sent),
            incoming: Rc::clone(&link.incoming),
            local_port: client.port(),
        };
        (link, socket)
    }

    pub(crate) fn carry<G: NetGame>(&self, server: &GameServer<G>) {
        let Some(end) = server.memory_end() else {
            return;
        };
        let client = SocketAddr::V4(self.client);
        for (to, data) in self.sent.borrow_mut().drain(..) {
            if SocketAddr::V4(to) == self.server {
                end.inject(client, &data);
            }
        }
        let SocketAddr::V4(from) = self.server else {
            return;
        };
        let mut incoming = self.incoming.borrow_mut();
        for (to, data) in end.take_sent() {
            if to == client {
                incoming.push_back((from, data));
            }
        }
    }

    /// Carries the datagrams of several clients that share `server`: each client's go in, and what the server sent goes to the client
    /// it is addressed to (`carry` of one link would take and drop the datagrams of the others).
    pub(crate) fn carry_all<G: NetGame>(links: &[&MemoryLink], server: &GameServer<G>) {
        let Some(end) = server.memory_end() else {
            return;
        };
        for link in links {
            let client = SocketAddr::V4(link.client);
            for (to, data) in link.sent.borrow_mut().drain(..) {
                if SocketAddr::V4(to) == link.server {
                    end.inject(client, &data);
                }
            }
        }
        for (to, data) in end.take_sent() {
            for link in links {
                if to == SocketAddr::V4(link.client)
                    && let SocketAddr::V4(from) = link.server
                {
                    link.incoming.borrow_mut().push_back((from, data.clone()));
                }
            }
        }
    }

    pub(crate) fn clear(&self) {
        self.sent.borrow_mut().clear();
        self.incoming.borrow_mut().clear();
    }
}

/// An in-memory socket linked to a server in the same process, whose transport is the server's `memory` one: every step
/// carries the datagrams each sent to the other. Registers `memory`, for [`MemoryTarget`].
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-memory-client")]
pub struct MemoryClientTransportPlugin;

impl Plugin for MemoryClientTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_client_transport("memory", |target: &MemoryTarget| {
            let (link, socket) = MemoryLink::new(target.client, target.server);
            let mut opened = ClientLink::new(Box::new(socket), NetPolicy::Loopback);
            opened.link = Some(link);
            Ok(opened)
        });
    }
}

// ---- web
// ----------------------------------------------------------------------------------------------------------------

/// Where a game socket over WebSocket goes.
#[cfg(feature = "web")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebTarget {
    /// A server's web transport at this address: the socket opens `ws://<address>/ws`.
    #[cfg(not(target_arch = "wasm32"))]
    Address(SocketAddr),
    /// A server's web transport at this `ws://` or `wss://` URL.
    #[cfg(target_arch = "wasm32")]
    Url(String),
}

/// A game socket over WebSocket to a server's web transport, with a WebRTC data channel beside it when the server offers one
/// (the `webrtc` feature). The link names its server [`SERVER_NAME`]. Registers `web`, for [`WebTarget`]; the link's state
/// (`torque_client::net::web_rtc::LinkWatch`) comes back in the [`LinkInfo`] with the `webrtc` feature.
#[cfg(feature = "web")]
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-web-client")]
pub struct WebClientTransportPlugin;

#[cfg(feature = "web")]
impl Plugin for WebClientTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_client_transport("web", |target: &WebTarget| {
            use torque_client::net::web_socket::WebSocketDatagrams;
            #[cfg(not(target_arch = "wasm32"))]
            let WebTarget::Address(to) = target;
            #[cfg(not(target_arch = "wasm32"))]
            let ws = WebSocketDatagrams::connect(*to, SERVER_NAME)
                .map_err(|e| format!("the game socket to {to}: {e}"))?;
            #[cfg(target_arch = "wasm32")]
            let WebTarget::Url(url) = target;
            #[cfg(target_arch = "wasm32")]
            let ws = WebSocketDatagrams::open(url, SERVER_NAME)?;
            #[cfg(feature = "webrtc")]
            {
                use torque_client::net::web_rtc::WebRtcDatagrams;
                let socket = WebRtcDatagrams::new(ws);
                let watch = socket.watch();
                Ok(ClientLink::new(Box::new(socket), NetPolicy::Loopback).with_extra(watch))
            }
            #[cfg(not(feature = "webrtc"))]
            Ok(ClientLink::new(Box::new(ws), NetPolicy::Loopback))
        });
    }
}

// ---- quic
// ----------------------------------------------------------------------------------------------------------------

/// A server's QUIC endpoint, and how its key is checked.
#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuicTarget {
    pub address: Ipv4Addr,
    pub port: u16,
    /// The server's identity (from a listing, an invite, a typed hash): the connection is refused if its key hashes to
    /// anything else. None: the identity remembered for the address in the known-servers file, or the one presented,
    /// trusted on first use and remembered.
    pub identity: Option<torque_net_quic::ServerIdentity>,
    /// The known-servers file (none: `known_servers` in the client's folder under the user's data folder,
    /// [`known_servers_path`]).
    pub known_servers: Option<std::path::PathBuf>,
    /// Open a WebTransport session on the connection, as a browser does (the server must take them), instead of the raw
    /// protocol: one round trip more at connect, the same engine bytes.
    pub web_transport: bool,
}

/// Where the default known-servers file lives: `known_servers` in the client's folder under the user's data folder. None
/// when the system gives no data folder.
#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
pub fn known_servers_path() -> Option<std::path::PathBuf> {
    torque_app_server::uploads::data_dir().map(|data| {
        data.join("torque-rs")
            .join(torque_net_quic::KNOWN_SERVERS_FILE)
    })
}

/// A QUIC connection to a server's QUIC endpoint: encrypted, the server's address validated and its key checked against
/// [`QuicTarget::identity`] or the known servers. The socket names its server by the address dialed, so a ticket is made for
/// it as on UDP, plus the link's channel binding. Registers `quic`, for [`QuicTarget`]; the link's state
/// ([`torque_net_quic::QuicWatch`]: connected, the identity, a refusal's text) comes back in the [`LinkInfo`].
#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-quic-client")]
pub struct QuicClientTransportPlugin;

#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
impl Plugin for QuicClientTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_client_transport("quic", |target: &QuicTarget| {
            use torque_net_quic::{QuicDatagrams, QuicLimits, SystemClock, Trust, client_socket};
            let trust = match target.identity {
                Some(identity) => Trust::Identity(identity),
                None => Trust::Known(
                    target
                        .known_servers
                        .clone()
                        .or_else(known_servers_path)
                        .ok_or("no data folder for the known servers file: give the server's identity or name the file")?,
                ),
            };
            let loopback = target.address.is_loopback();
            let bind = if loopback {
                Ipv4Addr::LOCALHOST
            } else {
                Ipv4Addr::UNSPECIFIED
            };
            let io = client_socket(std::net::IpAddr::V4(bind))
                .map_err(|e| format!("the client's socket: {e}"))?;
            let server = SocketAddrV4::new(target.address, target.port);
            let session = if target.web_transport {
                torque_net_quic::SessionKind::WebTransport(torque_net_quic::Dial {
                    authority: server.to_string(),
                    path: "/".to_string(),
                    origin: None,
                })
            } else {
                torque_net_quic::SessionKind::Raw
            };
            let socket = QuicDatagrams::connect_with(
                io,
                SystemClock,
                server,
                trust,
                &QuicLimits::default(),
                None,
                session,
            )?;
            let watch = socket.watch();
            let policy = if loopback {
                NetPolicy::Loopback
            } else {
                NetPolicy::Internet
            };
            Ok(ClientLink::new(Box::new(socket), policy).with_extra(watch))
        });
    }
}

// ---- rooms and relay, in a browser
// ----------------------------------------------------------------------------------------------------------------

/// A player's host, by its room's `code`, found through the room service at `url`.
#[cfg(all(feature = "p2p", target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomTarget {
    pub url: String,
    pub code: String,
}

/// The link the page holds to a server it reaches itself.
#[cfg(all(feature = "p2p", target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayTarget {
    /// The server in this page's worker.
    Local,
    /// A player's host the page reached by an invite link: the page holds the data channel.
    Page,
    /// The server that served the page, over the WebTransport session the page opened to it.
    WebTransport,
}

/// A player's host found through a room service by its code: every datagram goes on the data channel to it. The link's state
/// ([`LinkWatch`](torque_client::net::web_rtc::LinkWatch)) comes back in the [`LinkInfo`]. Registers `rooms`, for
/// [`RoomTarget`].
#[cfg(all(feature = "p2p", target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-rooms-client")]
pub struct RoomsClientTransportPlugin;

#[cfg(all(feature = "p2p", target_arch = "wasm32"))]
impl Plugin for RoomsClientTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_client_transport("rooms", |target: &RoomTarget| {
            let socket = torque_client::net::room_join::RoomDatagrams::open(
                &target.url,
                &target.code,
                SERVER_NAME,
            )?;
            let watch = socket.watch();
            Ok(ClientLink::new(Box::new(socket), NetPolicy::Loopback).with_extra(watch))
        });
    }
}

/// The links a browser's page holds: to the server in its worker (a host player's own client), to a player's host reached
/// by an invite link, or to the server that served the page over a WebTransport session. Registers `relay`, for
/// [`RelayTarget`]; the link's state comes back in the [`LinkInfo`].
#[cfg(all(feature = "p2p", target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-relay-client")]
pub struct RelayClientTransportPlugin;

#[cfg(all(feature = "p2p", target_arch = "wasm32"))]
impl Plugin for RelayClientTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_client_transport("relay", |target: &RelayTarget| {
            let (socket, watch) = match target {
                RelayTarget::Local => crate::web_join::Target::Local,
                RelayTarget::Page => crate::web_join::Target::Page,
                RelayTarget::WebTransport => crate::web_join::Target::WebTransport,
            }
            .open()?;
            Ok(ClientLink::new(socket, NetPolicy::Loopback).with_extra(watch))
        });
    }
}

/// The client transports a build has: UDP (natively), the in-memory link, the web (`web` feature), QUIC (`quic` feature,
/// natively), and in a browser with the `p2p` feature rooms and the relay.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClientTransportPlugins;

impl PluginGroup for ClientTransportPlugins {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::new();
        #[cfg(not(target_arch = "wasm32"))]
        let group = group.add(UdpClientTransportPlugin);
        let group = group.add(MemoryClientTransportPlugin);
        #[cfg(feature = "web")]
        let group = group.add(WebClientTransportPlugin);
        #[cfg(all(feature = "p2p", target_arch = "wasm32"))]
        let group = group
            .add(RoomsClientTransportPlugin)
            .add(RelayClientTransportPlugin);
        #[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
        let group = group.add(QuicClientTransportPlugin);
        group
    }
}
