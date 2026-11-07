//! [`Engine`]: the built, frozen result that every new script engine is made from.

use std::any::Any;
use std::fmt;
use std::rc::Rc;

use torque_ts_vm::vm::Vm;

use crate::recipe::Recipe;
use crate::slots::Store;
use crate::{EngineBuilder, PluginId, PluginType, RecipeError, Stage, Warning};

/// The plugins built into recipes, resources, slots and lists. Cheap to clone (one `Rc`); a game keeps it and makes
/// every server and client script engine from it, as many times as it likes.
///
/// The host that makes a script engine calls [`replay`](Self::replay) for each stage, in stage order, at the moment that
/// stage's parts exist.
///
/// ```
/// use torque_engine::{ClientStage, Engine, EngineBuilder, Plugin, PluginId, ServerStage};
/// use torque_ts_vm::vm::Vm;
///
/// #[derive(PluginId)]
/// struct Motd;
/// impl Plugin for Motd {
///     fn build(&self, engine: &mut EngineBuilder) {
///         engine.server(ServerStage::Game, |vm, _| {
///             vm.set_global_text(b"$Server::Motd", b"Welcome");
///             Ok(())
///         });
///     }
/// }
///
/// let engine = EngineBuilder::new().add_plugins(Motd).build()?;
/// // Two server starts: each gets the recipe, each gets fresh state.
/// for _ in 0..2 {
///     let mut vm = Vm::new();
///     for &stage in ServerStage::ALL {
///         engine.replay(stage, &mut vm)?;
///     }
///     assert_eq!(vm.global("$Server::Motd").as_deref(), Some(&b"Welcome"[..]));
/// }
/// // A client gets no server recipe.
/// let mut client = Vm::new();
/// for &stage in ClientStage::ALL {
///     engine.replay(stage, &mut client)?;
/// }
/// assert_eq!(client.global("$Server::Motd"), None);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct Engine(Rc<Inner>);

struct Inner {
    plugins: Vec<PluginId>,
    net_versions: Vec<(PluginId, u32)>,
    recipes: Vec<Recipe>,
    store: Store,
    warnings: Vec<Warning>,
}

impl Engine {
    /// An empty [`EngineBuilder`].
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    pub(crate) fn new(
        plugins: Vec<PluginId>,
        net_versions: Vec<(PluginId, u32)>,
        recipes: Vec<Recipe>,
        store: Store,
        warnings: Vec<Warning>,
    ) -> Engine {
        Engine(Rc::new(Inner {
            plugins,
            net_versions,
            recipes,
            store,
            warnings,
        }))
    }

    /// Runs, on `vm`, every recipe recorded for `stage`, in order. The first recipe that fails stops the replay; its
    /// error names the plugin and the stage.
    pub fn replay(&self, stage: impl Into<Stage>, vm: &mut Vm) -> Result<(), RecipeError> {
        let stage = stage.into();
        for recipe in self.0.recipes.iter().filter(|r| r.stage == stage) {
            (recipe.run)(vm, self).map_err(|message| RecipeError {
                plugin: recipe.plugin,
                stage,
                message,
            })?;
        }
        Ok(())
    }

    /// The plugins whose recipes [`replay`](Self::replay) runs at `stage`, in that order (a plugin with two recipes
    /// there shows twice).
    pub fn recipes(&self, stage: impl Into<Stage>) -> impl Iterator<Item = PluginId> + '_ {
        let stage = stage.into();
        self.0
            .recipes
            .iter()
            .filter(move |r| r.stage == stage)
            .map(|r| r.plugin)
    }

    /// The plugins built, in the order their `build` finished: a plugin added by another one's `build` comes before
    /// it.
    pub fn plugins(&self) -> &[PluginId] {
        &self.0.plugins
    }

    /// Whether the plugin with the id of type `P` was built into this engine (a disabled one was not).
    pub fn is_plugin_added<P: PluginType>(&self) -> bool {
        self.0.plugins.contains(&P::ID)
    }

    /// Whether the plugin with the id `id` was built into this engine.
    pub fn is_plugin_added_id(&self, id: &str) -> bool {
        self.0.plugins.iter().any(|p| *p == id)
    }

    /// What the build skipped or changed and went on: names that matched no plugin, plugins turned off for a disabled
    /// requirement. The host shows them (the console, a log).
    pub fn warnings(&self) -> &[Warning] {
        &self.0.warnings
    }

    /// The enabled plugins that have a [`net_version`](crate::Plugin::net_version), with it, sorted by id: what a server
    /// and a client compare when one connects.
    pub fn net_versions(&self) -> &[(PluginId, u32)] {
        &self.0.net_versions
    }

    /// The resource of type `T`.
    pub fn resource<T: Any>(&self) -> Option<&T> {
        self.0.store.resource()
    }

    /// The value of the slot of type `T`, if a plugin set it.
    pub fn slot<T: ?Sized + 'static>(&self) -> Option<Rc<T>> {
        self.0.store.slot()
    }

    /// The list of `T`s, in the order the plugins pushed them.
    pub fn list<T: Any>(&self) -> &[T] {
        self.0.store.list()
    }
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("plugins", &self.0.plugins)
            .field("net_versions", &self.0.net_versions)
            .field("recipes", &self.0.recipes.len())
            .finish_non_exhaustive()
    }
}
