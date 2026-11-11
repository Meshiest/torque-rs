//! The office played with others: its server on UDP and the web, answering the network's list of matches and requiring the
//! catalog mod of its players, and the engine every client of it is made by.
//!
//! **The engine** ([`office_engine`]) is the level demo's ([`crate::plugin::level_engine`]) with the network's list of matches,
//! the mods (a server that holds the catalog requires it; a client keeps what it downloads where its cache says), the plugins
//! server scripts need (their messages, the menu's controls and its layout) and the join screen's natives. One engine makes the
//! window's client and the servers it hosts.
//!
//! **A server** ([`start_office`]) plays `missions/office.mis` over the files the engine holds, the catalog's files among them,
//! named as the list of matches shows it.
//!
//! **A client's server scripts** ([`ServerScripts`]): the realm of the connection, framed after each step of the client. The
//! game's own key for the catalog (Q, `game/client.cs`) tells the GUI the player asked for the menu, so it may take the cursor
//! again after an Escape.

use std::rc::Rc;
use std::time::Duration;

use torque::app::client::GameClient;
use torque::app::hosting::register_join_natives;
use torque::app::plugins::{ClientWiring, DefaultPlugins, GuiPlugin, ModsPlugin};
use torque::app::server::Listen;
use torque::engine::command::CommandSpec;
use torque::engine::{ClientStage, Engine, EngineBuilder, Plugin, PluginGroup as _, PluginId};
use torque::gui::client_script::ShownControl;
use torque::mods::CacheChoice;
use torque_client_script_format::ui::Kind;
use torque_plugin_client_script::{Choices, ClientScriptsPlugin, Realms, RunnerFactory};
use torque_plugin_gui_layout::GuiLayoutPlugin;

use crate::files::GameFiles;
use crate::plugin::{Level, LevelPlugin, engine_of};
use crate::server::WorldServer;

/// The office's map.
pub const OFFICE_MAP: &str = "missions/office.mis";

/// The screen that shows a map's world, which the game marks as one that shows server controls.
pub const SCREEN: &str = "PlayGui";

/// The abilities the catalog's menu asks for, as lines of the consent prompt: `ui` and `net`.
pub const ABILITIES: usize = 2;

/// The UDP port number office servers listen on by default, and the one the list of matches asks on.
pub const DEFAULT_PORT: u16 = 28_000;

/// The control classes a server's menu is made of.
#[derive(PluginId)]
#[plugin(crate = torque::engine)]
pub struct MenuControls;

impl Plugin for MenuControls {
    fn build(&self, engine: &mut EngineBuilder) {
        torque::gui::client_script::add_controls(engine);
    }
}

/// The join screen's natives (`checkJoinAddress` and the rest): what the address box of the list of matches asks.
#[derive(Clone, Copy, Default, PluginId)]
#[plugin(crate = torque::engine)]
pub struct JoinScreenPlugin;

impl Plugin for JoinScreenPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Natives, |vm, _| {
            let wiring = vm
                .extension::<ClientWiring>()
                .ok_or("the join screen runs only when a game client is made")?;
            register_join_natives(vm, wiring.state());
            Ok(())
        });
    }
}

/// The level demo's engine over `files` for playing the office with others (module docs): a server it makes holds
/// `server_mods` (a path and its bytes each: the catalog mod, `catalog::mod_files`) and requires them of its clients, and a
/// client keeps what it downloads where `cache` says.
pub fn office_engine(
    files: GameFiles,
    server_mods: Vec<(String, Vec<u8>)>,
    cache: CacheChoice,
) -> Result<EngineBuilder, String> {
    let level = LevelPlugin::with_mods(files, server_mods)?;
    let mods = ModsPlugin::new().required_mods().cache(cache);
    let mut engine = engine_of(
        level,
        DefaultPlugins
            .set(GuiPlugin { layout_fonts: true })
            .set(mods),
    )?;
    engine.add_plugins((
        ClientScriptsPlugin::new(),
        MenuControls,
        GuiLayoutPlugin,
        JoinScreenPlugin,
    ));
    // The key for the menu (Q) reaches the server as the command `Catalog`: a couple a second is plenty.
    engine.server_command_limits("Catalog", CommandSpec::new().rate(2, 4));
    Ok(engine)
}

/// How an office server is started.
#[derive(Debug, Clone)]
pub struct OfficeStart<'a> {
    /// The name the list of matches shows.
    pub name: &'a str,
    /// Whether it is a dedicated server (the list of matches says so).
    pub dedicated: bool,
}

/// An office server made by `engine` ([`office_engine`]) over its files, the mods it holds required of its clients, on
/// `listen`, for a host that ticks it (or runs it with `GameServer::run_dedicated`).
pub fn start_office(
    engine: &Engine,
    listen: Listen,
    start: &OfficeStart,
) -> Result<WorldServer, String> {
    // The shared level, not the resource's borrow, which would be held while the server starts.
    let level = Rc::clone(
        &*engine
            .resource::<Rc<Level>>()
            .ok_or("the engine has no level demo (LevelPlugin)")?,
    );
    WorldServer::start_on(
        engine,
        &level.files,
        &level.server_mods,
        OFFICE_MAP,
        listen,
        &[
            ("$Pref::Server::Name", start.name),
            (
                "$Server::Dedicated",
                if start.dedicated { "1" } else { "0" },
            ),
        ],
    )
}

/// A client's server scripts: the realm of its connection and the realm's clock (module docs).
pub struct ServerScripts {
    pub realms: Realms,
    /// The realm's clock, in microseconds.
    clock_us: u64,
}

impl ServerScripts {
    /// Realms that remember the player's answers in `choices` and run the guest `runner` makes (none: this client cannot
    /// run server scripts, and says so to servers that offer them).
    pub fn new(choices: Choices, runner: Option<Box<dyn RunnerFactory>>) -> ServerScripts {
        let mut realms = Realms::new(choices, runner);
        if cfg!(target_arch = "wasm32") {
            realms.settings = torque_client_script_host::Settings::web();
        }
        ServerScripts {
            realms,
            clock_us: 0,
        }
    }

    /// A frame of the realm after `ms` of `client`'s clock, the game's key for the menu passed on first.
    pub fn frame<S>(&mut self, client: &mut GameClient<S>, ms: u32) {
        if client.take_number("$World::CatalogKey").is_some() {
            client.host.gui.client_script_player_asked();
        }
        self.clock_us += u64::from(ms) * 1_000;
        let gui = client.host.gui.clone();
        self.realms.frame(
            client.vm_mut(),
            &gui,
            self.clock_us,
            Duration::from_millis(u64::from(ms)),
        );
    }

    /// The connection ended: the question and the menu taken down at once.
    pub fn disconnect<S>(&mut self, client: &mut GameClient<S>) {
        let gui = client.host.gui.clone();
        self.realms.disconnect(client.vm_mut(), &gui);
    }
}

/// The center of a control's rectangle.
pub fn center(rect: [i32; 4]) -> (i32, i32) {
    (rect[0] + rect[2] / 2, rect[1] + rect[3] / 2)
}

// Sizes the menu's Lua gives its controls (`catalog/client/main.lua`), by which a test tells them apart: a card's name, the status
// line, a line of the recent list, a card's picture.
const WIDE: i32 = 734;

/// The controls of the realm that `client` shows, in tree order.
pub fn shown_of<S>(client: &GameClient<S>) -> Vec<ShownControl> {
    client.host.gui.client_script_controls(client.vm())
}

/// The cards of the catalog `client` shows (name and picture each), in the order of the grid.
pub fn card_controls<S>(client: &GameClient<S>) -> impl Iterator<Item = ShownControl> {
    shown_of(client)
        .into_iter()
        .filter(|c| c.kind == Kind::Button && !c.image.is_empty())
}

/// The names on the cards of the catalog `client` shows, in the order of the grid.
pub fn cards_of<S>(client: &GameClient<S>) -> Vec<String> {
    card_controls(client).map(|c| c.text).collect()
}

/// The status line `client` shows.
pub fn status_of<S>(client: &GameClient<S>) -> String {
    shown_of(client)
        .into_iter()
        .find(|c| c.kind == Kind::Text && c.rect[2] == WIDE && c.rect[3] == 20)
        .map(|c| c.text)
        .unwrap_or_default()
}

/// The lines of the recent list `client` shows, newest first (the empty ones left out).
pub fn recent_of<S>(client: &GameClient<S>) -> Vec<String> {
    shown_of(client)
        .into_iter()
        .filter(|c| c.kind == Kind::Text && c.rect[2] == WIDE && c.rect[3] == 16)
        .map(|c| c.text)
        .filter(|t| !t.is_empty())
        .collect()
}
