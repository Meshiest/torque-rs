//! [`ModsPlugin`] (`torque.mods`), its config and the registry that fills the rule slots.

use std::path::PathBuf;
use std::rc::Rc;

use torque_engine::{ClientStage, ClientTick, EngineBuilder, Plugin, Prefs, ServerStage};
use torque_net::chunk::ChannelLimits;

use crate::layout::{
    ModEnableList, ModLayout, ModNames, ModOrder, ModPrereqs, ModStages, ModTomlLayout,
};
use crate::mods::Mods;
use crate::package::ModLimits;
use crate::plan::ModSide;
use crate::share::ModShare;
use crate::transfer::{CHANNEL, CacheChoice, DownloadLimits, ModsChannel, NetOptions};

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
    pub(crate) net: Option<NetOptions>,
}

/// The prefs of the mods: declared for the script reference and the default value, read by the default enable list.
#[allow(dead_code)]
#[derive(Prefs)]
#[prefs(prefix = "$Pref::Mods")]
pub(crate) struct ModPrefs {
    /// The ids of the mods that are switched off, separated by spaces. The game's prefs file keeps it.
    #[pref(default = "")]
    disabled: String,
    /// Whether to download the content of mods a server requires when you do not have it. Off, a server that needs a mod you lack
    /// refuses you with the reason instead.
    #[pref(default = true)]
    pub(crate) download: bool,
    /// The most megabytes to take from one server (0: the game's limit). It never raises the game's limit.
    #[pref(default = 0, range = 0..=4096)]
    pub(crate) download_megabytes: i32,
    /// The most megabytes of downloaded mod content to keep (0: the game's limit).
    #[pref(default = 0, range = 0..=65536)]
    pub(crate) cache_megabytes: i32,
    /// Whether a server may match mods you downloaded from other servers. Off, each server matches only what it sent you, so it
    /// cannot learn which other servers you have joined; on, a mod you already have from anywhere is not downloaded again, and a
    /// server can tell which mods you have by which blocks you skip.
    #[pref(default = false)]
    pub(crate) share_cache: bool,
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
    net: Option<NetOptions>,
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
            net: None,
        }
    }

    /// Lets a server require mods of its clients, and clients download the content they lack ([module docs](crate)). The plugin
    /// then has a network version and a bulk channel: a server and a client of the game must both have it, and a plain Torque client
    /// cannot join such a server. Which mods a server requires is [`EngineMods::mods`]' `share` slot (by default a mod's own
    /// `share` in `mod.toml`, else required when it has content).
    pub fn required_mods(mut self) -> ModsPlugin {
        self.net.get_or_insert_with(|| NetOptions {
            limits: DownloadLimits::default(),
            allow_downloads: true,
            cache: CacheChoice::default(),
            server: None,
        });
        self
    }

    /// Sets the caps on what a server sends and a client takes ([`DownloadLimits`]); turns on [`required_mods`](Self::required_mods).
    pub fn downloads(mut self, limits: DownloadLimits) -> ModsPlugin {
        self = self.required_mods();
        if let Some(net) = &mut self.net {
            net.limits = limits;
        }
        self
    }

    /// Whether a server sends content to clients that lack it (on by default; off, clients that lack a required mod are refused).
    pub fn allow_downloads(mut self, allow: bool) -> ModsPlugin {
        self = self.required_mods();
        if let Some(net) = &mut self.net {
            net.allow_downloads = allow;
        }
        self
    }

    /// Where a client keeps what it downloaded ([`CacheChoice`]: a per-user folder by default).
    pub fn cache(mut self, cache: CacheChoice) -> ModsPlugin {
        self = self.required_mods();
        if let Some(net) = &mut self.net {
            net.cache = cache;
        }
        self
    }

    /// Names the server a client joins, for the cache's keys: entries are kept by the server they came from, and a link whose
    /// address is the same for every server (a browser's) needs this to tell them apart (`server_name_of_url`, or `server_name_of_room`).
    /// A browser's storage is used only for a client that names its server.
    pub fn cache_server(mut self, name: impl Into<String>) -> ModsPlugin {
        self = self.required_mods();
        if let Some(net) = &mut self.net {
            net.server = Some(name.into());
        }
        self
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
    fn net_version(&self) -> Option<u32> {
        self.net.as_ref().map(|_| 1)
    }

    /// In the engine's protocol a client asks for blocks with the RPC `torque.mods.get` (`crate::transfer::register_get_rpc`);
    /// in Torque's it stays the remote command, and the manifest is what it was.
    fn finish(&self, engine: &mut EngineBuilder) {
        if self.net.is_some() && engine.protocol() == torque_engine::ProtocolKind::Engine {
            crate::transfer::register_get_rpc(engine);
        }
    }

    fn build(&self, engine: &mut EngineBuilder) {
        engine.insert_resource(ModsOptions {
            limits: self.limits,
            layout: self.layout.clone(),
            net: self.net.clone(),
        });
        if self.net.is_some() {
            // The channel carries the offer and runs of blocks, each at most a couple of megabytes; a connection in a burst may
            // fill its packets with them, so the per-packet budget is the burst's.
            let channel = ChannelLimits::new(crate::transfer::BURST_BUDGET)
                .with_max_message(crate::transfer::MAX_MESSAGE)
                .with_max_backlog(2 * crate::transfer::MAX_MESSAGE);
            engine
                .net()
                .channel_with(CHANNEL, channel, ModsChannel::default);
            engine.push(ClientTick::new(Mods::client_tick));
        }
        // The mods are found and checked first thing, before the plugins after this one read any file; nothing is mounted until the
        // entry stage, so the game's entry script sees only the game's own files.
        engine.server(ServerStage::Prepare, |vm, engine| {
            Mods::setup(vm, engine, ModSide::Server)?;
            if let Some(mods) = Mods::of(vm) {
                mods.attach_server(vm);
            }
            Ok(())
        });
        engine.client(ClientStage::Game, |vm, engine| {
            Mods::setup(vm, engine, ModSide::Client)?;
            if let Some(mods) = Mods::of(vm) {
                mods.attach_client(vm);
            }
            Ok(())
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
        // Its prefs say which mods load and how much a download may take.
        engine
            .script()
            .with_effects(torque_engine::Effects::ENGINE, |r| {
                r.prefs::<ModPrefs>();
            });
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
            crate::natives::get_required_mod_count,
            crate::natives::get_required_mod_list,
            crate::natives::get_required_mod_field,
            crate::natives::get_mod_download_state,
            crate::natives::get_mod_download_progress,
            crate::natives::clear_mod_download_cache,
            crate::natives::server_cmd_get,
            crate::natives::server_cmd_ready,
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

    /// Sets what a server asks of clients for each of its mods ([`ModShare`]).
    pub fn share(&mut self, share: impl ModShare + 'static) -> &mut Self {
        self.engine.set_slot::<dyn ModShare>(Rc::new(share));
        self
    }

    /// Declares the load stages ([`ModStages`]).
    pub fn stages(&mut self, stages: ModStages) -> &mut Self {
        self.engine.set_slot::<ModStages>(Rc::new(stages));
        self
    }
}
