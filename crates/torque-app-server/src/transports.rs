//! The transports a server can listen on, each registered by a plugin under an id.
//!
//! | Plugin | Id | Plugin id | Needs | Listens on |
//! |--------|----|-----------|-------|------------|
//! | [`UdpServerTransportPlugin`] | `udp` | `torque.transport-udp-server` | - | [`Listen::Udp`]: one UDP socket under the bind policy |
//! | [`MemoryServerTransportPlugin`] | `memory` | `torque.transport-memory-server` | - | [`Listen::Memory`]: an in-process link, its other end [`GameServer::memory_end`](crate::server::GameServer::memory_end) |
//! | `WebServerTransportPlugin` | `web` | `torque.transport-web-server` | `web` feature, native | `Listen::Web`: HTTP and WebSocket on one TCP port, UDP (or the QUIC link with WebTransport, `WebListen::quic`) beside it or not |
//! | `RoomsServerTransportPlugin` | `rooms` | `torque.transport-rooms-server` | `p2p` feature, native | [`Listen::Bound`] with [`BoundTransport::ROOMS`]: a player's host found through a room service |
//! | `RelayServerTransportPlugin` | `relay` | `torque.transport-relay-server` | `p2p` feature | [`Listen::Bound`] with [`BoundTransport::RELAY`]: a browser host's links, held by its page |
//! | `InstantNetServerTransportPlugin` (torque-app) | `instant-net` | `torque.transport-instant-net-server` | - | [`Listen::Bound`] of an in-process network's transport: a harness's scripted session |
//! | `QuicServerTransportPlugin` | `quic` | `torque.transport-quic-server` | `quic` feature, native | `Listen::Quic`: the QUIC link on one UDP socket, the server's identity key (`torque-net-quic`), WebTransport sessions for browsers when `QuicListen::web_transport` says so |
//!
//! [`ServerTransportPlugins`] is the group of the ones a build has; [`MinimalPlugins`](crate::plugins::MinimalPlugins) has it.
//!
//! **Registering a transport.** Any plugin adds one under an id ([`EngineServerTransports::add_server_transport`]): a
//! function from the [`Listen`] a server was started with to the [`ServerLink`] it listens on. Each transport answers to
//! the [`Listen`] variants that name its id ([`Listen::transport_id`]); a game's own transport is asked for with
//! [`Listen::custom`] and reads its request back with [`CustomListen::request`]. [`GameServer::start_engine`] picks the
//! transport by the id of its [`Listen`], so the first one registered under an id serves it (a second is skipped, with a
//! warning on the server's console). An id nobody registered, such as one whose plugin was turned off, is refused with the
//! ids that are registered and, for the engine's own, the plugin that registers it.
//!
//! [`GameServer::start_engine`]: crate::server::GameServer::start_engine

use std::any::Any;
use std::cell::RefCell;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_engine::{Engine, EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder};
#[cfg(all(feature = "web", feature = "quic", not(target_arch = "wasm32")))]
use torque_net::engine::{ChannelBinding, LinkKind};
#[cfg(all(feature = "web", feature = "quic", not(target_arch = "wasm32")))]
use torque_server::net::transport::{BindError, Recv};
use torque_server::net::transport::{
    BindPolicy, LanRules, MemoryEnd, MemoryTransport, SharedTransport, Transport, UdpTransport,
};

use crate::server::Listen;
#[cfg(all(feature = "web", not(target_arch = "wasm32")))]
use crate::server::WebListen;
#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
use crate::server::{IdentitySource, QuicListen};

/// The id of the transport a server listens on when its game does not say: UDP.
pub const DEFAULT_TRANSPORT: &str = "udp";

/// What a transport opened for a server: the transport the network host runs on, and, for an in-memory one, its other end.
pub struct ServerLink {
    pub transport: SharedTransport,
    pub end: Option<MemoryEnd>,
}

impl From<SharedTransport> for ServerLink {
    fn from(transport: SharedTransport) -> ServerLink {
        ServerLink::new(transport)
    }
}

impl ServerLink {
    /// A link that is `transport` alone.
    pub fn new(transport: SharedTransport) -> ServerLink {
        ServerLink {
            transport,
            end: None,
        }
    }

    /// An in-memory transport bound at `at`, and its other end: what [`Listen::Memory`] listens on.
    ///
    /// # Errors
    /// The transport refused the address.
    pub fn memory(at: SocketAddr) -> Result<ServerLink, String> {
        let (transport, end) = MemoryTransport::new();
        let transport: SharedTransport = Rc::new(RefCell::new(transport));
        transport
            .borrow_mut()
            .bind(at.ip(), at.port())
            .map_err(|e| format!("the link: {e:?}"))?;
        Ok(ServerLink {
            transport,
            end: Some(end),
        })
    }
}

/// Opens a registered transport for the [`Listen`] a server was started with.
pub type OpenServerTransport = dyn Fn(&Listen) -> Result<ServerLink, String>;

/// A transport registered on the engine: its id and how it is opened.
#[derive(Clone)]
pub struct ServerTransportEntry {
    id: String,
    open: Rc<OpenServerTransport>,
}

impl ServerTransportEntry {
    /// Its id.
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl fmt::Debug for ServerTransportEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerTransportEntry")
            .field("id", &self.id)
            .finish()
    }
}

/// The engine builder's call that registers a transport a server can listen on.
pub trait EngineServerTransports {
    /// Registers the transport `open` opens under `id`. A second transport under an id already registered is skipped, with
    /// a warning on each server's console.
    fn add_server_transport(
        &mut self,
        id: &str,
        open: impl Fn(&Listen) -> Result<ServerLink, String> + 'static,
    ) -> &mut EngineBuilder;
}

impl EngineServerTransports for EngineBuilder {
    fn add_server_transport(
        &mut self,
        id: &str,
        open: impl Fn(&Listen) -> Result<ServerLink, String> + 'static,
    ) -> &mut EngineBuilder {
        let open: Rc<OpenServerTransport> = Rc::new(open);
        self.push(ServerTransportEntry {
            id: id.to_owned(),
            open,
        })
    }
}

/// The ids of the transports `engine` has for servers, in the order they were registered.
pub fn server_transport_ids(engine: &Engine) -> Vec<&str> {
    engine
        .list::<ServerTransportEntry>()
        .iter()
        .map(ServerTransportEntry::id)
        .collect()
}

/// The lines for the console of a server made by `engine`: a transport registered under an id that was already taken.
pub fn server_transport_warnings(engine: &Engine) -> Vec<String> {
    let entries = engine.list::<ServerTransportEntry>();
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

/// Opens the transport `listen` names for a server made by `engine`.
pub fn open_listen(engine: &Engine, listen: &Listen) -> Result<ServerLink, String> {
    let id = listen.transport_id();
    let Some(entry) = engine
        .list::<ServerTransportEntry>()
        .iter()
        .find(|e| e.id == id)
    else {
        return Err(unregistered(
            "listen on",
            id,
            builtin_server_plugin(id),
            &server_transport_ids(engine),
        ));
    };
    (entry.open)(listen)
}

/// The refusal for an id no plugin registered: what was asked (`verb`), the id, the engine's own plugin that registers it (if
/// it is one of the engine's) and the ids that are registered.
pub fn unregistered(verb: &str, id: &str, plugin: Option<&str>, registered: &[&str]) -> String {
    let have = if registered.is_empty() {
        "none".to_string()
    } else {
        registered.join(", ")
    };
    let how = match plugin {
        Some(plugin) => format!("add its plugin, {plugin}"),
        None => "add the plugin that registers it".to_string(),
    };
    format!(
        "cannot {verb} the transport {id:?}: no plugin registered it ({how}); registered: {have}"
    )
}

/// The plugin of the engine's own that registers `id` for servers.
fn builtin_server_plugin(id: &str) -> Option<&'static str> {
    Some(match id {
        "udp" => "torque.transport-udp-server",
        "memory" => "torque.transport-memory-server",
        "web" => "torque.transport-web-server (the `web` feature, not in a browser)",
        "rooms" => "torque.transport-rooms-server (the `p2p` feature, not in a browser)",
        "relay" => "torque.transport-relay-server (the `p2p` feature)",
        "instant-net" => "torque.transport-instant-net-server (torque-app)",
        "quic" => "torque.transport-quic-server (the `quic` feature, not in a browser)",
        _ => return None,
    })
}

/// The refusal for a transport handed a [`Listen`] meant for another.
fn wrong_request(id: &str, got: &Listen) -> String {
    format!(
        "the transport {id:?} was asked to listen as the transport {:?} does",
        got.transport_id()
    )
}

/// The request a game's own transport was asked to listen on: [`Listen::custom`] puts it in a [`Listen`] under the
/// transport's id, and the transport reads it back.
#[derive(Clone)]
pub struct CustomListen {
    id: String,
    request: Rc<dyn Any>,
}

impl CustomListen {
    /// The transport it is for.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The request, if it is a `T`.
    pub fn request<T: Any>(&self) -> Option<&T> {
        self.request.downcast_ref()
    }
}

impl fmt::Debug for CustomListen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CustomListen")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Listen {
    /// The id of the transport this names: `udp`, `memory`, `web`, the id of a [`BoundTransport`], a custom one's, or `late`
    /// (no transport: [`Listen::Late`]).
    pub fn transport_id(&self) -> &str {
        match self {
            Listen::Udp { .. } => "udp",
            Listen::Memory(_) => "memory",
            #[cfg(all(feature = "web", not(target_arch = "wasm32")))]
            Listen::Web { .. } => "web",
            #[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
            Listen::Quic { .. } => "quic",
            Listen::Bound(bound) => &bound.id,
            Listen::Custom(custom) => &custom.id,
            Listen::Late => "late",
        }
    }

    /// A listen on the transport registered under `id`, with `request` for it to read ([`CustomListen::request`]).
    pub fn custom(id: &str, request: impl Any) -> Listen {
        Listen::Custom(CustomListen {
            id: id.to_owned(),
            request: Rc::new(request),
        })
    }
}

/// A transport made and bound before the server starts ([`Listen::Bound`]), and the id of the transport it is.
#[derive(Clone)]
pub struct BoundTransport {
    pub id: String,
    pub transport: SharedTransport,
}

impl BoundTransport {
    /// A player's host found through a room service (`rooms`).
    pub const ROOMS: &'static str = "rooms";
    /// A browser host's links, held by its page (`relay`).
    pub const RELAY: &'static str = "relay";

    /// `transport`, which the transport registered under `id` takes.
    pub fn new(id: &str, transport: SharedTransport) -> BoundTransport {
        BoundTransport {
            id: id.to_owned(),
            transport,
        }
    }
}

impl fmt::Debug for BoundTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = self.transport.borrow().local_addr();
        f.debug_struct("BoundTransport")
            .field("id", &self.id)
            .field("at", &at)
            .finish()
    }
}

/// The bind policy for `address` (`public` allows `0.0.0.0` alone).
fn bind_policy(address: Ipv4Addr, public: bool) -> Result<BindPolicy, String> {
    BindPolicy::new(
        IpAddr::V4(address),
        public && address.is_unspecified(),
        &LanRules::PLAIN,
    )
    .map_err(|e| {
        format!(
            "the address {address} is not a private one; only 0.0.0.0 (every interface) may be public: {e}"
        )
    })
}

/// A UDP transport bound on `address` and `port` under the bind policy (`public` allows `0.0.0.0` alone).
fn udp_transport(address: Ipv4Addr, port: u16, public: bool) -> Result<SharedTransport, String> {
    let ip = IpAddr::V4(address);
    let policy = bind_policy(address, public)?;
    let mut udp = UdpTransport::new(policy);
    udp.bind(ip, port)
        .map_err(|e| format!("the server's socket: {e}"))?;
    Ok(Rc::new(RefCell::new(udp)))
}

/// The web transport (and UDP, or the QUIC link, beside it when `web.udp` or `web.quic` says so) bound on `address` and
/// `port` under the bind policy.
#[cfg(all(feature = "web", not(target_arch = "wasm32")))]
fn web_transport(
    address: Ipv4Addr,
    port: u16,
    public: bool,
    web: &WebListen,
) -> Result<SharedTransport, String> {
    use torque_server::net::web::{MultiTransport, WebTransport};
    let policy = bind_policy(address, public)?;
    let ip = IpAddr::V4(address);
    let site = WebTransport::new(policy, web.setup.clone());
    #[cfg(feature = "quic")]
    if let Some(quic) = &web.quic {
        if web.udp {
            return Err(
                "the QUIC link and plain UDP never share a port: a web listen takes one of them"
                    .to_string(),
            );
        }
        let endpoint = quic_endpoint(policy, quic)?;
        let mut transport = WebQuicTransport::new(MultiTransport::new(endpoint, site));
        transport
            .bind(ip, port)
            .map_err(|e| format!("the server's socket: {e}"))?;
        return Ok(Rc::new(RefCell::new(transport)));
    }
    let transport: SharedTransport = if web.udp {
        Rc::new(RefCell::new(MultiTransport::new(
            UdpTransport::new(policy),
            site,
        )))
    } else {
        Rc::new(RefCell::new(site))
    };
    transport
        .borrow_mut()
        .bind(ip, port)
        .map_err(|e| format!("the server's socket: {e}"))?;
    Ok(transport)
}

/// The QUIC link and the web on one port number, the page served told what a browser needs for a WebTransport session:
/// `webtransport.json` holds the QUIC port number, the CONNECT's path and the certificate hashes (`serverCertificateHashes`), written
/// again whenever the hashes change (the certificates rotate every six days).
#[cfg(all(feature = "web", feature = "quic", not(target_arch = "wasm32")))]
pub struct WebQuicTransport {
    inner: torque_server::net::web::MultiTransport<torque_net_quic::QuicServerTransport>,
    published: Option<Vec<torque_net_quic::CertHash>>,
}

#[cfg(all(feature = "web", feature = "quic", not(target_arch = "wasm32")))]
impl WebQuicTransport {
    /// The file the page reads.
    pub const FILE: &'static str = "webtransport.json";

    pub fn new(
        inner: torque_server::net::web::MultiTransport<torque_net_quic::QuicServerTransport>,
    ) -> WebQuicTransport {
        WebQuicTransport {
            inner,
            published: None,
        }
    }

    /// The QUIC endpoint.
    pub fn quic(&self) -> &torque_net_quic::QuicServerTransport {
        self.inner.udp()
    }

    /// Writes `webtransport.json` when the hashes it carries are not the ones published.
    fn publish(&mut self) {
        let quic = self.inner.udp();
        if !quic.web_transport_enabled() {
            return;
        }
        let hashes = quic.certificate_hashes();
        if self.published.as_ref() == Some(&hashes) {
            return;
        }
        let Some(port) = quic.local_addr().map(|a| a.port()) else {
            return;
        };
        let json = web_transport_json(port, "/", &hashes);
        self.inner
            .web_mut()
            .set_file(WebQuicTransport::FILE, json.into_bytes());
        self.published = Some(hashes);
    }
}

/// `webtransport.json`: `{"port":28000,"path":"/","hashes":["<hex>",...]}`.
#[cfg(all(feature = "web", feature = "quic", not(target_arch = "wasm32")))]
pub fn web_transport_json(port: u16, path: &str, hashes: &[torque_net_quic::CertHash]) -> String {
    let hashes: Vec<String> = hashes.iter().map(|h| format!("\"{h}\"")).collect();
    format!(
        "{{\"port\":{port},\"path\":\"{}\",\"hashes\":[{}]}}",
        path.replace(['"', '\\'], ""),
        hashes.join(",")
    )
}

#[cfg(all(feature = "web", feature = "quic", not(target_arch = "wasm32")))]
impl Transport for WebQuicTransport {
    fn bind(&mut self, ip: IpAddr, port: u16) -> Result<(), BindError> {
        self.inner.bind(ip, port)?;
        self.publish();
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Recv {
        self.inner.recv(buf)
    }

    fn send_to(&mut self, to: SocketAddr, data: &[u8]) -> bool {
        self.inner.send_to(to, data)
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.inner.local_addr()
    }

    fn is_real(&self) -> bool {
        true
    }

    fn close(&mut self) {
        self.inner.close();
    }

    fn service(&mut self) {
        self.inner.service();
        self.publish();
    }

    fn wants_service(&self) -> bool {
        self.inner.wants_service()
    }

    fn take_gone(&mut self) -> Vec<SocketAddr> {
        self.inner.take_gone()
    }

    fn status(&mut self) -> Vec<String> {
        self.inner.status()
    }

    fn link_kind(&self, peer: SocketAddr) -> LinkKind {
        self.inner.link_kind(peer)
    }

    fn peer_ip(&self, peer: SocketAddr) -> IpAddr {
        self.inner.peer_ip(peer)
    }

    fn channel_binding(&self, peer: SocketAddr) -> Option<ChannelBinding> {
        self.inner.channel_binding(peer)
    }
}

/// The QUIC endpoint on `address` and `port` under the bind policy, on the identity key `quic` names (made on first start
/// when it is a file that does not exist yet).
#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
fn quic_transport(
    address: Ipv4Addr,
    port: u16,
    public: bool,
    quic: &QuicListen,
) -> Result<SharedTransport, String> {
    let policy = bind_policy(address, public)?;
    let mut server = quic_endpoint(policy, quic)?;
    server
        .bind(IpAddr::V4(address), port)
        .map_err(|e| format!("the server's socket: {e}"))?;
    Ok(Rc::new(RefCell::new(server)))
}

/// A QUIC endpoint under `policy` (not yet bound), on the identity key `quic` names, with its WebTransport sessions on when
/// `quic.web_transport` says so.
#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
fn quic_endpoint(
    policy: BindPolicy,
    quic: &QuicListen,
) -> Result<torque_net_quic::QuicServerTransport, String> {
    use torque_net_quic::{QuicServerTransport, ServerKey, SystemClock, UdpIo};
    let key = match &quic.identity {
        IdentitySource::Key(key) => Rc::clone(key),
        IdentitySource::File(path) => {
            Rc::new(ServerKey::load_or_make(path).map_err(|e| e.to_string())?)
        }
        IdentitySource::Session => Rc::new(ServerKey::generate().map_err(|e| e.to_string())?),
        IdentitySource::Default => {
            let path = crate::server::identity_path().ok_or(
                "no data folder for the server's identity key: name the key file in QuicListen::identity",
            )?;
            Rc::new(ServerKey::load_or_make(&path).map_err(|e| e.to_string())?)
        }
    };
    let mut server = QuicServerTransport::new(
        UdpIo::new(policy),
        SystemClock,
        &key,
        quic.limits.clone(),
        None,
    )
    .map_err(|e| e.to_string())?;
    if let Some(setup) = &quic.web_transport {
        server.enable_web_transport(&key, setup.clone())?;
    }
    if quic.lan_list {
        server.pass_plain(is_lan_list_request);
    }
    Ok(server)
}

/// Whether `data` is one of the LAN list's requests (a ping or an info request), which a QUIC endpoint with
/// `QuicListen::lan_list` hands the host.
#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
fn is_lan_list_request(data: &[u8]) -> bool {
    data.first()
        .is_some_and(|ty| LanRules::PLAIN.broadcast_requests.contains(ty))
}

/// UDP datagrams: one socket on the server's address, which the LAN list's pings and queries share. Registers `udp`.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-udp-server")]
pub struct UdpServerTransportPlugin;

impl Plugin for UdpServerTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_server_transport("udp", |listen| match listen {
            &Listen::Udp {
                address,
                port,
                public,
            } => Ok(ServerLink::new(udp_transport(address, port, public)?)),
            other => Err(wrong_request("udp", other)),
        });
    }
}

/// An in-process link: the server's transport is bound at an address and its other end carries the datagrams a client in
/// the same process sends and receives, with no socket opened. Registers `memory`.
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-memory-server")]
pub struct MemoryServerTransportPlugin;

impl Plugin for MemoryServerTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_server_transport("memory", |listen| {
            let &Listen::Memory(at) = listen else {
                return Err(wrong_request("memory", listen));
            };
            ServerLink::memory(at)
        });
    }
}

/// Browsers over WebSocket, with the game's page served on the same TCP port and UDP beside it or not. Registers `web`.
#[cfg(all(feature = "web", not(target_arch = "wasm32")))]
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-web-server")]
pub struct WebServerTransportPlugin;

#[cfg(all(feature = "web", not(target_arch = "wasm32")))]
impl Plugin for WebServerTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_server_transport("web", |listen| match listen {
            Listen::Web {
                address,
                port,
                public,
                web,
            } => Ok(ServerLink::new(web_transport(
                *address, *port, *public, web,
            )?)),
            other => Err(wrong_request("web", other)),
        });
    }
}

/// Desktop players over the QUIC link: one UDP socket, every connection encrypted and address-validated by QUIC, the server
/// known by the hash of its identity key. Registers `quic`.
#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-quic-server")]
pub struct QuicServerTransportPlugin;

#[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
impl Plugin for QuicServerTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_server_transport("quic", |listen| match listen {
            Listen::Quic {
                address,
                port,
                public,
                quic,
            } => Ok(ServerLink::new(quic_transport(
                *address, *port, *public, quic,
            )?)),
            other => Err(wrong_request("quic", other)),
        });
    }
}

/// Takes a transport made and bound before the server starts, under `id`: the open function of a transport registered under
/// `id` that takes only a [`Listen::Bound`] of that id.
pub fn bound_transport(id: &'static str) -> impl Fn(&Listen) -> Result<ServerLink, String> {
    move |listen| match listen {
        Listen::Bound(bound) if bound.id == id => Ok(ServerLink::new(Rc::clone(&bound.transport))),
        other => Err(wrong_request(id, other)),
    }
}

/// A game a player hosts, found through a room service by its code: the host's transport carries the joiners' WebRTC data
/// channels straight to this computer, with no port forwarded. The transport is made and bound by
/// [`serve::room_host`](crate::serve::room_host) and given as a [`Listen::Bound`]. Registers `rooms`.
#[cfg(all(feature = "p2p", not(target_arch = "wasm32")))]
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-rooms-server")]
pub struct RoomsServerTransportPlugin;

#[cfg(all(feature = "p2p", not(target_arch = "wasm32")))]
impl Plugin for RoomsServerTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_server_transport(
            BoundTransport::ROOMS,
            bound_transport(BoundTransport::ROOMS),
        );
    }
}

/// A browser's host: its server runs in a worker while the page holds the room service's socket and the joiners' data
/// channels, and tells the relay what happens to each link. The relay transport is made by the worker and given as a
/// [`Listen::Bound`]. Registers `relay`.
#[cfg(feature = "p2p")]
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.transport-relay-server")]
pub struct RelayServerTransportPlugin;

#[cfg(feature = "p2p")]
impl Plugin for RelayServerTransportPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_server_transport(
            BoundTransport::RELAY,
            bound_transport(BoundTransport::RELAY),
        );
    }
}

/// The server transports a build has: UDP (natively), the in-memory link, the web (`web` feature, natively), rooms (`p2p`
/// feature, natively), the relay (`p2p` feature) and QUIC (`quic` feature, natively).
#[derive(Clone, Copy, Debug, Default)]
pub struct ServerTransportPlugins;

impl PluginGroup for ServerTransportPlugins {
    fn build(self) -> PluginGroupBuilder {
        let group = PluginGroupBuilder::new();
        #[cfg(not(target_arch = "wasm32"))]
        let group = group.add(UdpServerTransportPlugin);
        let group = group.add(MemoryServerTransportPlugin);
        #[cfg(all(feature = "web", not(target_arch = "wasm32")))]
        let group = group.add(WebServerTransportPlugin);
        #[cfg(all(feature = "p2p", not(target_arch = "wasm32")))]
        let group = group.add(RoomsServerTransportPlugin);
        #[cfg(feature = "p2p")]
        let group = group.add(RelayServerTransportPlugin);
        #[cfg(all(feature = "quic", not(target_arch = "wasm32")))]
        let group = group.add(QuicServerTransportPlugin);
        group
    }
}
