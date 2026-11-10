//! TGE's `TCPObject` and `HTTPObject` console classes for torque-rs: a script connects to an address, listens on a port, sends
//! text and gets each line it receives as a callback; an HTTPObject sends a GET or a POST and gets the body of the answer line
//! by line. [`TcpPlugin`] adds both classes to every server and client script engine and services their sockets once a tick.
//!
//! ```
//! use torque_engine::{EngineBuilder, ServerStage};
//! use torque_plugin_tcp::TcpPlugin;
//! use torque_ts_vm::vm::Vm;
//!
//! let engine = EngineBuilder::new().add_plugins(TcpPlugin::default()).build()?;
//! let mut vm = Vm::new();
//! for &stage in ServerStage::ALL {
//!     engine.replay(stage, &mut vm)?;
//! }
//! vm.eval(b"new HTTPObject(Feed); function Feed::onLine(%this, %line) { echo(%line); }")?;
//! // Each tick then calls `torque_plugin_tcp::poll(&mut vm)` through the plugin's tick hooks.
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! **The script API** is TGE's. `TCPObject::connect(address)` (`[ip:]host[:port]`, port 28000 when none is given),
//! `listen(port)`, `send(text, ...)` and `disconnect()`; `HTTPObject::get(address, path[, query])` and `post(address, path,
//! query, body)`. The callbacks are methods of the object, looked up from its name (a named object's name is its namespace):
//! `onDNSResolved` and `onDNSFailed` (an address given as a name only), `onConnected`, `onConnectFailed`, `onLine(line)`,
//! `onDisconnect`, and on a listening object `onConnectRequest(address, id)`, where `new TCPObject(name, id)` takes the
//! connection. The objects join `TCPGroup`.
//!
//! **The order** is TGE's: a name's lookup ends in `onDNSResolved` then `onConnected` or `onConnectFailed`, or in
//! `onDNSFailed` alone; an address in numbers goes straight to `onConnected` or `onConnectFailed`. Received bytes are split
//! into lines at `\n` or NUL with one `\r` dropped, and what waits when the peer closes is the last `onLine`, before
//! `onDisconnect`. A script's own `disconnect` (or a `connect` or `listen` that replaces the socket) calls nothing back. An
//! HTTPObject sends its request after the script's `onConnected`, takes in the status line and the headers, and gives each
//! line of the body (chunked or not); the answer ends when the server closes the connection.
//!
//! **What differs from TGE.** Nothing blocks a tick: the lookup, the connect and the TLS handshake run on a thread of their
//! own, and every socket is read without waiting. One script engine's sockets are bounded by [`TcpLimits`] (its own
//! connections, accepted connections waiting to be taken, listen sockets, connects under way, bytes read and sent per tick, the queue of a connection, the line length, callbacks per
//! tick); a connect a cap refuses warns and reports `onConnectFailed` on the next tick (once, however many were refused), a
//! peer that sends a line past the cap is disconnected, and a host name with a blank, a control byte or a byte past ASCII is
//! `onDNSFailed` before any lookup. The tick's pass is charged to the script engine's work budget ([`limits::PASS_WORK`],
//! [`limits::PASS_STEPS`]), and the methods charge the bytes they are given. `https://` in front of an HTTPObject's address
//! makes the connection TLS (rustls; the system's trusted roots unless the game gives its own, see
//! [`TcpPlugin::with_tls_config`]); `post` sends a POST with its body as a form, where TGE 1.4's sends a GET. An accepted
//! connection waits unread until a script takes it, and closes after [`TcpLimits::adopt_within`]. There is no allowlist, and
//! the client's network policy does not gate script sockets: scripts reach any address, as TGE's do. In a web build (wasm32)
//! the plugin adds nothing.
//!
//! Credit: follows TGE 1.4's `game/net/tcpObject.cc` and `game/net/httpObject.cc` (Copyright (C) GarageGames.com, Inc.) as
//! Torque3D carries them under the MIT license (`Engine/source/app/net/tcpObject.cpp` and
//! `Engine/source/app/net/httpObject.cpp`, each Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.
//!
//! The crate's README has the "Maintaining" section: the module map, the flow of a tick, the invariants and the tests that hold
//! them, the extension points and the check commands.

#![warn(missing_docs)]

#[cfg(not(target_arch = "wasm32"))]
mod address;
#[cfg(not(target_arch = "wasm32"))]
mod host;
#[cfg(not(target_arch = "wasm32"))]
mod http;
pub mod limits;
#[cfg(not(target_arch = "wasm32"))]
mod lines;
#[cfg(not(target_arch = "wasm32"))]
mod link;
#[cfg(not(target_arch = "wasm32"))]
mod natives;
#[cfg(all(feature = "https", not(target_arch = "wasm32")))]
mod tls;

#[cfg(not(target_arch = "wasm32"))]
pub use host::{PassReport, poll};
pub use limits::TcpLimits;
#[cfg(not(target_arch = "wasm32"))]
pub use link::{Resolver, system_resolver};
#[cfg(all(feature = "https", not(target_arch = "wasm32")))]
pub use tls::client_config;

use torque_engine::{EngineBuilder, Plugin, PluginId};

/// Adds `TCPObject` and `HTTPObject` to every server and client script engine, each with its own sockets under
/// [`TcpLimits`], and services them once a tick (a server's tick before its objects tick, a client's after its other tick
/// work). Not in `DefaultPlugins`: a game that wants script sockets adds it. Adds nothing in a web build.
#[derive(Clone, PluginId)]
#[plugin(id = "torque.tcp")]
pub struct TcpPlugin {
    limits: TcpLimits,
    #[cfg(not(target_arch = "wasm32"))]
    resolver: Resolver,
    #[cfg(all(feature = "https", not(target_arch = "wasm32")))]
    tls: tls::TlsSettings,
}

impl Default for TcpPlugin {
    /// The default caps, the system's resolver and, for `https://`, the system's trusted roots.
    fn default() -> TcpPlugin {
        TcpPlugin {
            limits: TcpLimits::default(),
            #[cfg(not(target_arch = "wasm32"))]
            resolver: system_resolver(),
            #[cfg(all(feature = "https", not(target_arch = "wasm32")))]
            tls: tls::TlsSettings::default(),
        }
    }
}

impl TcpPlugin {
    /// The plugin with these caps.
    #[must_use]
    pub fn with_limits(mut self, limits: TcpLimits) -> TcpPlugin {
        self.limits = limits;
        self
    }

    /// The plugin looking names up with `resolver` (a test's fixed answers, say) in place of the system's.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn with_resolver(mut self, resolver: Resolver) -> TcpPlugin {
        self.resolver = resolver;
        self
    }

    /// The plugin's HTTPS with these TLS settings (a game's own trusted roots: see [`client_config`]) in place of the
    /// system's roots.
    #[cfg(all(feature = "https", not(target_arch = "wasm32")))]
    #[must_use]
    pub fn with_tls_config(mut self, config: std::sync::Arc<rustls::ClientConfig>) -> TcpPlugin {
        self.tls = tls::TlsSettings::Given(config);
        self
    }

    /// The caps the plugin gives each script engine.
    pub fn limits(&self) -> &TcpLimits {
        &self.limits
    }
}

impl Plugin for TcpPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        #[cfg(not(target_arch = "wasm32"))]
        native::build(self, engine);
        #[cfg(target_arch = "wasm32")]
        let _ = engine;
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::rc::Rc;

    use torque_engine::{ClientStage, ClientTick, EngineBuilder, ServerStage, ServerTick, VmMut};
    use torque_ts_vm::vm::Vm;

    use crate::TcpPlugin;
    use crate::host::{TcpHost, poll};
    use crate::link::Connector;
    use crate::natives::install;

    /// Each server and client script engine gets its own sockets, and its tick services them.
    pub(crate) fn build(plugin: &TcpPlugin, engine: &mut EngineBuilder) {
        let connector = Connector {
            resolver: plugin.resolver.clone(),
            #[cfg(feature = "https")]
            tls: plugin.tls.clone(),
        };
        let make = {
            let limits = plugin.limits.clone();
            move || Rc::new(TcpHost::new(limits.clone(), connector.clone()))
        };
        let server = make.clone();
        engine.server(ServerStage::Game, move |vm, _| {
            install(vm, server());
            Ok(())
        });
        engine.client(ClientStage::Game, move |vm, _| {
            install(vm, make());
            Ok(())
        });
        engine.push(ServerTick::before(|mut vm: VmMut| tick(&mut vm)));
        engine.push(ClientTick::new(tick));
    }

    /// One tick's pass, its callbacks' errors printed.
    fn tick(vm: &mut Vm) {
        for error in poll(vm).errors {
            vm.error(format!("TCPObject callback: {error}"));
        }
    }
}
