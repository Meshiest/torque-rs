//! The engine builder: [`Plugin`]s, [`PluginGroup`]s, and the [`Engine`] they build.
//!
//! A plugin is one part of the engine or of a game. When the engine is built, each plugin records what it adds into the
//! [`EngineBuilder`]: other **plugins** it builds on, **recipes** that run on every new server or client script engine at a named [`Stage`], **resources**
//! shared by all of them, **slots** that exactly one plugin may set, and **lists** that any plugin may add to. The
//! result is an [`Engine`]: frozen, cheap to clone, and replayed into each script engine as it is made, so a game can
//! start a server many times from one engine. A dedicated server replays only the server recipes. The plugin that sets the
//! [`Runner`] slot says what [`EngineBuilder::run`] does with the built engine (open a window, serve a game).
//!
//! A plugin is known by its stable [`PluginId`](struct@PluginId), given by `#[derive(PluginId)]` or by hand
//! ([`PluginType`]), or reported at run time by a loaded extension ([`Plugin::id`]). Code names plugins by type
//! ([`EngineBuilder::disable`], [`EngineBuilder::enable`]: the type's id); a config file by id ([`EngineBuilder::disable_id`],
//! [`EngineBuilder::enable_id`]). The order is deterministic: add order, moved only by `add_before`/`add_after` in a
//! [`PluginGroupBuilder`] and by [`Plugin::requires`].
//!
//! A hello plugin, which adds a console function `hello(name)` to every server, answering with a greeting kept as a
//! resource:
//!
//! ```
//! use torque_engine::{EngineBuilder, Plugin, PluginId, ServerStage};
//! use torque_ts_vm::vm::{Native, Vm};
//!
//! /// What `hello` answers with, shared by every server the engine starts.
//! struct Greeting(String);
//!
//! /// A plugin; its id is `example.hello` (by default it would come from the crate and type names).
//! #[derive(PluginId)]
//! #[plugin(id = "example.hello")]
//! struct HelloPlugin {
//!     greeting: String,
//! }
//!
//! impl Plugin for HelloPlugin {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         engine.insert_resource(Greeting(self.greeting.clone()));
//!         // A recipe: it runs on every server script engine the engine makes.
//!         engine.server(ServerStage::Game, |vm, engine| {
//!             let engine = engine.clone();
//!             vm.register("hello", 2, 2, "(name) a greeting", move |_vm, call| {
//!                 let greeting = engine.resource::<Greeting>().expect("the hello plugin's greeting");
//!                 let name = String::from_utf8_lossy(&call.argv[1]);
//!                 Native::Str(format!("{}, {name}!", greeting.0).into_bytes())
//!             });
//!             Ok(())
//!         });
//!     }
//! }
//!
//! let engine = EngineBuilder::new()
//!     .add_plugins(HelloPlugin { greeting: "Hello".into() })
//!     .build()?;
//!
//! // What the host does each time it makes a server's script engine.
//! let mut vm = Vm::new();
//! for &stage in ServerStage::ALL {
//!     engine.replay(stage, &mut vm)?;
//! }
//! assert_eq!(vm.eval(b"return hello(\"world\");")?, b"Hello, world!");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Maintaining
//!
//! **A client over its server replays only its own entries.** [`Engine::replay_shared`] runs a client stage's recipes on the
//! script engine a server of the engine made, and registers only the registry's client-only entries: the entries of both sides
//! went in at the server's stage, and registering them again would set their globals and prefs back over what the server's
//! scripts wrote. [`Engine::replay`] is unchanged for every other host
//! (`a_shared_replay_registers_only_the_clients_own_entries_over_the_servers` in `script_tests.rs`).
//!
//! **A plugin datablock class's hooks are one behavior** (`class.rs`, `Hooks<T>`): `preload` runs only for a class with a
//! preload hook or an asset field, as before, and `datablock_changed` only for one with a refresh hook (`#[class(refresh = ..)]`,
//! `ScriptClass::HAS_REFRESH`), answering `true` so a server's drain does not warn about the class; the hook's `Err` is a console
//! error, never a deleted datablock (`the_refresh_hook_runs_with_the_fields_as_they_are_now_and_a_failure_is_an_error` in
//! `class_tests.rs`). A class with datablock users and neither hook is warned about once by the server's drain.
//!
//! **One dev gate.** [`script::dev_allowed`] (a debug build, or `$Pref::Dev` true) is the gate of a `dev` native, of
//! [`Assets::hot_reload_on`] and of a server's shape reload (torque-server's tick), and [`assets::HOT_RELOAD_INTERVAL_MS`] is
//! the one interval both watchers poll at: a change to either reaches every watcher.
//!
//! A native, global or pref that writes or deletes files, runs code from text, changes the engine as a whole (mods, limits,
//! quitting) or connects out declares it where it is registered ([`ScriptRegistry::with_effects`], `effects = "..."` on
//! [`function`]): the extensions' call policy reads it, and torque-plugin-loader's
//! `every_native_that_writes_or_evaluates_is_denied_to_languages` lists every native with its effects.
//!
//! **Who a connection is comes from the auth provider and nowhere else.** [`NetRegistry::auth`] sets the provider, a server's gate
//! asks it to verify each ticket ([`torque_net::connect::AuthProvider::verify`]), and what it returns is the connection's
//! [`torque_net::connect::Identity`]; every per-player key a server keeps ([`permissions::SubjectKey::of`], an upload's resume,
//! a prop's owner) is made from that or, with none, from the connection. With no provider every player is anonymous: the
//! identity exists, it is per connection, and no ticket is on the wire (the `tge` wire goldens and trace lines are unchanged).
//! A real provider gives the same account the same id on every connection, so roles and props follow it; what a provider
//! must guarantee is on [`torque_net::connect::AuthProvider::verify`], and a test's provider is
//! [`torque_net::connect::SharedSecret`] (`tests/net.rs` in torque-plugin-props runs grants and ownership through a reconnect,
//! `tests/server.rs` in torque-server checks a client cannot choose its identity). A key's text for script is
//! [`permissions::SubjectKey`]'s `Display` (`id:<hex>` or `conn:<n>`), read back by its `FromStr`
//! (`permissions::tests::a_key_reads_back_from_its_script_text_and_nothing_else_does`).

#![warn(missing_docs)]

// The derive names this crate by its path, which inside the crate (its tests) is this.
extern crate self as torque_engine;

pub mod args;
pub mod assets;
mod builder;
pub mod class;
pub mod command;
mod engine;
mod group;
pub mod net;
pub mod net_classes;
pub mod param;
pub mod permissions;
mod plugin;
pub mod predict_ids;
pub mod prefs;
mod recipe;
pub mod rpc;
mod run;
pub mod scope;
pub mod script;
mod slots;
pub mod upload;

#[cfg(test)]
mod command_tests;
#[cfg(test)]
mod macro_tests;
#[cfg(test)]
mod net_classes_tests;
#[cfg(test)]
mod script_tests;
#[cfg(test)]
mod tests;

pub use assets::{AssetError, AssetRegistry, Assets, Origin, Spec};
pub use builder::{BuildError, EngineBuilder, Warning};
pub use class::{ClassField, ClassInfo, ClassReadError, FieldDecl, ScriptClass};
pub use engine::Engine;
pub use group::{PluginGroup, PluginGroupBuilder, Plugins};
pub use net::{NetRegistry, ProtocolKind};
pub use net_classes::{DatablockCodec, GhostRecord, Replicate};
pub use param::{
    Cx, IntoRecipe, IntoServerTick, Local, Param, ParamError, ParamKind, RecipeReturn, Res, ResMut,
    ResourceError, Slot, This, VmMut, VmRef,
};
pub use plugin::{Plugin, PluginId, PluginSet, PluginType, Requires};
pub use prefs::{Pref, PrefDecl, PrefKind, PrefType, Prefs, VmPrefs};
pub use recipe::{
    ClientStage, ClientTick, RecipeError, ServerStage, ServerTick, Stage, TickInfo, TickPhase,
};
pub use run::{RunError, Runner};
pub use script::{IntoNative, NativeEntry, ScriptRegistry, Side, TypedNative};
pub use torque_ghosts::plugin::NetMessage;
/// Declares a console class as a struct with one typed field for each console field, for
/// [`ScriptRegistry::class`](ScriptRegistry::class): the struct then implements [`ScriptClass`].
///
/// `#[class(name = "RopeData", parent = "SimDataBlock", preload = RopeData::check)]`: `name` is the console class (the
/// struct's name when left out), `parent` the class it goes below (`SimObject` when left out), `preload` a function
/// `fn(&Self, &mut Vm, ObjId) -> Result<(), String>` run for each datablock of the class once it is registered, with the
/// datablock's fields read into the struct: an `Err` prints the preload failure with its text and deletes the datablock.
/// Only a class below `SimDataBlock` has a preload: the engine runs it for datablocks alone, so on any other class the
/// hook never runs. Each field takes
/// `#[field(...)]`:
///
/// - `name = ".."`: the console field's name; the Rust name in camelCase when left out (`max_length` is `maxLength`).
/// - `doc = ".."`: what it is for, when the doc comment is not it. `obj.dump()` lists it under `Field Docs:`.
/// - `range = low..=high`: a script's write is clamped to it (a warning says so), for `f32`, `i32`, `u8` and `u16`.
/// - `default = ..`: a literal, what a fresh object reads (the type's zero when left out).
/// - `asset = Type`: on a [`Filename`] field, the asset type the file is (a loader of it is added with
///   [`EngineBuilder::assets`]): each datablock's preload loads the file it names into its script engine's
///   [`Assets`], and a file that is missing, refused, over its loader's cap, untrusted or unreadable fails the
///   preload with the reason (an empty name names nothing). See [`assets`].
///
/// The field types are `i32`, `u8`, `u16`, `f32`, `bool`, `String`, [`Filename`], and the arrays `[f32; 3]`
/// (`Point3F`), `[f32; 2]` (`Point2F`), `[i32; 2]` (`Point2I`), `[f32; 4]` (`ColorF`) and `[u8; 4]` (`ColorI`)
/// ([`ClassField`]): a script writes and reads them by the console's rules for the console field type.
///
/// ```
/// use torque_engine::{EngineClass, Filename, ScriptClass};
///
/// #[derive(EngineClass)]
/// #[class(name = "RopeData", parent = "SimDataBlock")]
/// struct RopeData {
///     /// How many pieces the rope is cut into.
///     #[field(range = 2..=64, default = 8)]
///     segments: i32,
///     length: f32,
///     material: Filename,
/// }
///
/// assert_eq!(RopeData::NAME, "RopeData");
/// let names: Vec<_> = RopeData::FIELDS.iter().map(|f| f.name).collect();
/// assert_eq!(names, ["segments", "length", "material"]);
/// assert_eq!(RopeData::FIELDS[0].range_text().as_deref(), Some("2 to 64"));
/// ```
///
/// A field type that is not a console field type:
///
/// ```compile_fail,E0277
/// use torque_engine::EngineClass;
///
/// #[derive(EngineClass)]
/// struct Odd {
///     ids: Vec<i32>,
/// }
/// ```
///
/// A range of the wrong type:
///
/// ```compile_fail,E0308
/// use torque_engine::EngineClass;
///
/// #[derive(EngineClass)]
/// struct Odd {
///     #[field(range = 0.0..=1.0)]
///     count: i32,
/// }
/// ```
///
/// An asset type on a field that is not a `Filename`:
///
/// ```compile_fail,E0277
/// use torque_engine::EngineClass;
///
/// struct Words;
///
/// #[derive(EngineClass)]
/// struct Odd {
///     #[field(asset = Words)]
///     count: i32,
/// }
/// ```
pub use torque_macros::EngineClass;
/// Implements [`PluginType`] for a plugin type: the id is the package's name and the type's, as a path
/// (`my_game::ArenaPlugin`), or `#[plugin(id = "...")]`. `#[plugin(crate = path)]` names this crate when it is reached
/// through another path, such as `torque::engine`. See [`PluginType`].
pub use torque_macros::PluginId;
/// Declares a struct of typed preferences kept in `$Pref::...` globals, for
/// [`ScriptRegistry::prefs`](ScriptRegistry::prefs): the struct then implements [`Prefs`].
///
/// `#[prefs(prefix = "$Pref::Pad")]` is the globals' common start. Each field takes `#[pref(...)]`:
///
/// - `default = ..` (required): a literal of the field's type (`0.15`, `true`, `"text"`, `-1`); a script engine has the
///   global set to it before any script runs.
/// - `range = low..=high`: a read clamps the value to it.
/// - `name = "DeadZone"`: the global's last part; the field's name in PascalCase when left out (`dead_zone` is
///   `$Pref::Pad::DeadZone`).
/// - `doc = ".."`: what it is for, when the doc comment is not it.
///
/// The field types are `i32`, `u32`, `f32`, `bool` and `String` ([`PrefType`]). `crate = path` in `#[prefs(..)]` names this
/// crate when the manifest does not (it is found as `torque-engine`, or as the `torque` crate's `engine`).
///
/// ```
/// use torque_engine::Prefs;
///
/// #[derive(Prefs)]
/// #[prefs(prefix = "$Pref::Pad")]
/// struct PadPrefs {
///     /// How far the stick must move before it counts.
///     #[pref(default = 0.15, range = 0.0..=0.9)]
///     dead_zone: f32,
///     #[pref(default = true)]
///     invert_y: bool,
///     #[pref(default = "default", name = "Profile")]
///     active_profile: String,
/// }
///
/// let names: Vec<_> = PadPrefs::PREFS.iter().map(|p| p.name).collect();
/// assert_eq!(names, ["$Pref::Pad::DeadZone", "$Pref::Pad::InvertY", "$Pref::Pad::Profile"]);
/// assert_eq!(PadPrefs::PREFS[0].default, "0.15");
/// assert_eq!(PadPrefs::defaults().dead_zone, 0.15);
/// ```
///
/// A default of the wrong type, a range that does not fit the field, a default outside its range, and a field type that
/// is not a pref type do not compile. A default of the wrong type:
///
/// ```compile_fail,E0308
/// use torque_engine::Prefs;
///
/// #[derive(Prefs)]
/// #[prefs(prefix = "$Pref::Pad")]
/// struct PadPrefs {
///     #[pref(default = "fast")]
///     speed: f32,
/// }
/// ```
///
/// A range of the wrong type (whole numbers for a float):
///
/// ```compile_fail,E0308
/// use torque_engine::Prefs;
///
/// #[derive(Prefs)]
/// #[prefs(prefix = "$Pref::Pad")]
/// struct PadPrefs {
///     #[pref(default = 0.5, range = 0..=1)]
///     dead_zone: f32,
/// }
/// ```
///
/// A default outside the range:
///
/// ```compile_fail,E0080
/// use torque_engine::Prefs;
///
/// #[derive(Prefs)]
/// #[prefs(prefix = "$Pref::Pad")]
/// struct PadPrefs {
///     #[pref(default = 2.0, range = 0.0..=0.9)]
///     dead_zone: f32,
/// }
/// ```
///
/// A field type that is not a pref type:
///
/// ```compile_fail,E0277
/// use torque_engine::Prefs;
///
/// #[derive(Prefs)]
/// #[prefs(prefix = "$Pref::Pad")]
/// struct PadPrefs {
///     #[pref(default = 0)]
///     buttons: Vec<u8>,
/// }
/// ```
pub use torque_macros::Prefs;
/// Declares a free function as a TorqueScript native, for [`ScriptRegistry::natives`] (listed with [`natives!`]).
///
/// `#[function(ns = "Pad", name = "setDeadZone")]`: `ns` is the namespace (the global one when left out), `name` the
/// name scripts call (the function's name in camelCase when left out). The parameters are split by type, as
/// [`IntoNative`] splits them: script arguments ([`ScriptArg`](torque_ts_vm::convert::ScriptArg) types: numbers, truth
/// values, text, `Option<T>`, [`Rest`]) set the argument counts and the usage text, injected [`Param`]s ([`VmMut`],
/// [`Res`], [`Local`], ...) are invisible to scripts. The usage text is the signature made from the script arguments'
/// names (camelCase, or `#[arg(name = "...")]` on the parameter), then ` - ` and the doc comment's first line
/// ([`usage_text`](script::usage_text)); the whole comment is kept in the declaration ([`NativeDecl::docs`](script::NativeDecl::docs)) for the script reference.
///
/// Other options: `usage = "..."` keeps a usage text exactly as written (one that must not change, such as TGE's),
/// `description = "..."` replaces the doc comment's first line (empty for none), `dev` makes a development command
/// ([`ScriptRegistry::dev`]), `crate = path` names this crate when the manifest does not (it is found as
/// `torque-engine`, or as the `torque` crate's `engine`).
///
/// ```
/// use torque_engine::{EngineBuilder, Res, ServerStage, function, natives};
/// use torque_ts_vm::vm::Vm;
///
/// struct Greeting(String);
///
/// /// greets someone, loudly when asked
/// ///
/// /// The greeting is the engine's `Greeting` resource.
/// #[function(name = "greet")]
/// fn greeting(greeting: Res<Greeting>, #[arg(name = "who")] person: &str, loud: Option<bool>) -> String {
///     let text = format!("{}, {person}", greeting.0);
///     if loud.unwrap_or(false) { text.to_uppercase() } else { text }
/// }
///
/// let mut builder = EngineBuilder::new();
/// builder.insert_resource(Greeting("Hello".into()));
/// builder.script().natives(natives![greeting]);
/// let engine = builder.build()?;
/// let mut vm = Vm::new();
/// for &stage in ServerStage::ALL {
///     engine.replay(stage, &mut vm)?;
/// }
/// assert_eq!(vm.eval(b"return greet(\"Ann\", true);")?, b"HELLO, ANN");
/// assert!(vm.native_listing().contains("greet 2..3 greet(who [, loud]) - greets someone, loudly when asked\n"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// A signature the engine cannot run does not compile (`cargo check` finds it): a parameter type that is neither a
/// script argument nor injected, two `VmMut`s, a `VmMut` beside a `VmRef`, an `Option` before a required argument,
/// `self`, generics or `async`. A parameter type it does not know:
///
/// ```compile_fail,E0277
/// use torque_engine::function;
///
/// struct Unknown;
///
/// #[function]
/// fn odd(_value: Unknown) {}
/// ```
///
/// **The declaration's name.** Beside the function the attribute adds a hidden type with the function's own name (types
/// and functions have separate names, so both fit), which is how [`natives!`] finds the declaration by the function's
/// name. So a type, module or import of that name in the same scope collides with it (`error[E0428]: the name ... is
/// defined multiple times`, such as `fn net()` beside `mod net`): rename the function (and give the script name with
/// `name = "..."`), or move it into a module of its own.
pub use torque_macros::function;
/// Declares the fns of an impl block as TorqueScript methods of a class, for [`ScriptRegistry::natives`]: the block's
/// type is listed with [`natives!`] as all of them.
///
/// `#[methods(class = "Player")]` (the type's name when left out). Every `pub fn` of an inherent block becomes a
/// method, and every fn of a trait impl for a concrete type (`impl Tool for Weld`; not a trait definition, nor a
/// generic impl). On a fn, `#[torque(skip)]` leaves it out and `#[torque(name = "isArmed")]` renames it (camelCase of
/// the Rust name by default); `usage` and `description` are as for [`function`]. Parameters are as for
/// [`function`]. Two methods of a block with one name do not compile. A type takes one `#[methods]` block, as it
/// implements [`Natives`](script::Natives) for it; `list = Name` makes a type `Name` that does instead, for a second
/// block.
///
/// How a method reaches its object: a method with `&self` or `&mut self` is called on the object's Rust data, its
/// [`SimObject::ext`](torque_ts_vm::sim::SimObject::ext), which must be the block's type
/// ([`with_object_data`](script::with_object_data): taken out of the object for the call, so the method may also take
/// [`VmMut`] or [`VmRef`]; an object without the data is a console error). A method without a receiver takes
/// [`This`] for the object's id.
///
/// ```
/// use torque_engine::{EngineBuilder, ServerStage, This, VmRef, methods, natives};
/// use torque_ts_vm::vm::Vm;
///
/// struct Ammo {
///     rounds: i32,
/// }
///
/// #[methods(class = "SimObject")]
/// impl Ammo {
///     /// adds rounds, up to the most given, and returns the rounds now
///     pub fn give_ammo(&mut self, amount: i32, max: Option<i32>) -> i32 {
///         self.rounds = (self.rounds + amount).min(max.unwrap_or(i32::MAX));
///         self.rounds
///     }
///
///     /// the object's name
///     #[torque(name = "label")]
///     pub fn name_of(vm: VmRef, this: This) -> String {
///         let name = vm.objects().get(this.0).and_then(|o| o.name()).unwrap_or_default();
///         String::from_utf8_lossy(name).into_owned()
///     }
///
///     #[torque(skip)]
///     pub fn rounds(&self) -> i32 {
///         self.rounds
///     }
/// }
///
/// let mut builder = EngineBuilder::new();
/// builder.script().natives(natives![Ammo]);
/// let engine = builder.build()?;
/// let mut vm = Vm::new();
/// for &stage in ServerStage::ALL {
///     engine.replay(stage, &mut vm)?;
/// }
/// vm.eval(b"new SimObject(Crate);")?;
/// let id = vm.objects().find(b"Crate").expect("the object");
/// vm.objects_mut().get_mut(id).expect("the object").ext = Some(Box::new(Ammo { rounds: 5 }));
/// assert_eq!(vm.eval(b"return Crate.giveAmmo(10, 12);")?, b"12");
/// assert_eq!(vm.eval(b"return Crate.label();")?, b"Crate");
/// assert!(vm.native_listing().contains(
///     "SimObject::giveAmmo 3..4 simObject.giveAmmo(amount [, max]) - adds rounds, up to the most given, and returns the rounds now\n"
/// ));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub use torque_macros::methods;
pub use torque_ts_vm::convert::{Filename, Rest, ScriptError};
pub use torque_ts_vm::effects::Effects;

/// The record crate, for the code `#[derive(GhostRecord)]` and `#[derive(DatablockCodec)]` write.
#[doc(hidden)]
pub use torque_ghosts as __ghosts;

/// Declares a plugin's datablock record as a struct with one typed field for each value it sends, for
/// [`NetRegistry::datablock`]: the struct then implements [`DatablockCodec`]. The fields and their options are
/// [`GhostRecord`]'s, without `mask` (a datablock goes out whole). The packer reads each field from the datablock's console
/// field of its name, so one struct can be both the console class (`#[derive(EngineClass)]`) and its record.
///
/// ```
/// use torque_engine::{DatablockCodec, EngineClass};
///
/// #[derive(Debug, Clone, PartialEq, Default, EngineClass, DatablockCodec)]
/// #[class(name = "PropData", parent = "SimDataBlock")]
/// struct PropData {
///     /// The prop's mass.
///     #[field(range = 0.0..=1000.0, default = 10.0)]
///     mass: f32,
///     #[net(max_len = 64)]
///     model: String,
/// }
/// ```
///
/// A width on a float:
///
/// ```compile_fail,E0080
/// use torque_engine::DatablockCodec;
///
/// #[derive(Debug, Clone, PartialEq, Default, DatablockCodec)]
/// struct Odd {
///     #[net(bits = 8)]
///     mass: f32,
/// }
/// ```
pub use torque_macros::DatablockCodec;
/// Declares a plugin's ghost record as a struct with one typed field for each value it sends, for
/// [`NetRegistry::ghost`]: the struct then implements [`GhostRecord`] (and the record crate's `Layout` and `NetRecord`).
/// The struct must also be `Debug`, `Clone`, `PartialEq` and `Default` (a decoder starts from the default).
///
/// The fields go on the wire in the struct's order. Each takes `#[net(...)]`:
///
/// - `bits = N`: an integer's width (`u8`, `u16`, `u32`, `i32`; 1 to the type's width, the type's width when left out).
///   The packer cuts a value to it; an `i32` keeps its sign.
/// - `max_len = N`: a `String`'s most bytes (up to 255, the default) or a `Vec`'s most elements (up to 1024; 16 when left
///   out). The packer cuts to it; a decoder refuses more, so a peer cannot make it allocate past the cap.
/// - `mask = bits`: an `Option` field sent only when one of the object's dirty bits in `bits` is set (every bit is set for
///   a new ghost); a client keeps the last value it was sent when an update leaves it out. The game marks the bits
///   (`setMaskBits`).
/// - `name = ".."`: the object's console field the packer reads (when left out, the one an `#[field(name = "..")]` of
///   `#[derive(EngineClass)]` on the same struct gives, else the Rust name in camelCase).
/// - `skip`: not sent (a decoder leaves the default).
///
/// The field types are `bool`, `u8`, `u16`, `u32`, `i32`, `f32`, `[f32; N]`, `String`,
/// [`DatablockRef`](net_classes::DatablockRef) (a datablock pointer: the client takes the first as the ghost's datablock),
/// [`Xf`](net_classes::Xf) (a transform: the first is the ghost's pose), and `Option` and `Vec` of them
/// ([`NetField`](net_classes::NetField)). See [`net_classes`] for an example.
///
/// A width wider than the field's type:
///
/// ```compile_fail,E0080
/// use torque_engine::GhostRecord;
///
/// #[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
/// struct Odd {
///     #[net(bits = 12)]
///     small: u8,
/// }
/// ```
///
/// A cap on a type that has none:
///
/// ```compile_fail,E0080
/// use torque_engine::GhostRecord;
///
/// #[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
/// struct Odd {
///     #[net(max_len = 64)]
///     count: u32,
/// }
/// ```
///
/// A mask on a field that is always sent:
///
/// ```compile_fail,E0080
/// use torque_engine::GhostRecord;
///
/// #[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
/// struct Odd {
///     #[net(mask = 0x1)]
///     count: u32,
/// }
/// ```
///
/// A field type that does not travel:
///
/// ```compile_fail,E0277
/// use torque_engine::GhostRecord;
///
/// #[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
/// struct Odd {
///     table: std::collections::HashMap<u32, u32>,
/// }
/// ```
pub use torque_macros::GhostRecord;

/// Declares the request or reply of a plugin RPC as a struct with one typed field for each value, for
/// [`NetRegistry::rpc`]: the struct then implements [`NetMessage`] (and the record crate's `Layout`). The struct must also be
/// `Debug`, `Clone`, `PartialEq` and `Default`. The fields go on the wire in the struct's order and take `#[net(...)]`:
///
/// - `bits = N`: an integer's width (`u8`, `u16`, `u32`, `i32`).
/// - `max_len = N`: a `String`'s most bytes or a `Vec`'s most elements. **Required** for those types: a message has no default
///   cap, so a peer cannot make a handler allocate what the author did not choose.
/// - `finite`: a float field's decoder refuses a NaN or an infinity. **Required** for `f32`, `[f32; N]` and
///   [`Xf`](net_classes::Xf) fields.
///
/// The field types are those of a ghost record (`bool`, `u8`, `u16`, `u32`, `i32`, `f32`, `[f32; N]`, `String`,
/// [`DatablockRef`](net_classes::DatablockRef), [`Xf`](net_classes::Xf), `Option` and `Vec` of them) and
/// [`GhostRef`](net_classes::GhostRef): the index of a ghost the caller can see. A raw object id has no form on the wire and does
/// not compile.
///
/// ```
/// use torque_engine::NetMessage;
/// use torque_engine::net_classes::{GhostRef, Xf};
///
/// #[derive(Debug, Clone, PartialEq, Default, NetMessage)]
/// struct SpawnProp {
///     #[net(max_len = 64)]
///     data: String,
///     #[net(finite)]
///     at: Xf,
///     #[net(bits = 8)]
///     predict_id: u8,
///     target: GhostRef,
/// }
/// ```
///
/// A string with no cap:
///
/// ```compile_fail,E0080
/// use torque_engine::NetMessage;
///
/// #[derive(Debug, Clone, PartialEq, Default, NetMessage)]
/// struct Open {
///     data: String,
/// }
/// ```
pub use torque_macros::NetMessage;
