//! [`EngineBuilder`]: the plugin list, what the plugins record, and the build that turns both into an [`Engine`].

use std::any::Any;
use std::fmt;
use std::rc::Rc;

use torque_ts_vm::vm::Vm;

use crate::group::Entry;
use crate::param::IntoRecipe;
use crate::plugin::{Requires, ids_of};
use crate::recipe::{Recipe, RecipeFn};
use crate::script::{ScriptRegistry, ScriptTable, Side};
use crate::slots::Store;
use crate::{
    ClientStage, Engine, PluginGroupBuilder, PluginId, PluginSet, PluginType, Plugins, ServerStage,
    Stage,
};

/// What recipes, slots and the rest are filed under when they are recorded outside any plugin's `build`.
const OUTSIDE: PluginId = PluginId::new("engine");

/// Collects plugins and what they record, then builds an [`Engine`].
///
/// Plugins are added with [`add_plugins`](Self::add_plugins) and turned off or on by type with
/// [`disable`](Self::disable) and [`enable`](Self::enable). [`build`](Self::build) orders them and runs every plugin's
/// [`build`](crate::Plugin::build), then every plugin's [`finish`](crate::Plugin::finish). Both get this builder, to
/// record into:
///
/// - **Plugins** ([`add_plugins`](Self::add_plugins)): a plugin may add the plugins it builds on. They build at once,
///   before the rest of its `build`. A plugin type is added once: a second add is an error
///   ([`BuildError::DuplicatePlugin`]), so check [`is_plugin_added`](Self::is_plugin_added) first, or name it in
///   [`requires`](crate::Plugin::requires), which adds it only when it is missing.
/// - **Recipes** ([`server`](Self::server), [`client`](Self::client)) run on every new script engine of that side, at
///   their stage, in the order they were recorded: plugin order, and within it the plugin's own order. A plugin added
///   by another one's `build` records its recipes there, between that plugin's earlier and later ones. Recipes recorded
///   in `finish` come after every one recorded in `build`.
/// - **Toggles** ([`disable`](Self::disable), [`enable`](Self::enable)) are made before [`build`](Self::build); one
///   made while plugins build is refused with a [`Warning::ToggleDuringBuild`] (a plugin that does not want another
///   one does not add or require it).
/// - **Resources** ([`insert_resource`](Self::insert_resource)): one value per type, shared by every script engine the
///   engine makes. The built engine lends them out checked at run time ([`Engine::resource`],
///   [`Engine::resource_mut`], the [`Res`](crate::Res) and [`ResMut`](crate::ResMut) parameters).
/// - **Script functions, methods and globals** ([`script`](Self::script)): registered into every script engine; a name
///   taken by two plugins fails the build ([`BuildError::DuplicateScriptName`]).
/// - **Slots** ([`set_slot`](Self::set_slot)): one value per type that exactly one plugin may set, such as the game's
///   simulation classes; a second setter fails the build ([`BuildError::DuplicateSlot`]).
/// - **Lists** ([`push`](Self::push)): values of one type that any plugin may add to, in plugin order.
///
/// **Config files** (a game's `game.toml`) name plugins by id: [`disable_id`](Self::disable_id) and
/// [`enable_id`](Self::enable_id), with [`contains`](Self::contains) and [`ids`](Self::ids) to see what this build of
/// the engine has. Code uses the typed calls. A name that matches no plugin is skipped with a [`Warning`]
/// ([`Engine::warnings`]).
///
/// ```
/// use torque_engine::{EngineBuilder, Plugin, PluginId, PluginType, Warning};
///
/// #[derive(PluginId)]
/// #[plugin(id = "torque.gamepad")]
/// struct Gamepad;
/// impl Plugin for Gamepad {
///     fn build(&self, _engine: &mut EngineBuilder) {}
/// }
///
/// // In code: by type.
/// let engine = EngineBuilder::new().add_plugins(Gamepad).disable::<Gamepad>().build()?;
/// assert!(engine.plugins().is_empty());
///
/// // From a config file: by id, applied after every plugin is added.
/// let mut builder = EngineBuilder::new();
/// builder.add_plugins(Gamepad);
/// for id in ["torque.gamepad", "torque.gampad"] {
///     builder.disable_id(id);
/// }
/// let engine = builder.build()?;
/// assert!(engine.plugins().is_empty());
/// // The misspelled id is skipped, with a warning.
/// assert!(matches!(&engine.warnings()[0], Warning::UnknownPlugin { id, .. } if id == "torque.gampad"));
/// # Ok::<(), torque_engine::BuildError>(())
/// ```
pub struct EngineBuilder {
    entries: Vec<Entry>,
    /// `(first, then)`: `first` builds before `then`.
    order: Vec<(String, String)>,
    toggles: Vec<Toggle>,
    /// Per entry: turned off because a plugin it requires is disabled or missing.
    off: Vec<bool>,
    /// Per entry: added as a requirement's default.
    defaulted: Vec<bool>,
    /// Per entry: a default no plugin that is on requires any more, taken out of the list.
    dropped: Vec<bool>,
    /// Per entry: whether its `build` has started.
    built: Vec<bool>,
    /// The entries whose `build` ran, in that order.
    build_order: Vec<usize>,
    building: bool,
    current: PluginId,
    recipes: Vec<Recipe>,
    store: Store,
    script: ScriptTable,
    warnings: Vec<Warning>,
    error: Option<BuildError>,
}

/// An `enable` or `disable` call.
struct Toggle {
    id: String,
    enabled: bool,
}

impl Default for EngineBuilder {
    fn default() -> EngineBuilder {
        EngineBuilder {
            entries: Vec::new(),
            order: Vec::new(),
            toggles: Vec::new(),
            off: Vec::new(),
            defaulted: Vec::new(),
            dropped: Vec::new(),
            built: Vec::new(),
            build_order: Vec::new(),
            building: false,
            current: OUTSIDE,
            recipes: Vec::new(),
            store: Store::default(),
            script: ScriptTable::default(),
            warnings: Vec::new(),
            error: None,
        }
    }
}

impl EngineBuilder {
    /// An empty builder; the same as [`Engine::builder`].
    pub fn new() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Adds a plugin, a group, or a tuple of those, at the end of the list. Called from a plugin's `build`, the new
    /// plugins build at once (with what they require), before the caller's `build` goes on.
    pub fn add_plugins<M>(&mut self, plugins: impl Plugins<M>) -> &mut EngineBuilder {
        let mut group = PluginGroupBuilder::new();
        plugins.add_to(&mut group);
        self.warnings.append(&mut group.warnings);
        self.order.append(&mut group.order);
        let mut added = Vec::new();
        for entry in group.entries {
            if let Some(i) = self.insert(entry) {
                added.push(i);
            }
        }
        if self.building {
            self.process(added);
        }
        self
    }

    /// Whether a plugin with the id of type `P` has been added (enabled or not).
    pub fn is_plugin_added<P: PluginType>(&self) -> bool {
        self.is_plugin_added_id(P::ID.as_str())
    }

    /// Whether a plugin with the id `id` has been added (enabled or not): [`is_plugin_added`](Self::is_plugin_added) for
    /// a plugin that knows the others only by id.
    pub fn is_plugin_added_id(&self, id: &str) -> bool {
        self.position(id).is_some()
    }

    /// Turns the plugins of the types `S` (one type or a tuple, `disable::<(AudioPlugin, GizmoPlugin)>()`) off: they do
    /// not build, and their recipes never run. The calls apply in order to each plugin when the engine is built, so
    /// they may come before the plugin is added. A type that is never added is skipped with a
    /// [`Warning::UnknownPlugin`].
    pub fn disable<S: PluginSet>(&mut self) -> &mut EngineBuilder {
        for id in ids_of::<S>() {
            self.toggle(id.as_str(), false);
        }
        self
    }

    /// Turns the plugins of the types `S` on, such as ones their group added disabled. Applied like
    /// [`disable`](Self::disable).
    pub fn enable<S: PluginSet>(&mut self) -> &mut EngineBuilder {
        for id in ids_of::<S>() {
            self.toggle(id.as_str(), true);
        }
        self
    }

    /// [`disable`](Self::disable) by id: the path for config files.
    pub fn disable_id(&mut self, id: &str) -> &mut EngineBuilder {
        self.toggle(id, false)
    }

    /// [`enable`](Self::enable) by id: the path for config files.
    pub fn enable_id(&mut self, id: &str) -> &mut EngineBuilder {
        self.toggle(id, true)
    }

    /// Whether a plugin with the id `id` has been added (enabled or not), so a config reader can tell the ids this build
    /// of the engine has.
    pub fn contains(&self, id: &str) -> bool {
        self.position(id).is_some()
    }

    /// Records a warning of the caller's own (a config file's unknown key, say) with the build's, for the host to show
    /// ([`Engine::warnings`]).
    pub fn warn(&mut self, message: impl Into<String>) -> &mut EngineBuilder {
        self.warnings.push(Warning::Message(message.into()));
        self
    }

    /// The ids of the plugins added so far, in add order, disabled ones too.
    pub fn ids(&self) -> impl Iterator<Item = PluginId> + '_ {
        self.entries
            .iter()
            .zip(&self.dropped)
            .filter(|(_, dropped)| !**dropped)
            .map(|(entry, _)| entry.id)
    }

    /// The plugin whose `build` or `finish` is running (`engine` outside them).
    pub fn current_plugin(&self) -> PluginId {
        self.current
    }

    /// Records a recipe that runs on every server script engine at `stage`.
    pub fn server(
        &mut self,
        stage: ServerStage,
        recipe: impl Fn(&mut Vm, &Engine) -> Result<(), String> + 'static,
    ) -> &mut EngineBuilder {
        self.push_recipe(stage.into(), Box::new(recipe))
    }

    /// Records a recipe that runs on every client script engine at `stage` (a dedicated server makes none).
    pub fn client(
        &mut self,
        stage: ClientStage,
        recipe: impl Fn(&mut Vm, &Engine) -> Result<(), String> + 'static,
    ) -> &mut EngineBuilder {
        self.push_recipe(stage.into(), Box::new(recipe))
    }

    /// Records a recipe written as a function of [injected parameters](crate::param), at a server or a client `stage`:
    /// it names what it needs (`VmMut`, `Res<T>`, `ResMut<T>`, `Slot<dyn T>`, `&Engine`, `Local<T>`) and returns
    /// nothing or `Result<(), E>`. A parameter that cannot be fetched (a missing resource) stops the replay with its
    /// error. The closure form that [`server`](Self::server) takes works here too.
    ///
    /// ```
    /// use torque_engine::{EngineBuilder, Local, ServerStage, VmMut};
    ///
    /// // Counts the servers made from this engine: the recipe's own state outlives each replay.
    /// fn count(mut vm: VmMut, mut starts: Local<u32>) {
    ///     *starts += 1;
    ///     vm.set_global("$Server::Starts", &starts.to_string());
    /// }
    ///
    /// let mut builder = EngineBuilder::new();
    /// builder.recipe(ServerStage::Game, count);
    /// let engine = builder.build()?;
    /// for starts in ["1", "2"] {
    ///     let mut vm = torque_ts_vm::vm::Vm::new();
    ///     engine.replay(ServerStage::Game, &mut vm)?;
    ///     assert_eq!(vm.global("$Server::Starts").as_deref(), Some(starts.as_bytes()));
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn recipe<M>(
        &mut self,
        stage: impl Into<Stage>,
        recipe: impl IntoRecipe<M>,
    ) -> &mut EngineBuilder {
        self.push_recipe(stage.into(), recipe.into_recipe())
    }

    /// The script registry for every script engine, server and client: console functions, methods and globals (see
    /// [`ScriptRegistry`]).
    pub fn script(&mut self) -> ScriptRegistry<'_> {
        ScriptRegistry::new(self, Side::Both)
    }

    /// The script registry for the servers' script engines only.
    pub fn server_script(&mut self) -> ScriptRegistry<'_> {
        ScriptRegistry::new(self, Side::Server)
    }

    /// The script registry for the clients' script engines only (a dedicated server makes none).
    pub fn client_script(&mut self) -> ScriptRegistry<'_> {
        ScriptRegistry::new(self, Side::Client)
    }

    pub(crate) fn script_table(&mut self) -> &mut ScriptTable {
        &mut self.script
    }

    /// Stores `value` as the resource of its type, replacing one stored before.
    pub fn insert_resource<T: Any>(&mut self, value: T) -> &mut EngineBuilder {
        self.store.insert_resource(value);
        self
    }

    /// Stores `T::default()` as the resource of type `T`, unless there is one already.
    pub fn init_resource<T: Any + Default>(&mut self) -> &mut EngineBuilder {
        self.store.init_resource::<T>();
        self
    }

    /// The resource of type `T`.
    pub fn resource<T: Any>(&self) -> Option<&T> {
        self.store.resource()
    }

    /// The resource of type `T`, to change while the engine is built.
    pub fn resource_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.store.resource_mut()
    }

    /// Sets the exclusive slot of type `T` (usually a `dyn Trait`). A slot is set by one plugin only: a second setter
    /// fails the build, naming both plugins, and the first value stays.
    pub fn set_slot<T: ?Sized + 'static>(&mut self, value: Rc<T>) -> &mut EngineBuilder {
        if let Err(taken) = self.store.set_slot(self.current, value) {
            let second = self.current;
            self.fail(BuildError::DuplicateSlot {
                slot: taken.slot,
                first: taken.first,
                second,
            });
        }
        self
    }

    /// The value of the slot of type `T`, if a plugin set it.
    pub fn slot<T: ?Sized + 'static>(&self) -> Option<Rc<T>> {
        self.store.slot()
    }

    /// Adds `item` to the end of the list of `T`s.
    pub fn push<T: Any>(&mut self, item: T) -> &mut EngineBuilder {
        self.store.push(item);
        self
    }

    /// The list of `T`s so far, in the order they were pushed.
    pub fn list<T: Any>(&self) -> &[T] {
        self.store.list()
    }

    /// Builds the engine, leaving this builder empty.
    ///
    /// The plugins are ordered (add order, moved by [`requires`](crate::Plugin::requires) and a group's
    /// `add_before`/`add_after`), then each builds and each finishes. The first error is returned: a plugin type or id
    /// added twice, an ordering cycle, or a slot set twice. Names that match no plugin and plugins turned off for a
    /// disabled requirement are [`warnings`](Engine::warnings), not errors.
    pub fn build(&mut self) -> Result<Engine, BuildError> {
        let mut this = std::mem::take(self);
        this.building = true;
        let all = (0..this.entries.len()).collect();
        this.process(all);
        let mut k = 0;
        while k < this.build_order.len() && this.error.is_none() {
            let i = this.build_order[k];
            this.run_plugin(i, |plugin, engine| plugin.finish(engine));
            k += 1;
        }
        if let Some(err) = this.error.take() {
            return Err(err);
        }
        let unknown: Vec<Warning> = this
            .toggles
            .iter()
            .filter(|t| this.position(&t.id).is_none())
            .map(|t| Warning::UnknownPlugin {
                id: t.id.clone(),
                action: if t.enabled { "enable" } else { "disable" },
            })
            .collect();
        this.warnings.extend(unknown);
        let plugins: Vec<PluginId> = this
            .build_order
            .iter()
            .map(|&i| this.entries[i].id)
            .collect();
        let mut net_versions: Vec<(PluginId, u32)> = this
            .build_order
            .iter()
            .filter_map(|&i| {
                let entry = &this.entries[i];
                let plugin = entry.plugin.as_ref()?;
                plugin.net_version().map(|version| (entry.id, version))
            })
            .collect();
        net_versions.sort_unstable();
        Ok(Engine::new(
            plugins,
            net_versions,
            this.recipes,
            this.store,
            this.script.entries,
            this.warnings,
        ))
    }

    fn position(&self, id: &str) -> Option<usize> {
        (0..self.entries.len()).find(|&i| !self.dropped[i] && self.entries[i].id == id)
    }

    /// Adds `entry` unless its type or its id is taken (an error, the first one kept).
    fn insert(&mut self, entry: Entry) -> Option<usize> {
        if self.position(entry.id.as_str()).is_some() {
            self.fail(BuildError::DuplicatePlugin { id: entry.id });
            return None;
        }
        self.entries.push(entry);
        self.off.push(false);
        self.defaulted.push(false);
        self.dropped.push(false);
        self.built.push(false);
        Some(self.entries.len() - 1)
    }

    fn toggle(&mut self, id: &str, enabled: bool) -> &mut EngineBuilder {
        // Toggles are settled before any plugin builds, so the order, the requirements and the plugins on all agree.
        if self.building {
            self.warnings.push(Warning::ToggleDuringBuild {
                id: id.to_owned(),
                action: if enabled { "enable" } else { "disable" },
            });
        } else {
            self.toggles.push(Toggle {
                id: id.to_owned(),
                enabled,
            });
        }
        self
    }

    fn push_recipe(&mut self, stage: Stage, run: RecipeFn) -> &mut EngineBuilder {
        self.recipes.push(Recipe {
            stage,
            plugin: self.current,
            run,
        });
        self
    }

    /// Fails the build for the plugin whose `build` or `finish` is running ([`BuildError::Rejected`]): for a registry in
    /// another crate to refuse what a plugin declared, naming the plugin. The first error of a build is the one returned.
    ///
    /// ```
    /// use torque_engine::{BuildError, EngineBuilder, Plugin, PluginId};
    ///
    /// #[derive(PluginId)]
    /// #[plugin(id = "example.picky")]
    /// struct Picky;
    /// impl Plugin for Picky {
    ///     fn build(&self, engine: &mut EngineBuilder) {
    ///         engine.reject("`Player` is a class of the engine's");
    ///     }
    /// }
    ///
    /// let error = EngineBuilder::new().add_plugins(Picky).build().unwrap_err();
    /// assert!(matches!(error, BuildError::Rejected { .. }));
    /// assert_eq!(error.to_string(), "the plugin `example.picky`: `Player` is a class of the engine's");
    /// ```
    pub fn reject(&mut self, message: impl Into<String>) -> &mut EngineBuilder {
        let plugin = self.current;
        self.fail(BuildError::Rejected {
            plugin,
            message: message.into(),
        });
        self
    }

    pub(crate) fn fail(&mut self, err: BuildError) {
        self.error.get_or_insert(err);
    }

    /// Whether entry `i` is on: its group's state, then every toggle naming it in call order, unless a disabled
    /// requirement turned it off.
    fn is_on(&self, i: usize) -> bool {
        let id = self.entries[i].id;
        let toggled = self
            .toggles
            .iter()
            .filter(|t| id == t.id.as_str())
            .fold(self.entries[i].enabled, |_, t| t.enabled);
        toggled && !self.off[i]
    }

    /// Orders and builds `batch` (entries not built yet) with everything they must come after that has not built either.
    fn process(&mut self, batch: Vec<usize>) {
        if self.error.is_some() {
            return;
        }
        // Gather: the batch, what it requires (added as defaults when absent) and what it must come after.
        let mut in_batch = vec![false; self.entries.len()];
        let mut work = Vec::new();
        for i in batch {
            if !self.built[i] && !in_batch[i] {
                in_batch[i] = true;
                work.push(i);
            }
        }
        let mut before: Vec<(usize, usize)> = Vec::new();
        let mut requires: Vec<(usize, usize)> = Vec::new();
        let mut k = 0;
        while k < work.len() {
            let i = work[k];
            k += 1;
            if !self.is_on(i) {
                continue;
            }
            let mut needs = Requires::default();
            if let Some(plugin) = &self.entries[i].plugin {
                plugin.requires(&mut needs);
            }
            let mut firsts = Vec::new();
            let me = self.entries[i].id;
            for required in needs.list {
                let j = match (self.position(&required.id), required.make) {
                    (Some(j), _) => j,
                    (None, Some(make)) => match make() {
                        (Ok(id), plugin) => match self.insert(Entry::new(id, plugin)) {
                            Some(j) => {
                                self.defaulted[j] = true;
                                j
                            }
                            None => return,
                        },
                        // Its default reports an invalid id: nothing to add, so this plugin is off.
                        (Err(id), _) => {
                            self.off[i] = true;
                            self.warnings.push(Warning::InvalidId { id });
                            continue;
                        }
                    },
                    // Named by an id no plugin has: nothing to make, so this plugin is off.
                    (None, None) => {
                        self.off[i] = true;
                        self.warnings.push(Warning::RequiresMissing {
                            plugin: me,
                            requires: required.id,
                        });
                        continue;
                    }
                };
                requires.push((j, i));
                firsts.push(j);
            }
            if self.off[i] {
                continue;
            }
            let mut late = Vec::new();
            for (first, then) in &self.order {
                if me == then.as_str() {
                    if let Some(j) = self.position(first) {
                        before.push((j, i));
                        firsts.push(j);
                    }
                } else if me == first.as_str()
                    && let Some(j) = self.position(then)
                    && self.built[j]
                {
                    late.push(j);
                }
            }
            for j in late {
                self.warnings.push(Warning::OrderNotKept {
                    first: me,
                    then: self.entries[j].id,
                });
            }
            in_batch.resize(self.entries.len(), false);
            for j in firsts {
                if !self.built[j] && !in_batch[j] {
                    in_batch[j] = true;
                    work.push(j);
                }
            }
        }
        in_batch.resize(self.entries.len(), false);
        // A plugin whose requirement is off is off too; a default that no plugin still on requires is dropped.
        let mut changed = true;
        while changed {
            changed = false;
            for &(j, i) in &requires {
                if self.is_on(i) && !self.is_on(j) {
                    self.off[i] = true;
                    self.warnings.push(Warning::RequiresDisabled {
                        plugin: self.entries[i].id,
                        requires: self.entries[j].id,
                    });
                    changed = true;
                }
            }
            for j in 0..self.entries.len() {
                if self.defaulted[j]
                    && !self.dropped[j]
                    && !self.built[j]
                    && !requires.iter().any(|&(d, i)| d == j && self.is_on(i))
                {
                    self.dropped[j] = true;
                    self.off[j] = true;
                    changed = true;
                }
            }
        }
        // Order: add order, each entry after everything it must follow.
        work.sort_unstable();
        let mut firsts: Vec<Vec<usize>> = vec![Vec::new(); self.entries.len()];
        for &(j, i) in requires.iter().chain(&before) {
            if in_batch[j] && self.is_on(j) {
                firsts[i].push(j);
            }
        }
        let on: Vec<usize> = work.into_iter().filter(|&i| self.is_on(i)).collect();
        let mut walk = Walk {
            entries: &self.entries,
            firsts: &firsts,
            mark: vec![Mark::New; self.entries.len()],
            path: Vec::new(),
            out: Vec::new(),
        };
        let order = on
            .iter()
            .try_for_each(|&i| walk.visit(i))
            .map(|()| walk.out);
        let order = match order {
            Ok(order) => order,
            Err(err) => {
                self.fail(err);
                return;
            }
        };
        for i in order {
            if self.error.is_some() {
                return;
            }
            // Built already by a plugin that added a plugin needing it, or turned off by an earlier plugin's build.
            if self.built[i] || !self.is_on(i) {
                continue;
            }
            self.built[i] = true;
            self.run_plugin(i, |plugin, engine| plugin.build(engine));
            self.build_order.push(i);
        }
    }

    /// Runs `f` on entry `i`'s plugin, with this builder filed under it.
    fn run_plugin(&mut self, i: usize, f: impl FnOnce(&dyn crate::Plugin, &mut EngineBuilder)) {
        let Some(plugin) = self.entries[i].plugin.take() else {
            return;
        };
        let outer = std::mem::replace(&mut self.current, self.entries[i].id);
        f(plugin.as_ref(), self);
        self.current = outer;
        self.entries[i].plugin = Some(plugin);
    }
}

impl fmt::Debug for EngineBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineBuilder")
            .field(
                "plugins",
                &self
                    .entries
                    .iter()
                    .map(|entry| (entry.id, entry.enabled))
                    .collect::<Vec<_>>(),
            )
            .field("current", &self.current)
            .field("recipes", &self.recipes.len())
            .finish_non_exhaustive()
    }
}

/// Why an engine did not build. Every variant names the plugins involved.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum BuildError {
    /// A plugin type was added twice, or two plugin types share one id. Check
    /// [`EngineBuilder::is_plugin_added`] first, or use [`Plugin::requires`](crate::Plugin::requires) or
    /// [`PluginGroupBuilder::set`].
    DuplicatePlugin {
        /// The id.
        id: PluginId,
    },
    /// The ordering constraints (`add_before`, `add_after`, `requires`) form a cycle.
    OrderCycle {
        /// The plugins around the cycle, the first one repeated at the end: each must build before the next.
        cycle: Vec<PluginId>,
    },
    /// Two plugins set one exclusive slot.
    DuplicateSlot {
        /// The slot's type.
        slot: &'static str,
        /// The plugin that set it first (and keeps it).
        first: PluginId,
        /// The plugin that set it again.
        second: PluginId,
    },
    /// Two plugins declared one script function, method or global on a side they share (names compare without case).
    DuplicateScriptName {
        /// `"function"`, `"method"` or `"global"`: what the second plugin declared.
        kind: &'static str,
        /// The name, as the second plugin spelled it (`Namespace::name`, or a global's `$name`).
        name: String,
        /// The plugin that declared it first (and keeps it).
        first: PluginId,
        /// The plugin that declared it again.
        second: PluginId,
    },
    /// Two plugins declared one server command ([`EngineBuilder::server_command`]; names compare without case).
    DuplicateCommand {
        /// The command's function (`serverCmd` and its name), as the second plugin spelled it.
        name: String,
        /// The plugin that declared it first (and keeps it).
        first: PluginId,
        /// The plugin that declared it again.
        second: PluginId,
    },
    /// A script function's argument names do not match its function's script arguments.
    ScriptSignature {
        /// The plugin that declared it.
        plugin: PluginId,
        /// What does not match.
        message: String,
    },
    /// A plugin's registration was refused ([`EngineBuilder::reject`]): what it declared breaks a rule of the registry it
    /// went to.
    Rejected {
        /// The plugin that declared it.
        plugin: PluginId,
        /// What is wrong.
        message: String,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::DuplicatePlugin { id } => {
                write!(f, "the plugin `{id}` was added twice (or its id is taken)")
            }
            BuildError::OrderCycle { cycle } => {
                let names: Vec<&str> = cycle.iter().map(|id| id.as_str()).collect();
                write!(f, "the plugin order has a cycle: {}", names.join(" -> "))
            }
            BuildError::DuplicateSlot {
                slot,
                first,
                second,
            } => write!(
                f,
                "the slot `{slot}` is set by both `{first}` and `{second}`; only one plugin may set it"
            ),
            BuildError::DuplicateScriptName {
                kind,
                name,
                first,
                second,
            } => write!(
                f,
                "the script {kind} `{name}` is declared by both `{first}` and `{second}`; only one plugin may declare it"
            ),
            BuildError::DuplicateCommand {
                name,
                first,
                second,
            } => write!(
                f,
                "the server command `{name}` is declared by both `{first}` and `{second}`; only one plugin may declare it"
            ),
            BuildError::ScriptSignature { plugin, message }
            | BuildError::Rejected { plugin, message } => {
                write!(f, "the plugin `{plugin}`: {message}")
            }
        }
    }
}

impl std::error::Error for BuildError {}

/// Something the build skipped or changed, and went on: kept by the [`Engine`] for the host to show
/// ([`Engine::warnings`]).
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Warning {
    /// `set`, `enable` or `disable` named a plugin that is not in the list (or not in the group).
    UnknownPlugin {
        /// The id named.
        id: String,
        /// The call: `"set"`, `"enable"` or `"disable"`.
        action: &'static str,
    },
    /// `enable` or `disable` was called while plugins built, and refused: toggles are made before the build.
    ToggleDuringBuild {
        /// The id named.
        id: String,
        /// The call: `"enable"` or `"disable"`.
        action: &'static str,
    },
    /// A plugin reported an invalid id ([`Plugin::id`](crate::Plugin::id)) and was not added.
    InvalidId {
        /// The id reported.
        id: String,
    },
    /// A plugin was turned off because a plugin it requires is disabled.
    RequiresDisabled {
        /// The plugin turned off.
        plugin: PluginId,
        /// The disabled plugin it requires.
        requires: PluginId,
    },
    /// A plugin was turned off because it requires an id that no plugin has ([`Requires::id`]).
    RequiresMissing {
        /// The plugin turned off.
        plugin: PluginId,
        /// The id it requires.
        requires: String,
    },
    /// A plugin added while others built was to build before one that had built already.
    OrderNotKept {
        /// The plugin that was to come first.
        first: PluginId,
        /// The plugin that had built already.
        then: PluginId,
    },
    /// A warning a caller recorded with [`EngineBuilder::warn`].
    Message(String),
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::UnknownPlugin { id, action } => {
                write!(
                    f,
                    "{action} names `{id}`, which no plugin added has; skipped"
                )
            }
            Warning::ToggleDuringBuild { id, action } => write!(
                f,
                "{action} of `{id}` was called while plugins built; refused (toggle before the build)"
            ),
            Warning::InvalidId { id } => {
                write!(f, "a plugin reported the invalid id `{id}`; not added")
            }
            Warning::RequiresDisabled { plugin, requires } => write!(
                f,
                "the plugin `{plugin}` is off: it requires `{requires}`, which is disabled"
            ),
            Warning::RequiresMissing { plugin, requires } => write!(
                f,
                "the plugin `{plugin}` is off: it requires `{requires}`, which no plugin added has"
            ),
            Warning::OrderNotKept { first, then } => write!(
                f,
                "the plugin `{first}` was to build before `{then}`, which had built already"
            ),
            Warning::Message(message) => f.write_str(message),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    New,
    Visiting,
    Placed,
}

/// The ordering walk: add order, but before an entry is placed, every entry it must follow is placed, depth first.
struct Walk<'a> {
    entries: &'a [Entry],
    firsts: &'a [Vec<usize>],
    mark: Vec<Mark>,
    /// The entries being visited, outermost first.
    path: Vec<usize>,
    out: Vec<usize>,
}

impl Walk<'_> {
    fn visit(&mut self, i: usize) -> Result<(), BuildError> {
        match self.mark[i] {
            Mark::Placed => return Ok(()),
            Mark::Visiting => {
                let start = self.path.iter().position(|&p| p == i).unwrap_or(0);
                // `path` runs from a later plugin back to the ones it waits on; the cycle reads in build order.
                let mut cycle: Vec<PluginId> = self.path[start..]
                    .iter()
                    .rev()
                    .map(|&p| self.entries[p].id)
                    .collect();
                cycle.push(cycle[0]);
                return Err(BuildError::OrderCycle { cycle });
            }
            Mark::New => {}
        }
        self.mark[i] = Mark::Visiting;
        self.path.push(i);
        for &first in &self.firsts[i] {
            self.visit(first)?;
        }
        self.path.pop();
        self.mark[i] = Mark::Placed;
        self.out.push(i);
        Ok(())
    }
}
