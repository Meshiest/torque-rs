//! The engine builder: [`Plugin`]s, [`PluginGroup`]s, and the [`Engine`] they build.
//!
//! A plugin is one part of the engine or of a game. When the engine is built, each plugin records what it adds into the
//! [`EngineBuilder`]: other **plugins** it builds on, **recipes** that run on every new server or client script engine at a named [`Stage`], **resources**
//! shared by all of them, **slots** that exactly one plugin may set, and **lists** that any plugin may add to. The
//! result is an [`Engine`]: frozen, cheap to clone, and replayed into each script engine as it is made, so a game can
//! start a server many times from one engine. A dedicated server replays only the server recipes.
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
//!                 let Greeting(greeting) = engine.resource().expect("the hello plugin's greeting");
//!                 let name = String::from_utf8_lossy(&call.argv[1]);
//!                 Native::Str(format!("{greeting}, {name}!").into_bytes())
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
mod plugin;
mod recipe;
mod slots;

#[cfg(test)]
mod tests;

pub use builder::{BuildError, EngineBuilder, Warning};
pub use engine::Engine;
pub use group::{PluginGroup, PluginGroupBuilder, Plugins};
pub use plugin::{Plugin, PluginId, PluginSet, PluginType, Requires};
pub use recipe::{ClientStage, RecipeError, ServerStage, Stage};
/// Implements [`PluginType`] for a plugin type: the id is the package's name and the type's, as a path
/// (`my_game::ArenaPlugin`), or `#[plugin(id = "...")]`. `#[plugin(crate = path)]` names this crate when it is reached
/// through another path, such as `torque::engine`. See [`PluginType`].
pub use torque_macros::PluginId;
