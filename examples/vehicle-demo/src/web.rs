//! The vehicle demo in a browser: its files fetched by the page (`files.json`, which `vehicle-demo --web-files` writes beside
//! the pack) and the engine's window as a canvas on the page. The page's calls and its settings are the engine's web entry
//! (`torque::app::web_page`). Natively only `alone` is of use (the tests).
//!
//! **Played alone** (the page as it is): the server on an in-memory link in the page (no socket:
//! `ClientOptions::alone_only`); the game starts at once, the box figure beside its buggy on the hills (its first steps make
//! the map's interiors a few at a time, then ask to play alone: no one task of the page's thread holds the start and the map
//! together). A page built to be played alone (`web/build.mjs --solo`) waits at the menu instead ([`menu`]): Play alone
//! starts it there.
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
//! switches the running game in place (`torque::app::web_join::switch_to`, taken at the next step): to host (`host`: the
//! page starts the server in its worker, as `?host=1` with no room, and the client joins it on the local link), to an
//! invite link's game (`link`) and back to playing alone (`solo`). Any other page offers to host a game here and to join a
//! room by its code, both in place with no new page: Host a game asks the page (`torqueAsk('host')`, `web/host.js`
//! `roomPage`), which starts the server in its worker with a room and switches the game to it (`host`); Join room opens
//! the room's link here and joins. A host page shows its room's code on the menu and the world's title. Leaving that
//! world, or its link ending (the host left), takes the page back to its own game at its menu: a game it hosted ends
//! (`torqueAsk('back')`: the worker, the room and every joiner), and joining a room ends one first. The address bar says
//! what the page plays (`?host=1`, `?room=<code>` or neither: `torque::app::web_join::set_address`), so the page opened
//! on it again plays the same.
//!
//! **Joined by an invite link** (`web/invites.js`): the page holds the data channel to the host. A page opened on an
//! invite starts at its menu as one played alone, with the invite's panel over it; once the channel opens the page
//! switches the game to it (`link`): the client is on `torque::app::web_join::Target::Page` and joins at once (the trade
//! of links by hand takes longer than a join waits for its answer). Leaving that world (the Esc menu's Leave, or the host
//! left) asks the page to take the game back to playing alone (`back`). (`start_link_client` starts a client on the link
//! at once, for a page that has no game of its own.)
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
    // A page played alone at once starts at the menu too: its first steps make the map's interiors a few at a time
    // ([`WebVehicle::warm_interiors`]), then ask to play alone, so neither this call nor any one step holds the page's thread
    // for the server's start and its map's interiors together.
    #[cfg(target_arch = "wasm32")]
    let mut game = menu(files)?;
    #[cfg(not(target_arch = "wasm32"))]
    let game = alone(files)?;
    #[cfg(target_arch = "wasm32")]
    {
        offer_rooms(&mut game);
        game.call("showMenu", &[])?;
    }
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut))]
    let mut web = WebVehicle::new(game, None);
    #[cfg(target_arch = "wasm32")]
    if !torque::app::web_join::solo_page() {
        web.warm = web
            .game
            .files
            .kit
            .paths()
            .filter(|p| p.ends_with(".dif"))
            .map(String::from)
            .collect();
        web.play_when_warm = true;
    }
    torque::app::window::run(web)
}

/// The client over `files` with no socket of its own, at its menu (a page played alone: Play alone starts the game).
pub fn menu(files: GameFiles) -> Result<VehicleDemo, String> {
    VehicleDemo::new(
        files,
        ClientOptions {
            alone_only: true,
            ..ClientOptions::default()
        },
    )
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
    game.call("showMenu", &[])?;
    let (socket, watch) = target.open()?;
    game.set_socket(socket);
    // A link's join waits for its channel (module docs).
    let waiting = target == torque::app::web_join::Target::Page;
    if waiting {
        let vm = game.vm_mut();
        if let Some(status) = vm.objects().find(b"MenuStatus") {
            let text = b"Waiting for the connection to the host...".to_vec();
            let _ = vm.call_method(status, b"setText", &[text]);
        }
    } else {
        game.join(&SERVER.to_string())?;
    }
    torque::app::window::run(WebVehicle::new(game, Some(Link::new(target, watch))))
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

/// A joined game's link: where it goes, what it shows of itself, and whether its end was shown.
#[cfg(target_arch = "wasm32")]
struct Link {
    target: torque::app::web_join::Target,
    watch: torque::client::net::web_rtc::LinkWatch,
    ended: bool,
    /// Whether the client waits for the link's channel to open before it joins (an invite link's).
    waiting: bool,
    /// Whether the page was asked to take the game back to playing alone, the invite link's world left.
    asked_back: bool,
}

#[cfg(target_arch = "wasm32")]
impl Link {
    /// A new link to `target`: an invite link's waits for its channel before the join.
    fn new(
        target: torque::app::web_join::Target,
        watch: torque::client::net::web_rtc::LinkWatch,
    ) -> Link {
        let waiting = target == torque::app::web_join::Target::Page;
        Link {
            target,
            watch,
            ended: false,
            waiting,
            asked_back: false,
        }
    }

    /// Whether the link is an invite link's (`Target::Page`).
    fn page(&self) -> bool {
        self.target == torque::app::web_join::Target::Page
    }

    /// Whether the link goes to a game the page left its own for when it hosted or joined a room in place (the server
    /// in its worker, or a room's host): leaving that world takes the page back to its own game.
    fn away(&self) -> bool {
        use torque::app::web_join::Target;
        matches!(self.target, Target::Local | Target::Room { .. })
    }

    /// Whether the link goes to the server in the page's worker.
    fn hosted(&self) -> bool {
        self.target == torque::app::web_join::Target::Local
    }
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
    /// Whether the client was in a joined game's world at the last step.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    in_link_world: bool,
    /// Whether the page hosts and joins through a room service (a page not played alone): it switches in place, its
    /// address saying what it plays.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    rooms: bool,
    /// The kit's interiors still to be made before the game played at once starts ([`WebVehicle::warm_interiors`]).
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    warm: Vec<String>,
    /// Whether Play alone is asked once they are made.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    play_when_warm: bool,
}

/// The kit's interiors made in one step while a game played at once waits for them (each a few milliseconds).
#[cfg(target_arch = "wasm32")]
const WARM_PER_STEP: usize = 8;

impl WebVehicle {
    fn new(game: VehicleDemo, link: Option<Link>) -> WebVehicle {
        #[cfg(target_arch = "wasm32")]
        let rooms = !torque::app::web_join::solo_page();
        #[cfg(not(target_arch = "wasm32"))]
        let rooms = false;
        WebVehicle {
            game,
            frames: 0,
            link,
            room: None,
            in_link_world: false,
            rooms,
            warm: Vec::new(),
            play_when_warm: false,
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl WebVehicle {
    /// Makes [`WARM_PER_STEP`] of the kit's interiors (their hulls kept for the process: the server's map then finds them made,
    /// [`torque::game::scene::interior::made`]), and once all are made asks to play alone. The server charges each one's cost as
    /// before; only when its hulls are made changes.
    fn warm_interiors(&mut self) {
        if self.warm.is_empty() {
            return;
        }
        let at = self.warm.len().saturating_sub(WARM_PER_STEP);
        for path in self.warm.split_off(at) {
            if let Some(bytes) = torque_render_scene::Assets::read(&self.game.files.kit, &path) {
                let _ = torque::game::scene::interior::read_interior(&bytes, &mut |_| true);
            }
        }
        if self.warm.is_empty() && std::mem::take(&mut self.play_when_warm) {
            self.game.set_global("$World::AloneAsked", "1");
        }
    }

    /// An invite link's channel open: its join.
    fn join_when_open(&mut self) {
        use torque::client::net::web_rtc::Link as Way;
        let Some(link) = self.link.as_mut().filter(|l| l.waiting) else {
            return;
        };
        if link.watch.borrow().link != Way::Channel {
            return;
        }
        link.waiting = false;
        if let Err(e) = self.game.join(&torque::app::web_join::SERVER.to_string()) {
            self.menu_status(&format!("Could not join: {e}"));
        }
    }

    /// Switches the running game in place (module docs): out of the world, onto the link `mode` goes by.
    fn switch(&mut self, mode: torque::app::web_join::Mode) -> Result<(), String> {
        use torque::app::web_join::{Address, Mode, SERVER, Target};
        // Hosting already, the client on the worker's game: nothing to do (the Esc menu's Invite players).
        let on_host = self.link.as_ref().is_some_and(Link::hosted);
        if mode == Mode::Host
            && on_host
            && !self.game.local()
            && self.game.vm().objects().find(b"ServerConnection").is_some()
        {
            return Ok(());
        }
        let target = match mode {
            Mode::Solo => None,
            Mode::Link => Some(Target::Page),
            Mode::Host => Some(Target::Local),
        };
        // Err (the invite link ended before the switch was taken): nothing changed
        let opened = target.map(|t| t.open().map(|o| (t, o))).transpose()?;
        self.leave_game()?;
        if !self.rooms {
            self.game.set_global(
                "$Pause::CanInvite",
                if mode == Mode::Link { "0" } else { "1" },
            );
        }
        let Some((target, (socket, watch))) = opened else {
            self.own_game();
            return Ok(());
        };
        self.game.set_socket(socket);
        let page = target == Target::Page;
        self.link = Some(Link::new(target, watch));
        if self.rooms {
            self.address(if mode == Mode::Host {
                Address::Host
            } else {
                Address::Own
            });
        }
        if page {
            self.menu_status("Waiting for the connection to the host...");
        } else {
            self.menu_status("Joining your game...");
            self.game.join(&SERVER.to_string())?;
        }
        Ok(())
    }

    /// Out of the world (or the join under way), the page's own server stopped and the menu's asks cleared: a switch's
    /// first step, before the client's socket changes.
    fn leave_game(&mut self) -> Result<(), String> {
        if self.game.in_world() {
            self.game.call("leaveWorld", &[])?;
        } else {
            self.game.call("disconnect", &[])?;
        }
        self.game.stop_hosting();
        self.game.set_global("$World::PageAsk", "");
        self.game.set_global("$World::AloneAsked", "0");
        self.game.set_global("$World::LeaveAsked", "0");
        self.in_link_world = false;
        // a room code a hosted game showed is not the next game's
        self.room = None;
        self.game.set_global("$World::RoomCode", "");
        Ok(())
    }

    /// The page's own game again, with no link of its own: at its menu, played alone when asked.
    fn own_game(&mut self) {
        self.game.clear_socket();
        self.link = None;
        self.room = None;
        self.game.set_global("$World::RoomCode", "");
        torque::app::web_join::unmark_link();
        if self.rooms {
            self.address(torque::app::web_join::Address::Own);
        }
    }

    /// The page's address says what it plays (a page with a room service; module docs). One not written leaves the
    /// address as it was: the game goes on.
    fn address(&self, address: torque::app::web_join::Address) {
        let _ = torque::app::web_join::set_address(address);
    }

    /// The menu's Join room: the game of room `code`, joined in place. Any game this page hosts ends first (its players
    /// with it). Err (not a room code, no room service): nothing changed.
    fn join_room(&mut self, code: &str) -> Result<(), String> {
        use torque::app::web_join::{Address, SERVER, Target, rooms_url};
        let code = torque::app::web_join::room_code(code)?;
        let target = Target::Room {
            url: rooms_url()?,
            code: code.clone(),
        };
        let (socket, watch) = target.open()?;
        self.leave_game()?;
        // the game this page hosts, or one it is starting, ends (the client left it first): its server, its room and its
        // players
        torque::app::web_join::ask_page("back");
        self.game.set_socket(socket);
        self.link = Some(Link::new(target, watch));
        self.address(Address::Room(&code));
        self.menu_status(&format!("Joining room {code}..."));
        self.game.join(&SERVER.to_string())
    }

    /// The menu's Host a game: the page starts its server with a room, then switches the game to it (`host`). Hosting
    /// already, at the menu: the client joins that game again.
    fn host_asked(&mut self) {
        if self.link.as_ref().is_some_and(Link::hosted) {
            if !self.game.in_world()
                && let Err(e) = self.game.join(&torque::app::web_join::SERVER.to_string())
            {
                self.menu_status(&format!("Could not join: {e}"));
            }
            return;
        }
        if !torque::app::web_join::ask_page("host") {
            self.menu_status("This page cannot host a game.");
        }
    }

    /// The menu's Play alone while a game the page left its own for is still joining (hosted, or a room's): that game is
    /// left first (its server and room stop), so the game played alone has no link that could end it. Taken before the
    /// game's step plays alone.
    fn alone_asked(&mut self) {
        let asked = !matches!(self.game.global("$World::AloneAsked").as_str(), "" | "0");
        if !asked || !self.rooms || !self.link.as_ref().is_some_and(Link::away) {
            return;
        }
        let _ = self.game.call("disconnect", &[]);
        self.back_to_own();
    }

    /// A game the page left its own for (hosted, or a room's) is over: any it hosts stops (its players ended), and the
    /// page's own game is back, at its menu.
    fn back_to_own(&mut self) {
        if self.link.as_ref().is_some_and(Link::hosted) {
            torque::app::web_join::ask_page("back");
        }
        self.own_game();
    }

    /// An invite link's world left (the Esc menu's Leave, the host left, the connection lost), or on a page with a room
    /// service a hosted game's or a room's: the page's own game again.
    fn left_link_world(&mut self) {
        let in_world = self.game.in_world();
        let Some(link) = self.link.as_mut().filter(|l| !l.waiting) else {
            self.in_link_world = false;
            return;
        };
        let left = self.in_link_world && !in_world;
        self.in_link_world = in_world;
        if !left {
            return;
        }
        if link.page() {
            if !link.asked_back {
                link.asked_back = true;
                torque::app::web_join::ask_page("back");
            }
        } else if self.rooms && link.away() {
            self.back_to_own();
        }
    }

    /// A link that ended for good: out of the world, with why on the menu.
    fn show_end(&mut self) {
        let Some(link) = self.link.as_mut().filter(|l| !l.ended) else {
            return;
        };
        let Some(text) = torque::app::web_join::ended_text(&link.watch) else {
            return;
        };
        link.ended = true;
        // an invite link's: the page's own game again, with why on its menu (once)
        let page = link.page();
        let back = page && !link.asked_back;
        link.asked_back |= page;
        let away = self.rooms && link.away();
        if self.game.global("$World::InWorld") == "1" {
            let _ = self.game.call("leaveWorld", &[]);
        }
        self.menu_status(&text);
        if back {
            torque::app::web_join::ask_page("back");
        }
        // a room's game over: the page's own game again, with why on its menu
        if away {
            self.back_to_own();
        }
    }

    /// Shows `text` on the menu's status line, handed to `setText` as an argument (never built into script source).
    fn menu_status(&mut self, text: &str) {
        let vm = self.game.vm_mut();
        if let Some(status) = vm.objects().find(b"MenuStatus") {
            let _ = vm.call_method(status, b"setText", &[text.as_bytes().to_vec()]);
        }
    }

    /// What the menu asked of the page: to host a game or to join a room by its code (both in place), or its invite
    /// panels (a solo page).
    fn menu_asks(&mut self) {
        let ask = self.game.global("$World::PageAsk");
        if !ask.is_empty() {
            self.game.set_global("$World::PageAsk", "");
            torque::app::web_join::ask_page(&ask);
        }
        if self.game.take_flag("$World::WebHostAsked") {
            self.host_asked();
        }
        let code = self.game.global("$World::WebJoinCode");
        if !code.is_empty() {
            self.game.set_global("$World::WebJoinCode", "");
            if let Err(e) = self.join_room(&code) {
                self.menu_status(&e);
            }
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
        #[cfg(target_arch = "wasm32")]
        self.warm_interiors();
        #[cfg(target_arch = "wasm32")]
        self.alone_asked();
        App::advance(&mut self.game, ms);
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(mode) = torque::app::web_join::take_switch()
                && let Err(e) = self.switch(mode)
            {
                self.menu_status(&format!("Could not join: {e}"));
                // the invite link ended before its switch: the page's own game goes on
                if mode == torque::app::web_join::Mode::Link {
                    torque::app::web_join::ask_page("back");
                }
                // the page's server started for nothing: it stops (its room with it), and a later Host a game starts
                // another
                if mode == torque::app::web_join::Mode::Host && self.rooms {
                    torque::app::web_join::ask_page("back");
                    if self.link.as_ref().is_some_and(Link::hosted) {
                        self.own_game();
                    }
                }
            }
            self.join_when_open();
            self.show_end();
            self.left_link_world();
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
        self.play_when_warm
            || self.game.server.is_some() && (!self.game.in_world() || self.game.figure().is_none())
    }
}
