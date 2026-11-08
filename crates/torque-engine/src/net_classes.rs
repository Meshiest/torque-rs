//! Plugin net classes: ghost and datablock classes a plugin adds beside TGE's ([`EngineBuilder::net`](crate::EngineBuilder::net)).
//!
//! A ghost class is the console class of the objects it ghosts: `engine.net().ghost::<RopeGhost>("Rope",
//! Replicate::Interpolated)` sends every `Rope` object (a class below `NetObject`, registered with
//! [`ScriptRegistry::class`](crate::ScriptRegistry::class) or by the game) to the engine's own clients as a `RopeGhost`
//! record, packed from the object's fields. A datablock class works the same way for the datablocks of a class
//! (`engine.net().datablock::<PropData>("PropData")`): a server sends them with the mission's datablocks, and a client that
//! has the console class too makes and preloads each one it is sent. The records are structs with
//! `#[derive(GhostRecord)]` or `#[derive(DatablockCodec)]`.
//!
//! **Ids.** Each kind is sorted by name and numbered past TGE's ids and the engine's own classes
//! ([`PluginClasses`]), the same on the server and its clients, so TGE's own ids and records are unchanged. Two plugins
//! adding a class of one name, a name of one of TGE's classes, or more classes than TGE's class id widths have room for
//! fail the build ([`BuildError::Rejected`](crate::BuildError::Rejected)).
//!
//! **Where they go.** Every server and client script engine the engine makes gets the set as its extension at the first
//! stage ([`ServerStage::Classes`], [`ClientStage::Classes`]): the network host and the client's connection read it from
//! there ([`Engine::net_classes`] gives it too).
//!
//! ```
//! use torque_engine::{DatablockCodec, EngineBuilder, GhostRecord, Replicate, ServerStage};
//! use torque_engine::net_classes::{DatablockRef, PluginClasses};
//!
//! /// What a client knows of a rope.
//! #[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
//! struct RopeGhost {
//!     /// How many pieces it is cut into: 6 bits.
//!     #[net(bits = 6)]
//!     segments: u32,
//!     length: f32,
//!     /// Sent only when the dirty mask has bit 1 (and always to a new ghost).
//!     #[net(mask = 0x2)]
//!     color: Option<[f32; 3]>,
//!     data_block: DatablockRef,
//! }
//!
//! #[derive(Debug, Clone, PartialEq, Default, DatablockCodec)]
//! struct RopeData {
//!     #[net(max_len = 64)]
//!     material: String,
//!     stiffness: f32,
//! }
//!
//! let mut builder = EngineBuilder::new();
//! builder
//!     .net()
//!     .ghost::<RopeGhost>("Rope", Replicate::Interpolated)
//!     .datablock::<RopeData>("RopeData");
//! let engine = builder.build()?;
//! let classes = engine.net_classes();
//! assert_eq!(classes.ghosts().iter().map(|c| c.name()).collect::<Vec<_>>(), ["Rope"]);
//! let mut vm = torque_ts_vm::vm::Vm::new();
//! engine.replay(ServerStage::Classes, &mut vm)?;
//! assert!(vm.extension::<PluginClasses>().is_some_and(|set| set.datablock("RopeData").is_some()));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::rc::Rc;

use torque_ghosts::plugin::PluginClassError;

pub use torque_ghosts::codec::Xf;
pub use torque_ghosts::plugin::{
    DatablockCodec, DatablockRef, FieldSpec, GhostRecord, NetField, NetRecord, PluginClass,
    PluginClasses, Record, Replicate,
};

use torque_net::classes::Kind;

use crate::{ClientStage, Engine, NetRegistry, PluginId, ServerStage};

/// A class a plugin added.
struct NetClassEntry {
    plugin: PluginId,
    class: PluginClass,
}

/// The marker that the recipes installing the set into each script engine are recorded.
struct Installs;

/// The plugin net classes of [`EngineBuilder::net`](crate::EngineBuilder::net)'s registry ([module docs](crate::net_classes)); each class is also
/// listed in the manifest ([`NetRegistry::ghost_class`], [`NetRegistry::datablock_class`]).
impl NetRegistry<'_> {
    /// Adds the ghost class `name`: every object of the console class `name` is ghosted to the engine's own clients as a
    /// `G` record, shown on them as `replicate` says.
    pub fn ghost<G: GhostRecord>(&mut self, name: &str, replicate: Replicate) -> &mut Self {
        self.add(PluginClass::ghost::<G>(name, replicate))
    }

    /// Adds the datablock class `name`: every datablock of the console class `name` goes to the engine's own clients as a
    /// `D` record.
    pub fn datablock<D: DatablockCodec>(&mut self, name: &str) -> &mut Self {
        self.add(PluginClass::datablock::<D>(name))
    }

    fn add(&mut self, class: PluginClass) -> &mut Self {
        let kept = self.engine.list::<NetClassEntry>();
        if let Some(first) = kept.iter().find(|e| e.class.name() == class.name()) {
            let message = format!(
                "`{}` already added the net class {}",
                first.plugin,
                class.name()
            );
            self.engine.reject(message);
            return self;
        }
        let all = kept.iter().map(|e| e.class).chain([class]);
        if let Err(error) = PluginClasses::new(all) {
            self.engine.reject(match error {
                PluginClassError::BadName(_) | PluginClassError::Taken(_) => {
                    format!("the net class {}: {error}", class.name())
                }
                _ => error.to_string(),
            });
            return self;
        }
        let plugin = self.engine.current_plugin();
        self.engine.push(NetClassEntry { plugin, class });
        match class.kind() {
            Kind::Object => self.ghost_class(class.name()),
            _ => self.datablock_class(class.name()),
        };
        if self.engine.resource::<Installs>().is_none() {
            self.engine.insert_resource(Installs);
            self.engine.server(ServerStage::Classes, |vm, engine| {
                vm.set_extension(Rc::new(engine.net_classes()));
                Ok(())
            });
            self.engine.client(ClientStage::Classes, |vm, engine| {
                vm.set_extension(Rc::new(engine.net_classes()));
                Ok(())
            });
        }
        self
    }
}

impl Engine {
    /// The plugins' net classes, each kind in id order (empty when no plugin added one).
    pub fn net_classes(&self) -> PluginClasses {
        // The registry refused every set that would not make one.
        PluginClasses::new(self.list::<NetClassEntry>().iter().map(|e| e.class)).unwrap_or_default()
    }

    /// The plugin that added each net class, in the order they were added.
    pub fn net_class_plugins(&self) -> impl Iterator<Item = (PluginClass, PluginId)> + '_ {
        self.list::<NetClassEntry>()
            .iter()
            .map(|e| (e.class, e.plugin))
    }
}
