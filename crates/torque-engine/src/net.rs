//! What an engine says about its network surface: the plugins' network versions, the class names and bulk channels plugins register, and
//! the auth provider ([`EngineBuilder::net`]). From these the engine builds the [`Manifest`] a client sends when it connects and the
//! [`ManifestGate`] a server judges clients with ([`Engine::connect_block`], [`Engine::connect_gate`]).
//!
//! An engine with no networked plugin, no registered name and no auth provider says nothing: it sends no block and judges no
//! client, so its handshake is Torque's own. The moment one plugin has a [`net_version`](crate::Plugin::net_version), the
//! server refuses a client without that plugin at that version ("server needs plugin torque.rope 2, you have 1") before any
//! ghost or datablock is sent.
//!
//! ```
//! use torque_engine::{EngineBuilder, Plugin, PluginId};
//! use torque_net::connect::{AuthContext, SharedSecret};
//!
//! #[derive(PluginId)]
//! #[plugin(id = "example.rope")]
//! struct Rope;
//!
//! impl Plugin for Rope {
//!     fn net_version(&self) -> Option<u32> {
//!         Some(2)
//!     }
//!
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         engine.net().ghost_class("Rope").datablock_class("RopeData");
//!         engine.net().auth(SharedSecret::new([7; 16], "player"));
//!     }
//! }
//!
//! let engine = EngineBuilder::new().add_plugins(Rope).build()?;
//! let cx = AuthContext {
//!     server: "192.0.2.1:28000".parse()?,
//!     sequence: 1,
//!     challenge: [1, 2, 3, 4],
//! };
//! let block = engine.connect_block(&cx)?.expect("a networked plugin sends a manifest");
//! let manifest = block.manifest.expect("the manifest");
//! assert_eq!(manifest.plugins[0].id, "example.rope");
//! assert_eq!(manifest.plugins[0].version, 2);
//! assert!(block.ticket.is_some());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::rc::Rc;
use std::sync::Arc;

use torque_collide::source::{PluginCollision, source_names};
use torque_net::chunk::ChannelLimits;
use torque_net::chunk::registry::{ChannelEntry, ChannelError, ChannelRegistry, ChunkChannel};
use torque_net::connect::{AuthContext, AuthProvider, ConnectBlock, ManifestGate};
use torque_net::manifest::Manifest;
use torque_net::modsync::ModsSection;
use torque_net::profile::HandshakeProfile;

use torque_ts_vm::vm::Vm;

use crate::rpc::{self, RpcEntry};
use crate::{Engine, EngineBuilder};

/// What a client's script engine adds to the connect block its connections send: that it takes a server's mod content and the most
/// it will take ([`ModsSection`]; nothing about the mods it has). A plugin makes it an extension of the client's script engine
/// (`vm.set_extension(Rc::new(ConnectExtras(..)))`) and each connection asks it when it starts.
pub struct ConnectExtras(pub Rc<MakeSection>);

/// The address a client's next network connection dials, which a plugin may read when it makes the section ([`ConnectExtras`]):
/// the client sets it on its script engine as each connection starts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ConnectTarget(pub std::net::SocketAddr);

/// What makes the section ([`ConnectExtras`]).
pub type MakeSection = dyn Fn(&Vm) -> Option<ModsSection>;

/// The auth provider slot's value.
struct AuthSlot(Arc<dyn AuthProvider>);

/// Which network protocol a game's servers and clients speak. A game speaks one: the exclusive slot ([`NetRegistry::protocol`])
/// holds it, and an engine whose plugins set none speaks [`ProtocolKind::Tge`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProtocolKind {
    /// Torque's own protocol: its handshake, its packet header and its event layer, byte for byte, so a server and a client of
    /// the engine can also talk to a peer that is not the engine's.
    #[default]
    Tge,
    /// The engine's own protocol, which the engine's peers speak among themselves.
    Engine,
}

impl ProtocolKind {
    /// The word that names the protocol in `game.toml` (`[net] protocol`) and in the trace hashes' lines.
    pub const fn label(self) -> &'static str {
        match self {
            ProtocolKind::Tge => "tge",
            ProtocolKind::Engine => "engine",
        }
    }

    /// The protocol `label` names.
    pub fn from_label(label: &str) -> Option<ProtocolKind> {
        match label {
            "tge" => Some(ProtocolKind::Tge),
            "engine" => Some(ProtocolKind::Engine),
            _ => None,
        }
    }
}

/// The protocol slot's value.
struct ProtocolSlot(ProtocolKind);

/// Which name table a registered name goes in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NameTable {
    Ghost,
    Datablock,
}

/// A name a plugin registered for the manifest.
struct NetName {
    table: NameTable,
    name: String,
}

impl EngineBuilder {
    /// The network registry: the auth provider and the names the manifest lists ([module docs](crate::net)).
    pub fn net(&mut self) -> NetRegistry<'_> {
        NetRegistry { engine: self }
    }
}

/// Adds to what an engine being built says about its network surface ([module docs](crate::net)).
pub struct NetRegistry<'a> {
    pub(crate) engine: &'a mut EngineBuilder,
}

impl NetRegistry<'_> {
    /// The engine being built, for a crate that adds its own calls to the registry (an extension trait on `NetRegistry`, such
    /// as the client's prediction slot).
    pub fn builder(&mut self) -> &mut EngineBuilder {
        self.engine
    }

    /// Sets the auth provider: a client sends the ticket it makes ([`AuthProvider::ticket`]) with its connect request, and a
    /// server refuses a client whose ticket it does not verify ([`AuthProvider::verify`]), after the manifest check and before
    /// `onConnectRequest`. Both get the connection's [`AuthContext`] (the server's address and the challenge), and a provider
    /// binds its tickets to it so a ticket cannot be replayed on another server or connection. One plugin sets it; a second
    /// fails the build.
    pub fn auth<A: AuthProvider + 'static>(&mut self, provider: A) -> &mut Self {
        self.engine.set_slot(Rc::new(AuthSlot(Arc::new(provider))));
        self
    }

    /// Sets the network protocol the game speaks. One plugin sets it (`TgeProtocolPlugin` in the plugin groups); a second fails the
    /// build, naming both. A game that moves to another protocol swaps that plugin, or names the protocol in `game.toml`
    /// (`[net] protocol`).
    pub fn protocol(&mut self, kind: ProtocolKind) -> &mut Self {
        self.engine.set_slot(Rc::new(ProtocolSlot(kind)));
        self
    }

    /// Lists a ghost class in the manifest. A peer with another table of ghost classes is refused. (A plugin that registers ghost
    /// classes through the engine's class registry has them listed by it; this is for a game that decides on its own.)
    pub fn ghost_class(&mut self, name: impl Into<String>) -> &mut Self {
        self.name(NameTable::Ghost, name.into())
    }

    /// Lists a datablock class in the manifest.
    pub fn datablock_class(&mut self, name: impl Into<String>) -> &mut Self {
        self.name(NameTable::Datablock, name.into())
    }

    /// Adds a bulk channel: large, mostly static data a server sends to each client in chunks (see
    /// [`torque_net::chunk`]). `C::default()` is the channel's logic on each side, and `budget` the most bytes of each packet
    /// it takes. The name is listed in the manifest, so a peer with other channels is refused. A name two plugins both use, or
    /// a budget outside the limits, fails when the server or client is made ([`Engine::net_channels`]).
    pub fn channel<C: ChunkChannel + Default>(
        &mut self,
        name: impl Into<String>,
        budget: usize,
    ) -> &mut Self {
        self.channel_with(name, ChannelLimits::new(budget), C::default)
    }

    /// Adds the bulk channel `name` with all its limits and a logic made by `make` (once for each side and connection).
    pub fn channel_with<C: ChunkChannel>(
        &mut self,
        name: impl Into<String>,
        limits: ChannelLimits,
        make: impl Fn() -> C + 'static,
    ) -> &mut Self {
        self.engine.push(ChannelEntry::new(name, limits, make));
        self
    }

    /// Adds a client-to-server bulk channel: messages each client sends the server in chunks at the front of its data packets,
    /// reliable and ordered per channel, under the server's caps for what it holds from clients, message size and rate
    /// ([`torque_net::chunk::upstream`]). `C::default()` is the logic the server runs, which gets each message
    /// ([`ChunkChannel::receive_from`]), and `budget` the most bytes of a client packet its chunks take. A client sends through
    /// its channels' `send`. The channel is listed in the manifest (with a prefix, so it never collides with a server-to-client
    /// name), so a peer without it is refused, and the section exists only on connections whose manifest matched.
    pub fn channel_to_server<C: ChunkChannel + Default>(
        &mut self,
        name: impl Into<String>,
        budget: usize,
    ) -> &mut Self {
        self.channel_to_server_with(name, ChannelLimits::new(budget), C::default)
    }

    /// Adds the client-to-server channel `name` with all its limits and a logic made by `make`.
    pub fn channel_to_server_with<C: ChunkChannel>(
        &mut self,
        name: impl Into<String>,
        limits: ChannelLimits,
        make: impl Fn() -> C + 'static,
    ) -> &mut Self {
        self.engine
            .push(ChannelEntry::to_server(name, limits, make));
        self
    }

    fn name(&mut self, table: NameTable, name: String) -> &mut Self {
        self.engine.push(NetName { table, name });
        self
    }
}

impl Engine {
    /// The network protocol the engine speaks: the one a plugin set ([`NetRegistry::protocol`]), else Torque's.
    pub fn protocol(&self) -> ProtocolKind {
        self.slot::<ProtocolSlot>()
            .map_or(ProtocolKind::Tge, |slot| slot.0)
    }

    /// The auth provider a plugin set ([`NetRegistry::auth`]).
    pub fn auth(&self) -> Option<Arc<dyn AuthProvider>> {
        self.slot::<AuthSlot>().map(|slot| Arc::clone(&slot.0))
    }

    /// The manifest of this engine for peers speaking `profile`'s protocol: every plugin with a network version, the names plugins
    /// registered. The mods a server requires are not in it: they ride in the connect block ([`torque_net::modsync`]).
    pub fn net_manifest(&self, profile: &HandshakeProfile) -> Manifest {
        let mut manifest = Manifest::new(profile.protocol_min, profile.protocol_current);
        for (id, version) in self.net_versions() {
            manifest = manifest.with_plugin(id.as_str(), *version);
        }
        let names = |table: NameTable| {
            self.list::<NetName>()
                .iter()
                .filter(move |n| n.table == table)
                .map(|n| n.name.as_str())
        };
        // The plugins' collision sources ride in the datablock table as `collision.<Name>`: a peer with other sources has other
        // type bits, so its manifest differs and it is refused at connect (no class has a dot in its name).
        let collision: Vec<String> = self
            .collision_names()
            .map(|n| format!("collision.{n}"))
            .collect();
        manifest
            .with_ghosts(names(NameTable::Ghost))
            .with_datablocks(
                names(NameTable::Datablock).chain(collision.iter().map(String::as_str)),
            )
            .with_channels(
                self.channel_entries()
                    .iter()
                    .map(ChannelEntry::manifest_name),
            )
            .with_rpcs(self.list::<RpcEntry>().iter().map(RpcEntry::manifest_entry))
    }

    /// The channel entries of this engine: the plugins', and the reserved RPC channel in each direction when plugins registered
    /// RPCs.
    fn channel_entries(&self) -> Vec<ChannelEntry> {
        let mut entries = self.list::<ChannelEntry>().to_vec();
        if !self.list::<RpcEntry>().is_empty() {
            entries.extend(rpc::channel_entries());
        }
        entries
    }

    /// The names of the collision sources plugins added ([`PluginCollision`]), each once, the valid ones only.
    pub fn collision_names(&self) -> impl Iterator<Item = &str> {
        source_names(
            self.list::<PluginCollision>()
                .iter()
                .map(|c| c.name.as_str()),
        )
        .into_iter()
    }

    /// The bulk channels plugins added, in id order (sorted by name). Empty for an engine with none, whose packets are then
    /// Torque's own.
    pub fn net_channels(&self) -> Result<ChannelRegistry, ChannelError> {
        ChannelRegistry::new(&self.channel_entries())
    }

    /// Whether this engine says anything on the wire: a networked plugin, a registered name or an auth provider.
    pub fn is_networked(&self) -> bool {
        !self.net_versions().is_empty()
            || !self.list::<NetName>().is_empty()
            || !self.list::<ChannelEntry>().is_empty()
            || !self.list::<RpcEntry>().is_empty()
            || self.collision_names().next().is_some()
            || self.auth().is_some()
    }

    /// What a client of this engine adds to its connect request
    /// ([`ConnectFields::with_block`](torque_net::handshake::ConnectFields::with_block)) for the connection `cx` names: its
    /// manifest and a ticket its auth provider made for that connection. `Ok(None)` for an engine that
    /// [says nothing](Engine::is_networked), whose request stays Torque's. An `Err` is the auth provider's reason it has no
    /// ticket, or why the block cannot be sent (shown to the player; the connect ends).
    pub fn connect_block(&self, cx: &AuthContext) -> Result<Option<ConnectBlock>, String> {
        self.connect_block_for(&HandshakeProfile::default(), cx)
    }

    /// [`Engine::connect_block`] for a game whose handshake is `profile`.
    pub fn connect_block_for(
        &self,
        profile: &HandshakeProfile,
        cx: &AuthContext,
    ) -> Result<Option<ConnectBlock>, String> {
        if !self.is_networked() {
            return Ok(None);
        }
        let ticket = self.auth().map(|auth| auth.ticket(cx)).transpose()?;
        let block = ConnectBlock {
            manifest: Some(self.net_manifest(profile)),
            ticket,
            ..Default::default()
        };
        // A block the request could not carry (a manifest over its limit, a name with invisible characters) stops the connect with
        // the reason, rather than going out as a request with nothing in it.
        block
            .validate()
            .map_err(|e| format!("this build cannot send its manifest: {e}"))?;
        Ok(Some(block))
    }

    /// The gate a server of this engine judges connect requests with: clients must match its manifest and carry a ticket its
    /// auth provider verifies. `None` for an engine that [says nothing](Engine::is_networked), whose server judges no one
    /// ([`ServerConfig::gate`](torque_net::handshake::ServerConfig::gate)).
    pub fn connect_gate(&self, profile: &HandshakeProfile) -> Option<ManifestGate> {
        if !self.is_networked() {
            return None;
        }
        let gate = ManifestGate::new(self.net_manifest(profile));
        Some(match self.auth() {
            Some(auth) => gate.with_auth(auth),
            None => gate,
        })
    }
}

#[cfg(test)]
#[path = "net_tests.rs"]
mod tests;
