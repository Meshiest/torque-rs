//! The office's prop catalog: a mod the server requires of its clients, whose menu runs in a realm on each client.
//!
//! The mod is the folder `catalog/` of this crate (`mod.toml`, `server.cs`, `client/main.lua`) and the picture of each kind of
//! prop (`icons/<kind>.png`). The server holds it under `mods/catalog/` and its clients do not, so they download it, picture
//! files included, before their realm can ask the player. The server's script keeps the catalog (a kind and a display name), checks
//! every request (a known kind, a label of allowed characters and length, a cap of props out a player, a rate limit, and
//! `propCheckPlacement` in front of the player) and spawns the prop with `propSpawn`, owned by the player. The menu is a grid of
//! cards (picture, name, a Spawn button), a field for an optional label, a status line for the server's reply, and the list of
//! recent spawns.
//!
//! [`Catalog`] is the example's session: the office played by a client that hosts its server (the level demo's own client) and a
//! second client that joins it, each with a realm. Both engines are made by [`catalog_engine`], which adds the plugins server
//! scripts need and has the server require its mods. Nothing of the menu is read or run before the player allows it.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use torque::app::client::{ClientBuilder, ClientNet, GameClient, carry_between};
use torque::app::plugins::{DefaultPlugins, GuiPlugin, ModsPlugin, ServerQueryPlugin};
use torque::app::view::WorldView;
use torque::app::{App, Host};
use torque::engine::command::CommandSpec;
use torque::engine::{Engine, EngineBuilder, Plugin, PluginGroup as _, PluginId};
use torque::gui::GuiDraws;
use torque::gui::client_script::{PromptSpot, ShownControl, prompt_spots};
use torque::gui::input::InputEvent;
use torque::gui::keys::{
    KEY_BACKSPACE, KEY_BUTTON0, KEYBOARD_DEVICE, MOUSE_DEVICE, SI_BREAK, SI_BUTTON, SI_KEY, SI_MAKE,
};
use torque::mods::CacheChoice;
use torque_client_script_format::ui::Kind;
use torque_client_script_host::{GuestEngine, InstanceLimits, RealmGuest, Runner, Trap};
use torque_plugin_client_script::{
    Choices, ClientScriptsPlugin, RealmState, Realms, RunnerFactory,
};
use torque_plugin_gui_layout::GuiLayoutPlugin;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::client::LevelDemo;
use crate::files::GameFiles;
use crate::plugin::{Level, LevelPlugin, engine_of};
use crate::server::{SERVER_ADDR, WorldServer};

/// The size of a client's screen (pixels).
pub const SIZE: (u32, u32) = (1280, 720);

/// The screen that shows a map's world, which the game marks as one that shows server controls.
pub const SCREEN: &str = "PlayGui";

/// The abilities the menu asks for, as lines of the consent prompt: `ui` and `net`.
pub const ABILITIES: usize = 2;

/// The second client's address on the link.
const OTHER_ADDR: std::net::SocketAddrV4 =
    std::net::SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 50_001);

/// The folder of the mod's own files.
fn mod_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog")
}

/// The mod's files as the server holds them (a path and its bytes each): the folder `catalog/` under `mods/catalog/`, its pictures
/// (`icons/<kind>.png`, made by `examples/catalog-icons.rs`) included.
pub fn mod_files() -> io::Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    read_folder(&mod_dir(), "mods/catalog", &mut files)?;
    Ok(files)
}

/// The files under `dir` (a path and its bytes each), their paths below `prefix`, in the order of their names.
fn read_folder(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) -> io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        if entry.file_type()?.is_dir() {
            read_folder(&entry.path(), &path, out)?;
        } else {
            out.push((path, std::fs::read(entry.path())?));
        }
    }
    Ok(())
}

/// The control classes a menu is made of.
#[derive(PluginId)]
#[plugin(crate = torque::engine)]
struct MenuControls;

impl Plugin for MenuControls {
    fn build(&self, engine: &mut EngineBuilder) {
        torque::gui::client_script::add_controls(engine);
    }
}

/// The level demo's engine over `files`, with what the catalog needs: the server requires the catalog mod of its clients (which
/// keep what they download in `cache`) and the plugins that carry the menu's messages, make its controls and lay its containers
/// out. The prediction and the props are [`level_engine`](crate::plugin::level_engine)'s.
pub fn catalog_engine(files: GameFiles, cache: PathBuf) -> Result<EngineBuilder, String> {
    let level = LevelPlugin::with_mods(files, mod_files().map_err(|e| e.to_string())?)?;
    let mods = ModsPlugin::new()
        .required_mods()
        .cache(CacheChoice::Folder(cache));
    let mut engine = engine_of(
        level,
        DefaultPlugins
            .set(GuiPlugin { layout_fonts: true })
            .set(mods)
            .disable::<ServerQueryPlugin>(),
    )?;
    engine.add_plugins((ClientScriptsPlugin::new(), MenuControls, GuiLayoutPlugin));
    // The key for the menu (Q) reaches the server as the command `Catalog`: a couple a second is plenty.
    engine.server_command_limits("Catalog", CommandSpec::new().rate(2, 4));
    Ok(engine)
}

/// The realm guest the clients run: the file `TORQUE_CLIENT_GUEST` names, else the one `just lua-client-web` leaves in the build
/// folder. The engine runs only a guest whose hash a release lists.
pub fn guest_path() -> PathBuf {
    if let Some(path) = std::env::var_os("TORQUE_CLIENT_GUEST") {
        return PathBuf::from(path);
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    );
    target.join("lua-web").join("torque_lua_client.wasm")
}

/// The guest at `path`, compiled and checked against the build's list.
pub fn load_guest(path: &Path) -> Result<RealmGuest, String> {
    let bytes = std::fs::read(path).map_err(|e| {
        format!(
            "{}: {e} (build the guest with `just lua-client-web`)",
            path.display()
        )
    })?;
    let engine = GuestEngine::new(Duration::from_millis(5)).map_err(|e| e.to_string())?;
    RealmGuest::new(&engine, &bytes).map_err(|e| e.to_string())
}

/// Starts a guest instance for each realm.
struct Instances(RealmGuest);

impl RunnerFactory for Instances {
    fn open(&self, limits: InstanceLimits) -> Result<Box<dyn Runner>, Trap> {
        Ok(Box::new(self.0.open(limits)?))
    }
}

/// A client's realm: nothing is remembered between runs, and the player is asked about every server (the settings and the prompt
/// are the engine's).
fn realms(guest: &RealmGuest) -> Realms {
    Realms::new(
        Choices::in_memory(),
        Some(Box::new(Instances(guest.clone()))),
    )
}

/// The office played by two clients, [`Catalog::demo`] (the level demo's own client, which draws) and [`Catalog::other`], joined to
/// a server of its own.
pub struct Catalog {
    /// The office's server, once [`Catalog::start_office`] has started it.
    pub server: Option<WorldServer>,
    pub demo: LevelDemo,
    pub other: GameClient<WorldServer>,
    /// The realm of each client: the first is the demo's.
    pub realms: [Realms; 2],
    /// The realms' clock, in microseconds.
    clock_us: u64,
}

/// The server's tick, and each client's step (ms).
pub const TICK_MS: u32 = 32;

/// What a client of [`Catalog`] is called when it connects.
pub const NAMES: [&str; 2] = ["Ann", "Bob"];

impl Catalog {
    /// The two clients, not yet in a map. Each keeps its downloads in a folder under `work`.
    pub fn new(guest: &RealmGuest, work: &Path) -> Result<Catalog, String> {
        let _ = std::fs::remove_dir_all(work);
        let engine = |n: usize| -> Result<Engine, String> {
            let files = GameFiles::bundled().map_err(|e| e.to_string())?;
            catalog_engine(files, work.join(format!("client-{n}")))?
                .build()
                .map_err(|e| e.to_string())
        };
        let mut demo = LevelDemo::from_engine(&engine(0)?)?;
        let second = engine(1)?;
        let level = std::rc::Rc::clone(
            &*second
                .resource::<std::rc::Rc<Level>>()
                .ok_or("the engine has no level demo")?,
        );
        let mut b = ClientBuilder::from_engine(
            &second,
            Some(level.files.mem_files()),
            ClientNet::Memory {
                client: OTHER_ADDR,
                server: SERVER_ADDR,
            },
        )?;
        b.exec("client.cs")?;
        b.invoke("buildMenu")?;
        let mut other = b.build();
        for (n, client) in [&mut demo.client, &mut other].into_iter().enumerate() {
            client.set_global("$Pref::Player::Name", NAMES[n]);
            client.host.surface_size(SIZE);
            client.host.gui.set_client_script_screens(&[SCREEN]);
        }
        Ok(Catalog {
            server: None,
            demo,
            other,
            realms: [realms(guest), realms(guest)],
            clock_us: 0,
        })
    }

    /// Starts the office's server, which requires the catalog mod of its clients, and has both clients join it.
    pub fn start_office(&mut self) -> Result<(), String> {
        let index = self
            .demo
            .maps
            .iter()
            .position(|m| crate::shots::map_name(&m.path) == "office")
            .ok_or("no office map")?;
        let path = self.demo.maps[index].path.clone();
        self.server = Some(WorldServer::start_with(
            self.demo.engine(),
            &self.demo.files,
            self.demo.server_mods(),
            &path,
        )?);
        self.demo.join_map(index)?;
        self.other
            .call("connectTo", &[SERVER_ADDR.to_string().as_str()])
            .map(drop)
    }

    /// The client `n` (0 is the demo's own).
    pub fn client(&mut self, n: usize) -> &mut GameClient<WorldServer> {
        if n == 0 {
            &mut self.demo.client
        } else {
            &mut self.other
        }
    }

    fn carry(&self) {
        if let Some(server) = self.server.as_ref() {
            carry_between(server, &[&self.demo.client, &self.other]);
        }
    }

    /// A step of 32 ms (the server's tick): the server, then both clients, the datagrams carried across around each, and a frame
    /// of each realm.
    pub fn step(&mut self) {
        let ms = TICK_MS;
        self.carry();
        if let Some(server) = self.server.as_mut() {
            server.tick();
        }
        self.carry();
        self.demo.step(ms);
        self.other.step(ms);
        self.carry();
        // The game's own key for the menu (Q, in `game/client.cs`) tells the host the player asked for it, so the menu may take the
        // cursor again after an Escape; the server's script answers the same key with its message to the menu.
        for n in 0..2 {
            if self.client(n).take_number("$World::CatalogKey").is_some() {
                self.client(n).host.gui.client_script_player_asked();
            }
        }
        self.clock_us += u64::from(ms) * 1_000;
        let (clock, frame) = (self.clock_us, Duration::from_millis(u64::from(ms)));
        let gui = self.demo.client.host.gui.clone();
        self.realms[0].frame(self.demo.client.vm_mut(), &gui, clock, frame);
        let gui = self.other.host.gui.clone();
        self.realms[1].frame(self.other.vm_mut(), &gui, clock, frame);
    }

    /// Steps of 32 ms until `done` holds, at most `limit`; whether it did.
    pub fn run_until(&mut self, limit: u32, mut done: impl FnMut(&mut Catalog) -> bool) -> bool {
        for _ in 0..limit {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    /// `steps` steps of 32 ms.
    pub fn run(&mut self, steps: u32) {
        for _ in 0..steps {
            self.step();
        }
    }

    /// Runs script `code` on the server and returns its result.
    pub fn server_eval(&mut self, code: &str) -> Result<String, String> {
        let server = self.server.as_mut().ok_or("no server")?;
        server
            .vm_mut()
            .eval(code.as_bytes())
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .map_err(|e| format!("{code}: {e}"))
    }

    /// A global of the server's script engine as text (empty when unset).
    pub fn server_global(&self, name: &str) -> String {
        self.server
            .as_ref()
            .and_then(|s| s.vm().global(name))
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_default()
    }

    /// Whether client `n` is in the world with a figure of its own on the server.
    pub fn in_world(&self, n: usize) -> bool {
        let flag = |name: &str| {
            if n == 0 {
                self.demo.client.flag(name)
            } else {
                self.other.flag(name)
            }
        };
        flag("$World::InWorld")
    }

    /// The controls of the realm that client `n` shows, in tree order.
    pub fn shown(&self, n: usize) -> Vec<ShownControl> {
        let client = if n == 0 {
            &self.demo.client
        } else {
            &self.other
        };
        client.host.gui.client_script_controls(client.vm())
    }
}

/// The center of a control's rectangle.
pub fn center(rect: [i32; 4]) -> (i32, i32) {
    (rect[0] + rect[2] / 2, rect[1] + rect[3] / 2)
}

/// The player presses and releases the left button at `at`: whether the GUI took both.
pub fn click(client: &mut GameClient<WorldServer>, at: (i32, i32)) -> bool {
    let gui = client.host.gui.clone();
    gui.mouse_moved(client.vm_mut(), at, 0);
    let mut button = |action| {
        let event = InputEvent {
            device: MOUSE_DEVICE,
            obj_type: SI_BUTTON,
            obj_inst: KEY_BUTTON0,
            action,
            ..InputEvent::default()
        };
        gui.input_event(client.vm_mut(), &event).0
    };
    let down = button(SI_MAKE);
    let up = button(SI_BREAK);
    down && up
}

fn key(client: &mut GameClient<WorldServer>, code: u16, ascii: u16) {
    let gui = client.host.gui.clone();
    for action in [SI_MAKE, SI_BREAK] {
        let event = InputEvent {
            device: KEYBOARD_DEVICE,
            obj_type: SI_KEY,
            obj_inst: code,
            action,
            ascii,
            ..InputEvent::default()
        };
        gui.input_event(client.vm_mut(), &event);
    }
}

/// The player types `text`, a key at a time, into whatever has the focus.
pub fn type_text(client: &mut GameClient<WorldServer>, text: &str) -> Result<(), String> {
    // A key event carries one UTF-16 unit; a character past U+FFFF would need two and the field takes neither alone, so it is
    // refused here rather than typed as something else.
    if let Some(c) = text.chars().find(|c| u16::try_from(u32::from(*c)).is_err()) {
        return Err(format!(
            "{c:?} is past U+FFFF, which a key event cannot carry"
        ));
    }
    for c in text.chars() {
        let code = u16::try_from(u32::from(c)).map_err(|e| e.to_string())?;
        key(client, code, code);
    }
    Ok(())
}

/// The player presses `spot` of the consent prompt (with `rows` abilities in it).
pub fn press_prompt(client: &mut GameClient<WorldServer>, rows: usize, spot: PromptSpot) -> bool {
    let size = client.host.gui.0.borrow().canvas_size;
    let Some((_, rect)) = prompt_spots(size, rows)
        .into_iter()
        .find(|(s, _)| *s == spot)
    else {
        return false;
    };
    click(client, center(rect))
}

// Sizes the menu's Lua gives its controls (`catalog/client/main.lua`), by which a test tells them apart: a card's name, the status
// line, a line of the recent list, a card's picture.
const WIDE: i32 = 734;

impl Catalog {
    /// Whether client `n` shows anything of a realm at all.
    pub fn has_menu(&self, n: usize) -> bool {
        !self.shown(n).is_empty()
    }

    /// The names on the cards of the catalog as client `n` shows them, in the order of the grid.
    pub fn cards(&self, n: usize) -> Vec<String> {
        self.shown(n)
            .into_iter()
            .filter(|c| c.kind == Kind::Button && !c.image.is_empty())
            .map(|c| c.text)
            .collect()
    }

    /// The pictures on the cards (content paths), in the order of the grid.
    pub fn pictures(&self, n: usize) -> Vec<String> {
        self.shown(n)
            .into_iter()
            .filter(|c| c.kind == Kind::Button && !c.image.is_empty())
            .map(|c| c.image)
            .collect()
    }

    /// The status line, as client `n` shows it.
    pub fn status(&self, n: usize) -> String {
        self.shown(n)
            .into_iter()
            .find(|c| c.kind == Kind::Text && c.rect[2] == WIDE && c.rect[3] == 20)
            .map(|c| c.text)
            .unwrap_or_default()
    }

    /// The lines of the recent list as client `n` shows them, newest first (the empty ones left out).
    pub fn recent(&self, n: usize) -> Vec<String> {
        self.shown(n)
            .into_iter()
            .filter(|c| c.kind == Kind::Text && c.rect[2] == WIDE && c.rect[3] == 16)
            .map(|c| c.text)
            .filter(|t| !t.is_empty())
            .collect()
    }

    fn find(
        &self,
        n: usize,
        kind: Kind,
        wanted: impl Fn(&ShownControl) -> bool,
    ) -> Result<ShownControl, String> {
        self.shown(n)
            .into_iter()
            .find(|c| c.kind == kind && wanted(c))
            .ok_or_else(|| format!("client {n} shows no such {kind:?}"))
    }

    /// The player at client `n` answers the consent prompt with Allow: they wait out the prompt's half second of ignoring the
    /// pointer (it ignores a click made too soon) and press the button, and the menu starts.
    pub fn allow(&mut self, n: usize) -> Result<(), String> {
        if !self.run_until(2_000, |s| s.realms[n].state() == RealmState::Asking) {
            return Err(format!(
                "client {n} was not asked: {:?}",
                self.realms[n].state()
            ));
        }
        self.run(24);
        if !press_prompt(self.client(n), ABILITIES, PromptSpot::Allow) {
            return Err(format!("client {n}'s prompt took no press"));
        }
        if self.run_until(2_000, |s| s.realms[n].state() == RealmState::Running) {
            Ok(())
        } else {
            Err(format!(
                "client {n}'s menu did not start: {:?}",
                self.realms[n].state()
            ))
        }
    }

    /// The player at client `n` types `label` into the form (what was there is cleared first) and clicks the card named `card`
    /// ("Chair"), which is a button as a whole.
    pub fn spawn(&mut self, n: usize, card: &str, label: &str) -> Result<(), String> {
        self.type_label(n, label)?;
        let button = self
            .shown(n)
            .into_iter()
            .find(|c| c.kind == Kind::Button && !c.image.is_empty() && c.text == card)
            .ok_or_else(|| format!("client {n} shows no card {card:?}"))?;
        click(self.client(n), center(button.rect));
        Ok(())
    }

    /// The player at client `n` types `label` into the label field, after clearing it.
    pub fn type_label(&mut self, n: usize, label: &str) -> Result<(), String> {
        let field = self.find(n, Kind::TextField, |_| true)?;
        let client = self.client(n);
        click(client, center(field.rect));
        for _ in 0..field.text.chars().count() {
            key(client, KEY_BACKSPACE, 0);
        }
        type_text(client, label)
    }

    /// The player at client `n` presses and releases `key` (a key of the game's: the action maps and the GUI see it as they do a
    /// real one).
    pub fn press_key(&mut self, n: usize, key: torque::client::input::Key) {
        let client = self.client(n);
        client.key(key, true);
        client.key(key, false);
    }

    /// The player at client `n` presses the button whose text is `text`.
    pub fn press(&mut self, n: usize, text: &str) -> Result<(), String> {
        let button = self.find(n, Kind::Button, |c| c.text == text)?;
        click(self.client(n), center(button.rect));
        Ok(())
    }
}

/// The catalog as an [`App`]: client 0's view, the session stepped with it.
pub struct CatalogView {
    pub catalog: Catalog,
    /// The time the session has not stepped yet (ms).
    owed: u32,
}

impl CatalogView {
    pub fn new(catalog: Catalog) -> CatalogView {
        CatalogView { catalog, owed: 0 }
    }
}

impl App for CatalogView {
    fn host(&mut self) -> &mut Host {
        &mut self.catalog.demo.client.host
    }

    fn host_ref(&self) -> &Host {
        &self.catalog.demo.client.host
    }

    fn title(&self) -> String {
        "torque-rs level demo: prop catalog".to_string()
    }

    fn log_name(&self) -> &str {
        "example-level-demo"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        self.catalog.demo.view(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.owed += ms;
        while self.owed >= TICK_MS {
            self.owed -= TICK_MS;
            self.catalog.step();
        }
        self.catalog.demo.add_frame_time(ms as f32 / 1000.0);
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        self.catalog.demo.frame(view)
    }
}

/// Pictures of the catalog from the first client, offscreen, into `dir`: the consent prompt, the catalog with its pictures, the
/// label typed and the recent list after it, a chair put down in the office (the catalog hidden), and a refusal under the form.
/// Returns the paths.
pub fn write_shots(guest: &RealmGuest, work: &Path, dir: &Path) -> Result<Vec<PathBuf>, String> {
    use torque::app::shots::{Seat, Shots};
    use torque::app::window::offscreen_gpu;

    let mut catalog = Catalog::new(guest, work)?;
    catalog.start_office()?;
    let asked = |c: &mut Catalog| {
        (0..2).all(|n| c.in_world(n) && c.realms[n].state() == RealmState::Asking)
    };
    if !catalog.run_until(6_000, asked) {
        return Err("the clients were not asked".into());
    }
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let mut seat = Seat::new(CatalogView::new(catalog), &gpu, SIZE)?;
    let mut shots = Shots::new(dir)?;
    // The player is asked before anything of the menu exists.
    seat.run(&gpu, 800)?;
    shots.save(&seat.picture(&gpu)?, "consent-prompt")?;
    for n in 0..2 {
        seat.game.catalog.allow(n)?;
    }
    let office = |x: f32, y: f32| [x - 18.0, y - 12.0];
    let stand = |seat: &mut Seat<CatalogView>, who: &str, x: f32, y: f32| -> Result<(), String> {
        let [x, y] = office(x, y);
        seat.game.catalog.server_eval(&format!(
            "for (%i = 0; %i < ClientGroup.getCount(); %i++) {{ %c = ClientGroup.getObject(%i); \
             if (%c.name $= \"{who}\") %c.player.setTransform(\"{x} {y} 0.2 0 0 1 1.5708\"); }}"
        ))?;
        seat.game.catalog.run(12);
        Ok(())
    };
    stand(&mut seat, "Ann", 13.0, 14.0)?;
    stand(&mut seat, "Bob", 22.0, 14.0)?;
    seat.run(&gpu, 1_500)?;
    shots.save(&seat.picture(&gpu)?, "catalog")?;
    // Bob puts a labeled chair down; Ann types a label, and then sends it.
    seat.game.catalog.spawn(1, "Chair", "Bob's chair")?;
    seat.run(&gpu, 800)?;
    seat.game.catalog.type_label(0, "Ann's chair")?;
    seat.run(&gpu, 400)?;
    shots.save(&seat.picture(&gpu)?, "catalog-label")?;
    seat.game.catalog.spawn(0, "Chair", "Ann's chair")?;
    seat.run(&gpu, 1_200)?;
    shots.save(&seat.picture(&gpu)?, "catalog-recent")?;
    // The catalog out of the way: the chair in front of the figure, seen from along the corridor.
    seat.game.catalog.press(0, "Hide catalog")?;
    seat.run(&gpu, 1_500)?;
    let at = |x: f32, y: f32, z: f32| {
        let [x, y] = office(x, y);
        [x, y, z]
    };
    seat.eye = Some(crate::client::with_fov(
        torque_render_scene::hd::kit::look_at(at(17.0, 13.3, 1.9), at(14.0, 14.0, 0.7)),
    ));
    let picture = seat.picture(&gpu);
    seat.eye = None;
    shots.save(&picture?, "chair-in-the-office")?;
    // A label the server turns down.
    seat.game.catalog.press(0, "Show catalog")?;
    seat.run(&gpu, 4_000)?;
    seat.game
        .catalog
        .spawn(0, "Plant", "This label is much too long for it")?;
    seat.run(&gpu, 1_200)?;
    shots.save(&seat.picture(&gpu)?, "refusal")?;
    Ok(shots.paths)
}
