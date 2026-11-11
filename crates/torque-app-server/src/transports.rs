//! The transports a server can listen on, each registered by a plugin under an id.
//!
//! | Plugin | Id | Plugin id | Needs | Listens on |
//! |--------|----|-----------|-------|------------|
//! | [`UdpServerTransportPlugin`] | `udp` | `torque.transport-udp-server` | - | [`Listen::Udp`]: one UDP socket under the bind policy |
//! | [`MemoryServerTransportPlugin`] | `memory` | `torque.transport-memory-server` | - | [`Listen::Memory`]: an in-process link, its other end [`GameServer::memory_end`](crate::server::GameServer::memory_end) |
//! | `WebServerTransportPlugin` | `web` | `torque.transport-web-server` | `web` feature, native | `Listen::Web`: HTTP and WebSocket on one TCP port, UDP beside it or not |
//! | `RoomsServerTransportPlugin` | `rooms` | `torque.transport-rooms-server` | `p2p` feature, native | [`Listen::Bound`] with [`BoundTransport::ROOMS`]: a player's host found through a room service |
//! | `RelayServerTransportPlugin` | `relay` | `torque.transport-relay-server` | `p2p` feature | [`Listen::Bound`] with [`BoundTransport::RELAY`]: a browser host's links, held by its page |
//! | `InstantNetServerTransportPlugin` (torque-app) | `instant-net` | `torque.transport-instant-net-server` | - | [`Listen::Bound`] of an in-process network's transport: a harness's scripted session |
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
use std::net::{IpAddr, Ipv4Addr};
use std::rc::Rc;

use torque_engine::{Engine, EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder};
use torque_server::net::transport::{
    BindPolicy, LanRules, MemoryEnd, MemoryTransport, SharedTransport, Transport, UdpTransport,
};

use crate::server::Listen;
#[cfg(all(feature = "web", not(target_arch = "wasm32")))]
use crate::server::WebListen;

/// The id of the transport a server listens on when its game does not say: UDP.
pub const DEFAULT_TRANSPORT: &str = "udp";

/// What a transport opened for a server: the transport the network host runs on, and, for an in-memory one, its other end.
pub struct ServerLink {
    pub transport: SharedTransport,
    pub end: Option<MemoryEnd>,
}

impl ServerLink {
    /// A link that is `transport` alone.
    pub fn new(transport: SharedTransport) -> ServerLink {
        ServerLink {
            transport,
            end: None,
        }
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
    /// The id of the transport this names: `udp`, `memory`, `web`, the id of a [`BoundTransport`], or a custom one's.
    pub fn transport_id(&self) -> &str {
        match self {
            Listen::Udp { .. } => "udp",
            Listen::Memory(_) => "memory",
            #[cfg(all(feature = "web", not(target_arch = "wasm32")))]
            Listen::Web { .. } => "web",
            Listen::Bound(bound) => &bound.id,
            Listen::Custom(custom) => &custom.id,
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

/// The web transport (and UDP beside it when `web.udp` says so) bound on `address` and `port` under the bind policy.
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
/// feature, natively) and the relay (`p2p` feature).
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
        group
    }
}
