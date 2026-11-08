//! [`ModsPlugin`] (`torque.mods`), its config and the registry that fills the rule slots.

use std::path::PathBuf;
use std::rc::Rc;

use torque_engine::{ClientStage, EngineBuilder, Plugin, Prefs, ServerStage};

use crate::layout::{
    ModEnableList, ModLayout, ModNames, ModOrder, ModPrereqs, ModStages, ModTomlLayout,
};
use crate::mods::Mods;
use crate::package::ModLimits;
use crate::plan::ModSide;

/// What game.toml's `[mods]` section says (the engine's config reader stores it as a resource).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModsConfig {
    /// `enabled = false` turns the plugin off (`torque.mods` among `[plugins] disable`).
    pub enabled: bool,
    /// `folder`: the folder of the game's files the default layout searches (`mods`).
    pub folder: String,
    /// `disable`: ids of mods that start disabled.
    pub disable: Vec<String>,
}

impl Default for ModsConfig {
    fn default() -> ModsConfig {
        ModsConfig {
            enabled: true,
            folder: crate::manifest::DEFAULT_FOLDER.to_string(),
            disable: Vec::new(),
        }
    }
}

/// What the plugin was given, for the loader to read when a script engine is made.
pub(crate) struct ModsOptions {
    pub(crate) limits: ModLimits,
    pub(crate) layout: ModTomlLayout,
}

/// The prefs of the mods: declared for the script reference and the default value, read by the default enable list.
#[allow(dead_code)]
#[derive(Prefs)]
#[prefs(prefix = "$Pref::Mods")]
struct ModPrefs {
    /// The ids of the mods that are switched off, separated by spaces. The game's prefs file keeps it.
    #[pref(default = "")]
    disabled: String,
}

/// Local mods: folders and zips of scripts and assets that are found and ordered when a server or a client starts and mounted over
/// the game's files once its entry script has run, with their scripts run by stage as TorqueScript packages. It is in `DefaultPlugins` and
/// `MinimalPlugins`; turn it off with `disable::<ModsPlugin>()` or `[mods] enabled = false` in game.toml.
///
/// With nothing else set, mods are `mod.toml` folders and zips in the game's `mods` folder, loaded in dependency order,
/// each running its `server.cs` on servers and its `client.cs` on clients. The rules are slots:
/// [`EngineMods::mods`] replaces them.
#[derive(Clone, Debug, torque_engine::PluginId)]
#[plugin(id = "torque.mods")]
pub struct ModsPlugin {
    limits: ModLimits,
    layout: ModTomlLayout,
}

impl Default for ModsPlugin {
    fn default() -> ModsPlugin {
        ModsPlugin::new()
    }
}

impl ModsPlugin {
    /// The plugin with the default limits and layout.
    pub fn new() -> ModsPlugin {
        ModsPlugin {
            limits: ModLimits::default(),
            layout: ModTomlLayout::default(),
        }
    }

    /// Sets the caps on what mods may hold. They apply to every layout.
    pub fn limits(mut self, limits: ModLimits) -> ModsPlugin {
        self.limits = limits;
        self
    }

    /// Searches the game-files folder `folder` for mods instead of game.toml's (`mods`). Call it again to search several.
    pub fn folder(mut self, folder: impl Into<String>) -> ModsPlugin {
        self.layout.folders.push(folder.into());
        self
    }

    /// Also searches the folder `dir` on the disk (not on the web). Links inside it are not followed.
    pub fn disk(mut self, dir: impl Into<PathBuf>) -> ModsPlugin {
        self.layout.disk.push(dir.into());
        self
    }
}

impl Plugin for ModsPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.insert_resource(ModsOptions {
            limits: self.limits,
            layout: self.layout.clone(),
        });
        // The mods are found and checked first thing, before the plugins after this one read any file; nothing is mounted until the
        // entry stage, so the game's entry script sees only the game's own files.
        engine.server(ServerStage::Prepare, |vm, engine| {
            Mods::setup(vm, engine, ModSide::Server)
        });
        engine.client(ClientStage::Game, |vm, engine| {
            Mods::setup(vm, engine, ModSide::Client)
        });
        engine.server(ServerStage::Entry, |vm, _| {
            if let Some(mods) = Mods::of(vm) {
                mods.run_auto(vm);
            }
            Ok(())
        });
        engine.client(ClientStage::Entry, |vm, _| {
            if let Some(mods) = Mods::of(vm) {
                mods.run_auto(vm);
            }
            Ok(())
        });
        engine.script().prefs::<ModPrefs>();
        engine.script().natives(torque_engine::natives![
            crate::natives::get_mod_count,
            crate::natives::get_mod_list,
            crate::natives::get_mod_field,
            crate::natives::is_mod_enabled,
            crate::natives::enable_mod,
            crate::natives::disable_mod,
            crate::natives::load_mods,
            crate::natives::get_mod_stages,
            crate::natives::dump_mods,
        ]);
    }
}

/// Fills the slots that say how mods are found and loaded. Each slot holds one implementation: a second plugin that sets
/// one fails the engine's build, naming both. A slot nobody sets has the default.
///
/// ```
/// use torque_engine::EngineBuilder;
/// use torque_mods::{EngineMods, ModStages, ModsPlugin, PrereqPolicy};
///
/// let mut builder = EngineBuilder::new();
/// builder
///     .mods()
///     .stages(ModStages::new(["boot", "game"]).server(["boot"]))
///     .prereqs(PrereqPolicy::Error);
/// let engine = builder.add_plugins(ModsPlugin::new()).build()?;
/// assert!(engine.is_plugin_added::<ModsPlugin>());
/// # Ok::<(), torque_engine::BuildError>(())
/// ```
pub trait EngineMods {
    /// The slots for mods.
    fn mods(&mut self) -> ModsRegistry<'_>;
}

impl EngineMods for EngineBuilder {
    fn mods(&mut self) -> ModsRegistry<'_> {
        ModsRegistry { engine: self }
    }
}

/// The slots of [`EngineMods::mods`].
pub struct ModsRegistry<'b> {
    engine: &'b mut EngineBuilder,
}

impl ModsRegistry<'_> {
    /// Sets how mods are found and recognized ([`ModLayout`]).
    pub fn layout(&mut self, layout: impl ModLayout + 'static) -> &mut Self {
        self.engine.set_slot::<dyn ModLayout>(Rc::new(layout));
        self
    }

    /// Sets how folder and zip names map to mod names ([`ModNames`]).
    pub fn names(&mut self, names: impl ModNames + 'static) -> &mut Self {
        self.engine.set_slot::<dyn ModNames>(Rc::new(names));
        self
    }

    /// Sets how mods are put in load order ([`ModOrder`]).
    pub fn order(&mut self, order: impl ModOrder + 'static) -> &mut Self {
        self.engine.set_slot::<dyn ModOrder>(Rc::new(order));
        self
    }

    /// Sets what happens when a mod's prerequisites are not met ([`ModPrereqs`]).
    pub fn prereqs(&mut self, prereqs: impl ModPrereqs + 'static) -> &mut Self {
        self.engine.set_slot::<dyn ModPrereqs>(Rc::new(prereqs));
        self
    }

    /// Sets where enabled and disabled state comes from and is kept ([`ModEnableList`]).
    pub fn enable_list(&mut self, list: impl ModEnableList + 'static) -> &mut Self {
        self.engine.set_slot::<dyn ModEnableList>(Rc::new(list));
        self
    }

    /// Declares the load stages ([`ModStages`]).
    pub fn stages(&mut self, stages: ModStages) -> &mut Self {
        self.engine.set_slot::<ModStages>(Rc::new(stages));
        self
    }
}
