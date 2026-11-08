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
//! use torque_net::connect::SharedSecret;
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
//! let block = engine.connect_block()?.expect("a networked plugin sends a manifest");
//! let manifest = block.manifest.expect("the manifest");
//! assert_eq!(manifest.plugins[0].id, "example.rope");
//! assert_eq!(manifest.plugins[0].version, 2);
//! assert!(block.ticket.is_some());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::rc::Rc;
use std::sync::Arc;

use torque_net::chunk::ChannelLimits;
use torque_net::chunk::registry::{ChannelEntry, ChannelError, ChannelRegistry, ChunkChannel};
use torque_net::connect::{AuthProvider, ConnectBlock, ManifestGate};
use torque_net::manifest::Manifest;
use torque_net::profile::HandshakeProfile;

use crate::{Engine, EngineBuilder};

/// The auth provider slot's value.
struct AuthSlot(Arc<dyn AuthProvider>);

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
    /// Sets the auth provider: a client sends the ticket it makes ([`AuthProvider::ticket`]) with its connect request, and a
    /// server refuses a client whose ticket it does not verify ([`AuthProvider::verify`]), after the manifest check and before
    /// `onConnectRequest`. One plugin sets it; a second fails the build.
    pub fn auth<A: AuthProvider + 'static>(&mut self, provider: A) -> &mut Self {
        self.engine.set_slot(Rc::new(AuthSlot(Arc::new(provider))));
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

    fn name(&mut self, table: NameTable, name: String) -> &mut Self {
        self.engine.push(NetName { table, name });
        self
    }
}

impl Engine {
    /// The auth provider a plugin set ([`NetRegistry::auth`]).
    pub fn auth(&self) -> Option<Arc<dyn AuthProvider>> {
        self.slot::<AuthSlot>().map(|slot| Arc::clone(&slot.0))
    }

    /// The manifest of this engine for peers speaking `profile`'s protocol: every plugin with a network version, the names plugins
    /// registered, no required mods.
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
        manifest
            .with_ghosts(names(NameTable::Ghost))
            .with_datablocks(names(NameTable::Datablock))
            .with_channels(self.list::<ChannelEntry>().iter().map(ChannelEntry::name))
    }

    /// The bulk channels plugins added, in id order (sorted by name). Empty for an engine with none, whose packets are then
    /// Torque's own.
    pub fn net_channels(&self) -> Result<ChannelRegistry, ChannelError> {
        ChannelRegistry::new(self.list::<ChannelEntry>())
    }

    /// Whether this engine says anything on the wire: a networked plugin, a registered name or an auth provider.
    pub fn is_networked(&self) -> bool {
        !self.net_versions().is_empty()
            || !self.list::<NetName>().is_empty()
            || !self.list::<ChannelEntry>().is_empty()
            || self.auth().is_some()
    }

    /// What a client of this engine adds to its connect request
    /// ([`ConnectFields::with_block`](torque_net::handshake::ConnectFields::with_block)): its manifest and a ticket from its auth
    /// provider. `Ok(None)` for an engine that [says nothing](Engine::is_networked), whose request stays Torque's. An `Err` is
    /// the auth provider's reason it has no ticket, or why the block cannot be sent (shown to the player; the connect does not
    /// start).
    pub fn connect_block(&self) -> Result<Option<ConnectBlock>, String> {
        self.connect_block_for(&HandshakeProfile::default())
    }

    /// [`Engine::connect_block`] for a game whose handshake is `profile`.
    pub fn connect_block_for(
        &self,
        profile: &HandshakeProfile,
    ) -> Result<Option<ConnectBlock>, String> {
        if !self.is_networked() {
            return Ok(None);
        }
        let ticket = self.auth().map(|auth| auth.ticket()).transpose()?;
        let block = ConnectBlock {
            manifest: Some(self.net_manifest(profile)),
            ticket,
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
