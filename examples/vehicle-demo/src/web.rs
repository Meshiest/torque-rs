//! The vehicle demo in a browser: its files fetched by the page (`files.json`, which `vehicle-demo --web-files` writes beside
//! the pack) and the engine's window as a canvas on the page. The page's calls and its settings are the engine's web entry
//! (`torque::app::web_page`). Natively only `alone` is of use (the tests).
//!
//! **Played alone** (the page as it is): the server on an in-memory link in the page (no socket:
//! `ClientOptions::alone_only`); the game starts at once, the box figure beside its buggy on the hills.
//!
//! **Joined** (the engine's `torque::app::web_join`): `?join` joins the server that served the page
//! (`vehicle-demo-server --transport both`, its game socket over WebSocket with a WebRTC data channel beside it when the
//! server offers one, `--webrtc`), `?server=<ws or wss URL>` one elsewhere, and `?room=<code>` a game a player hosts,
//! found through the room service (`?rooms=<ws or wss URL>`, else the page's own host). `?name=<name>` sets the player's
//! name. When the link ends for good (the host left, the server closed the connection) the world is left with why on the
//! menu.
//!
//! **Hosted by the page** (`?host=1`, `web/host.js`): the server runs in the page's Web Worker on the engine's relay
//! transport (`host_start`, `torque::app::web_host`), others join by the room's code, and the host player's client
//! reaches it over the local link (`start_host_client`).
//!
//! **The menu** (after leaving the world): a page built to be played alone (`web/build.mjs --solo`, a site with no game
//! server) offers to play alone and to host or join with an invite link (`$World::WebInvites`): those ask the page (its
//! `torqueAsk`, `web/invites.js`, through `torque::app::web_join::ask_page`), which trades connection codes by hand and
//! opens the page again to host (`?invites=1`: the server in the page's worker, as `?host=1` with no room). Any other page
//! offers to host a game here (the page opens again with `?host=1`) and to join a room by its code (`?room=<code>`), and a
//! host page shows its room's code on the menu and the world's title.
//!
//! **Joined by an invite link** (`web/invites.js`): the page holds the data channel to the host and calls
//! `start_link_client`; the client is on `torque::app::web_join::Target::Page`.
//!
//! The page's root element carries the game's state for a test to wait on (`web_page::mark_play`): the frames drawn, the map
//! (empty off it), whether the client holds its figure and where it sees it; and `data-torque-others` (the other players'
//! figures the client holds), `data-torque-seat` (`-1` on foot, `0` at the wheel, `1` the passenger's seat),
//! `data-torque-buggy` (where it draws the buggy it is in or the one nearest it, `x y z`), `data-torque-buggies` (each
//! buggy it holds, `x y z` separated by `;`), `data-torque-status` (the menu's status line), and for a joined game the link's
//! state (`torque::app::web_join::mark_link`).

use torque::app::view::WorldView;
use torque::app::web_page;
use torque::app::{App, Host};
use torque::gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::client::{ClientOptions, VehicleDemo};
use crate::files::GameFiles;

/// The module carries no files: the page fetches them.
pub const EMBEDDED: bool = false;

/// This build carries no files: the page fetches them by its list.
pub fn start_embedded() -> Result<(), String> {
    Err("this build carries no files: the page fetches them by its list".into())
}

/// Starts the game on the fetched files (Err: one the list names is missing, or a pack is broken): played alone, or joined
/// when the page's address asks (module docs).
pub fn start_fetched() -> Result<(), String> {
    let files = GameFiles::from_fetched(web_page::take_fetched()?)?;
    #[cfg(target_arch = "wasm32")]
    if let Some(target) = torque::app::web_join::Target::from_page()? {
        return joined(files, target);
    }
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut))]
    let mut game = alone(files)?;
    #[cfg(target_arch = "wasm32")]
    offer_rooms(&mut game);
    torque::app::window::run(WebVehicle {
        game,
        frames: 0,
        link: None,
        room: None,
    })
}

/// The client over `files` with no socket of its own, playing alone.
pub fn alone(files: GameFiles) -> Result<VehicleDemo, String> {
    let mut game = VehicleDemo::new(
        files,
        ClientOptions {
            alone_only: true,
            ..ClientOptions::default()
        },
    )?;
    game.play_alone()?;
    Ok(game)
}

/// Starts the host player's game on the fetched files: its server runs in the page's worker ([`host_start`]), and the
/// client reaches it over the local link.
#[cfg(target_arch = "wasm32")]
pub fn start_host_client() -> Result<(), String> {
    let files = GameFiles::from_fetched(web_page::take_fetched()?)?;
    joined(files, torque::app::web_join::Target::Local)
}

/// Starts a client joined by an invite link on the fetched files: the page holds its data channel to the host.
#[cfg(target_arch = "wasm32")]
pub fn start_link_client() -> Result<(), String> {
    let files = GameFiles::from_fetched(web_page::take_fetched()?)?;
    joined(files, torque::app::web_join::Target::Page)
}

/// Starts the worker's server on the fetched files and `map` (empty or the demo's one), and returns the map's path.
#[cfg(target_arch = "wasm32")]
pub fn host_start(map: &str) -> Result<String, String> {
    use crate::files::MISSION;
    if !map.is_empty() && map != MISSION {
        return Err(format!("no map {map:?}: the demo has {MISSION}"));
    }
    let files = GameFiles::from_fetched(web_page::take_fetched()?)?;
    torque::app::web_host::start(|relay| {
        crate::server::WorldServer::start_bound(&files, relay, "Hills").map(Into::into)
    })?;
    Ok(MISSION.to_string())
}

/// The client over `files` joined to `target`'s game.
#[cfg(target_arch = "wasm32")]
fn joined(files: GameFiles, target: torque::app::web_join::Target) -> Result<(), String> {
    use torque::app::web_join::{SERVER, query};
    let name = query("name")
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Browser player".to_string());
    let mut game = VehicleDemo::new(
        files,
        ClientOptions {
            alone_only: true,
            name,
            ..ClientOptions::default()
        },
    )?;
    offer_rooms(&mut game);
    let (socket, watch) = target.open()?;
    game.set_socket(socket);
    game.join(&SERVER.to_string())?;
    torque::app::window::run(WebVehicle {
        game,
        frames: 0,
        link: Some(Link {
            watch,
            ended: false,
        }),
        room: None,
    })
}

/// A page that is not one played alone (`data-torque-solo`) offers the page's own hosting and joining by room code on the
/// menu (`$World::WebRooms`); a solo page offers to host and join with an invite link (`$World::WebInvites`), and its
/// Esc menu to invite players.
#[cfg(target_arch = "wasm32")]
fn offer_rooms(game: &mut VehicleDemo) {
    if torque::app::web_join::solo_page() {
        game.set_global("$World::WebInvites", "1");
        // the Esc menu's Invite players
        game.set_global("$Pause::CanInvite", "1");
    } else {
        game.set_global("$World::WebRooms", "1");
    }
}

/// A joined game's link: what it shows of itself, and whether its end was shown.
#[cfg(target_arch = "wasm32")]
struct Link {
    watch: torque::client::net::web_rtc::LinkWatch,
    ended: bool,
}

/// Natively nothing joins.
#[cfg(not(target_arch = "wasm32"))]
type Link = ();

/// The vehicle demo, its state written on the page as it draws (module docs).
struct WebVehicle {
    game: VehicleDemo,
    frames: u64,
    /// The link of a joined game (none played alone).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    link: Option<Link>,
    /// The room code a host page shows, once it came.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    room: Option<String>,
}

#[cfg(target_arch = "wasm32")]
impl WebVehicle {
    /// A link that ended for good: out of the world, with why on the menu.
    fn show_end(&mut self) {
        let Some(link) = self.link.as_mut().filter(|l| !l.ended) else {
            return;
        };
        let Some(text) = torque::app::web_join::ended_text(&link.watch) else {
            return;
        };
        link.ended = true;
        if self.game.global("$World::InWorld") == "1" {
            let _ = self.game.call("leaveWorld", &[]);
        }
        self.menu_status(&text);
    }

    /// Shows `text` on the menu's status line, handed to `setText` as an argument (never built into script source).
    fn menu_status(&mut self, text: &str) {
        let vm = self.game.vm_mut();
        if let Some(status) = vm.objects().find(b"MenuStatus") {
            let _ = vm.call_method(status, b"setText", &[text.as_bytes().to_vec()]);
        }
    }

    /// What the menu asked of the page: to host a game, or to join a room by its code (the page opens again for it), or
    /// its invite panels (a solo page).
    fn menu_asks(&mut self) {
        use torque::app::web_join::open_page;
        let ask = self.game.global("$World::PageAsk");
        if !ask.is_empty() {
            self.game.set_global("$World::PageAsk", "");
            torque::app::web_join::ask_page(&ask);
        }
        let asked = if self.game.take_flag("$World::WebHostAsked") {
            Some(open_page(true, ""))
        } else {
            let code = self.game.global("$World::WebJoinCode");
            (!code.is_empty()).then(|| {
                self.game.set_global("$World::WebJoinCode", "");
                open_page(false, &code)
            })
        };
        if let Some(Err(e)) = asked {
            self.menu_status(&e);
        }
    }

    /// A host page's room code, once it shows: on the menu and the world's title.
    fn room_code(&mut self) {
        if self.room.is_some() {
            return;
        }
        let Some(code) = torque::app::web_join::page_room() else {
            return;
        };
        self.game.set_global("$World::RoomCode", &code);
        let _ = self.game.call("showRoomCode", &[]);
        self.room = Some(code);
    }

    fn mark(&mut self) {
        use torque::app::web_page::mark;
        self.room_code();
        let in_world = self.game.in_world();
        let map = if in_world { crate::files::MISSION } else { "" };
        web_page::mark_play(self.frames, map, self.game.figure().map(|f| f.position));
        let players = self
            .game
            .with_world(|w| {
                w.records
                    .ghosts
                    .values()
                    .filter(|g| g.class == Some(torque::ghosts::TgeClass::Player))
                    .count()
            })
            .unwrap_or(0);
        let others = if in_world {
            players.saturating_sub(1)
        } else {
            0
        };
        mark("others", &others.to_string());
        let seat = self.game.seat().map_or(-1, i64::from);
        mark("seat", &seat.to_string());
        let at = |p: [f32; 3]| format!("{:.2} {:.2} {:.2}", p[0], p[1], p[2]);
        let buggy = self.game.buggy_pose().map(|m| at(m.column(3)));
        mark("buggy", &buggy.unwrap_or_default());
        let buggies: Vec<String> = self.game.buggy_positions().into_iter().map(at).collect();
        mark("buggies", &buggies.join(";"));
        let status = self
            .game
            .call("eval", &["return MenuStatus.getValue();"])
            .unwrap_or_default();
        mark("status", &status);
        if let Some(link) = &self.link {
            // A window of about two seconds (eight marks).
            torque::app::web_join::mark_link(&link.watch, self.frames % 120 == 1);
        }
    }
}

impl App for WebVehicle {
    fn host(&mut self) -> &mut Host {
        App::host(&mut self.game)
    }

    fn host_ref(&self) -> &Host {
        App::host_ref(&self.game)
    }

    fn title(&self) -> String {
        App::title(&self.game)
    }

    fn log_name(&self) -> &str {
        App::log_name(&self.game)
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        App::view(&self.game, gpu)
    }

    fn advance(&mut self, ms: u32) {
        App::advance(&mut self.game, ms);
        #[cfg(target_arch = "wasm32")]
        {
            self.show_end();
            self.menu_asks();
        }
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let drawn = App::frame(&mut self.game, view);
        self.frames += 1;
        #[cfg(target_arch = "wasm32")]
        if self.frames % 15 == 1 {
            self.mark();
        }
        drawn
    }

    /// The map is loading until the client is in the world and holds its figure (played alone; a joined game's menu shows
    /// how its join goes).
    fn loading(&self) -> bool {
        self.game.server.is_some() && (!self.game.in_world() || self.game.figure().is_none())
    }
}
