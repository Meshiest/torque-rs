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
//! **What a client does with them.** A plugin that draws or simulates a ghost on a client names a handler for its class
//! ([`NetRegistry::on_ghost`]): each client tick, after the connection's packets are read, the handler hears each ghost of the
//! class that appeared, that an update changed (with the whole record, the updates merged) and that went ([`GhostEvent`]).
//! [`ClientGhosts`] answers where a ghost is.
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
use torque_ts_vm::vm::Vm;

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

/// The marker that the recipe installing the ghost handlers into each client is recorded.
struct SinkInstalls;

/// What happened to one ghost of a plugin class on a client, as a handler hears it ([`NetRegistry::on_ghost`]).
#[derive(Debug)]
pub enum GhostEvent<'a, G> {
    /// A ghost of the class appeared (its first record).
    Added {
        /// The ghost's index on the connection.
        index: u32,
        /// The record the server sent.
        record: &'a G,
    },
    /// An update reached a ghost: the record is the whole state, the update merged into what was kept.
    Changed {
        /// The ghost's index.
        index: u32,
        /// The record as kept.
        record: &'a G,
    },
    /// The server deleted the ghost, or the connection ended.
    Removed {
        /// The ghost's index.
        index: u32,
    },
}

/// A ghost event with its record as the class's record type is not known yet ([`GhostSink`]).
pub enum RawGhostEvent<'a> {
    /// See [`GhostEvent::Added`].
    Added {
        /// The ghost's index.
        index: u32,
        /// The record.
        record: &'a dyn Record,
    },
    /// See [`GhostEvent::Changed`].
    Changed {
        /// The ghost's index.
        index: u32,
        /// The record.
        record: &'a dyn Record,
    },
    /// See [`GhostEvent::Removed`].
    Removed {
        /// The ghost's index.
        index: u32,
    },
}

type SinkFn = dyn Fn(&mut Vm, RawGhostEvent<'_>);

/// A handler a plugin named for the ghosts of one class ([`NetRegistry::on_ghost`]).
#[derive(Clone)]
pub struct GhostSink {
    class: String,
    run: Rc<SinkFn>,
}

impl GhostSink {
    /// The name of the ghost class the handler is for.
    pub fn class(&self) -> &str {
        &self.class
    }

    /// Tells the handler what happened.
    pub fn run(&self, vm: &mut Vm, event: RawGhostEvent<'_>) {
        (self.run)(vm, event);
    }
}

impl std::fmt::Debug for GhostSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GhostSink({})", self.class)
    }
}

/// The ghost handlers of an engine, a script-engine extension of every client (the connection reads it).
#[derive(Debug, Default)]
pub struct GhostSinks(pub Vec<GhostSink>);

/// Where the client's ghosts are: a client's connection gives the script engine an implementation
/// ([`ClientGhostLookup`]).
pub trait ClientGhosts {
    /// Where ghost `index` was last put by the server (its position), or none when the client has no such ghost or it has
    /// no position.
    fn position(&self, index: u32) -> Option<[f32; 3]>;
}

/// The client's [`ClientGhosts`], a script-engine extension of a client that is connected to a server.
#[derive(Clone)]
pub struct ClientGhostLookup(pub Rc<dyn ClientGhosts>);

impl ClientGhostLookup {
    /// The client's lookup, if the script engine is a client's.
    pub fn of(vm: &Vm) -> Option<Rc<ClientGhostLookup>> {
        vm.extension::<ClientGhostLookup>()
    }
}

impl std::fmt::Debug for ClientGhostLookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClientGhostLookup")
    }
}

/// A handler added to the engine builder.
struct SinkEntry(GhostSink);

/// The plugin net classes of [`EngineBuilder::net`](crate::EngineBuilder::net)'s registry ([module docs](crate::net_classes)); each class is also
/// listed in the manifest ([`NetRegistry::ghost_class`], [`NetRegistry::datablock_class`]).
impl NetRegistry<'_> {
    /// Adds the ghost class `name`: every object of the console class `name` is ghosted to the engine's own clients as a
    /// `G` record, shown on them as `replicate` says.
    pub fn ghost<G: GhostRecord>(&mut self, name: &str, replicate: Replicate) -> &mut Self {
        self.add(PluginClass::ghost::<G>(name, replicate))
    }

    /// Names the handler of the ghost class `name` on a client ([module docs](crate::net_classes)): `handler` hears each
    /// ghost of the class that appeared, changed or went, once a tick, with the record as `G`. The class must be added
    /// ([`NetRegistry::ghost`]) by this plugin or another before the engine is built, and `G` must be its record type.
    pub fn on_ghost<G: GhostRecord>(
        &mut self,
        name: &str,
        handler: impl Fn(&mut Vm, GhostEvent<'_, G>) + 'static,
    ) -> &mut Self {
        let class = name.to_string();
        let run: Rc<SinkFn> = Rc::new(move |vm, event| match event {
            RawGhostEvent::Added { index, record } => {
                if let Some(record) = record.downcast_ref::<G>() {
                    handler(vm, GhostEvent::Added { index, record });
                }
            }
            RawGhostEvent::Changed { index, record } => {
                if let Some(record) = record.downcast_ref::<G>() {
                    handler(vm, GhostEvent::Changed { index, record });
                }
            }
            RawGhostEvent::Removed { index } => handler(vm, GhostEvent::Removed { index }),
        });
        self.engine.push(SinkEntry(GhostSink { class, run }));
        if self.engine.resource::<SinkInstalls>().is_none() {
            self.engine.insert_resource(SinkInstalls);
            self.engine.client(ClientStage::Classes, |vm, engine| {
                let sinks = engine
                    .list::<SinkEntry>()
                    .iter()
                    .map(|e| e.0.clone())
                    .collect();
                vm.set_extension(Rc::new(GhostSinks(sinks)));
                Ok(())
            });
        }
        self
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
