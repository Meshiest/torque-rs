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

#![warn(missing_docs)]

// The derive names this crate by its path, which inside the crate (its tests) is this.
extern crate self as torque_engine;

mod builder;
mod engine;
mod group;
pub mod param;
mod plugin;
mod recipe;
mod run;
pub mod script;
mod slots;

#[cfg(test)]
mod macro_tests;
#[cfg(test)]
mod script_tests;
#[cfg(test)]
mod tests;

pub use builder::{BuildError, EngineBuilder, Warning};
pub use engine::Engine;
pub use group::{PluginGroup, PluginGroupBuilder, Plugins};
pub use param::{
    Cx, IntoRecipe, Local, Param, ParamError, ParamKind, RecipeReturn, Res, ResMut, ResourceError,
    Slot, This, VmMut, VmRef,
};
pub use plugin::{Plugin, PluginId, PluginSet, PluginType, Requires};
pub use recipe::{ClientStage, ClientTick, RecipeError, ServerStage, Stage};
pub use run::{RunError, Runner};
pub use script::{IntoNative, ScriptRegistry, TypedNative};
/// Implements [`PluginType`] for a plugin type: the id is the package's name and the type's, as a path
/// (`my_game::ArenaPlugin`), or `#[plugin(id = "...")]`. `#[plugin(crate = path)]` names this crate when it is reached
/// through another path, such as `torque::engine`. See [`PluginType`].
pub use torque_macros::PluginId;
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
pub use torque_ts_vm::convert::{Rest, ScriptError};
