//! [`Engine`]: the built, frozen result that every new script engine is made from.

use std::any::Any;
use std::fmt;
use std::rc::Rc;

use torque_ts_vm::vm::Vm;

use crate::param::{Res, ResMut, ResourceError};
use crate::recipe::Recipe;
use crate::script::{EntryKind, ScriptEntry, replay_entries};
use crate::slots::{Resources, Store};
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
    resources: Resources,
    script: Vec<ScriptEntry>,
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
        mut store: Store,
        script: Vec<ScriptEntry>,
        warnings: Vec<Warning>,
    ) -> Engine {
        let resources = store.take_resources();
        Engine(Rc::new(Inner {
            plugins,
            net_versions,
            recipes,
            store,
            resources,
            script,
            warnings,
        }))
    }

    /// Runs, on `vm`, every recipe recorded for `stage`, in order. The first recipe that fails stops the replay; its
    /// error names the plugin and the stage. At the last stage before scripts run
    /// ([`SERVER_SCRIPT_STAGE`](crate::script::SERVER_SCRIPT_STAGE), [`CLIENT_SCRIPT_STAGE`](crate::script::CLIENT_SCRIPT_STAGE)),
    /// the script registry's functions, methods and globals for that side are registered after the recipes, in the
    /// order the plugins declared them ([`ScriptRegistry`](crate::ScriptRegistry)).
    pub fn replay(&self, stage: impl Into<Stage>, vm: &mut Vm) -> Result<(), RecipeError> {
        let stage = stage.into();
        for recipe in self.0.recipes.iter().filter(|r| r.stage == stage) {
            (recipe.run)(vm, self).map_err(|message| RecipeError {
                plugin: recipe.plugin,
                stage,
                message,
            })?;
        }
        replay_entries(&self.0.script, stage, vm, self);
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

    /// The resource of type `T`, to read while the returned guard lives.
    ///
    /// # Panics
    ///
    /// When it is borrowed to change ([`resource_mut`](Self::resource_mut), a [`ResMut`] parameter); see
    /// [`try_resource`](Self::try_resource) to handle that.
    pub fn resource<T: Any>(&self) -> Option<Res<'_, T>> {
        match self.try_resource() {
            Ok(value) => Some(value),
            Err(ResourceError::Missing { .. }) => None,
            Err(error) => panic!("{error}"),
        }
    }

    /// The resource of type `T`, to change while the returned guard lives.
    ///
    /// # Panics
    ///
    /// When it is borrowed; see [`try_resource_mut`](Self::try_resource_mut) to handle that.
    pub fn resource_mut<T: Any>(&self) -> Option<ResMut<'_, T>> {
        match self.try_resource_mut() {
            Ok(value) => Some(value),
            Err(ResourceError::Missing { .. }) => None,
            Err(error) => panic!("{error}"),
        }
    }

    /// The resource of type `T`, to read; an error naming it when it is missing or borrowed to change.
    pub fn try_resource<T: Any>(&self) -> Result<Res<'_, T>, ResourceError> {
        self.0.resources.get()
    }

    /// The resource of type `T`, to change; an error naming it when it is missing or borrowed.
    pub fn try_resource_mut<T: Any>(&self) -> Result<ResMut<'_, T>, ResourceError> {
        self.0.resources.get_mut()
    }

    /// The value of the slot of type `T`, if a plugin set it.
    pub fn slot<T: ?Sized + 'static>(&self) -> Option<Rc<T>> {
        self.0.store.slot()
    }

    /// What the script registry declared, in order.
    pub(crate) fn script_entries(&self) -> impl Iterator<Item = &EntryKind> + '_ {
        self.0.script.iter().map(|entry| &entry.kind)
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
