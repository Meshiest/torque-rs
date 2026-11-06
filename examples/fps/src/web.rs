//! The arena in a browser, join-only: its files, fetched by the page or carried inside the module, the engine's window as a
//! canvas on the page, and a game socket over WebSocket to the server that served the page (`fps-server --transport web` or
//! `both`), with a WebRTC data channel beside it when the server offers one (`--webrtc`).
//!
//! **Its files**, as the level demo's: built with the `embed-pack` feature the module carries them (a pack made by
//! `fps-client --web-pack`, its path in `TORQUE_WEB_PACK` when it is built) and the page calls [`start_embedded`]; without
//! it the page fetches the files `files.json` names (`fps-client --web-files` writes both) and hands them over:
//! [`read_manifest`], [`add_file`] for each (taken only when its size and hash are the list's), then [`start_fetched`].
//!
//! **The page.** The game socket opens `ws://<the page's host>/ws` (`wss://` on an `https` page), or the address in
//! `?server=<ws or wss URL>` when the page is served elsewhere; `?name=<name>` sets the player's name. The client joins at
//! once; when the game ends or the connection is lost the main menu offers only to join again. The page's root element
//! carries the game's state for a test to wait on: `data-torque-frames` (the frames drawn), `data-torque-map` (the map
//! being played, empty off it), `data-torque-figure` (`1` once the client predicts its own figure),
//! `data-torque-position` (where, `x y z`), `data-torque-others` (the other players' figures the client holds),
//! `data-torque-status` (the menu's status line), `data-torque-link` (`websocket`, `connecting` or `datachannel`: which
//! carries the datagrams), `data-torque-rtc-tried` and `data-torque-rtc-failed` (`1` once a data channel was tried, or
//! failed or was refused), `data-torque-rtc-in` (the messages the channel brought) and `data-torque-rtc-gap` (the longest
//! time between two of them, in milliseconds, over the last two seconds or so).
//!
//! **A player's game** (`?room=<code>`): the page joins the game a player hosts (`fps-server --room`), found by its code
//! through the room service at `?rooms=<ws or wss URL>` (else the page's own host at `/ws`: the service serves the
//! page). Every datagram goes on the data channel to the host; when it closes, the game ends with a message on the menu
//! (`data-torque-ended` carries it).
//!
//! **Played alone** (a page `web/build.mjs --solo` marked, for a site with no game server): the page runs the server in its
//! worker as a host player's does, with no room, and calls [`start_solo_client`]; the game waits at the main menu, which
//! offers to play alone, to host with a link and to join with one, and the Esc menu to invite players. Those ask the page (its `torqueAsk`,
//! `web/invites.js`, through [`torque::app::web_join::ask_page`]), which trades connection codes by hand.
//!
//! **Joined by an invite link** (`web/invites.js`): the page holds the data channel to the host. A page opened on an invite
//! starts as one played alone, with the invite's panel over its menu; once the channel opens the page switches the game
//! to it ([`switch_to`] `link`): the client moves to [`Target::Page`], fed by the page on its own link ([`link_datagram`]),
//! and joins ([`link_opened`]: the trade of links by hand takes longer than a join waits for its answer). Leaving that game
//! (the Esc menu's Leave, or the host left) asks the page to take the game back to playing alone (`back`), which it does
//! with `solo`: the client is back on the worker's link, at the menu. The worker's server runs all along.
//! ([`start_link_client`] starts a client on [`Target::Page`] at once, for a page that has no game of its own.)

use std::cell::RefCell;

use torque::app::manifest::Manifest;
use torque::app::view::WorldView;
use torque::app::web_join::{Mode, SERVER, Target, query, rooms_url, socket_url};
use torque::app::{App, Host};
use torque::client::net::web_rtc::{Link, LinkWatch};
use torque::gui::GuiDraws;
use torque::script::text::atoi;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::client::{ArenaGame, ClientOptions};
use crate::files::GameFiles;

/// The files the build carries.
#[cfg(feature = "embed-pack")]
static PACK: &[u8] = include_bytes!(env!("TORQUE_WEB_PACK"));

/// The files fetched so far and the list they were fetched by.
struct Fetched {
    manifest: Manifest,
    files: Vec<(String, Vec<u8>)>,
}

thread_local! {
    static FETCHED: RefCell<Option<Fetched>> = const { RefCell::new(None) };
}

/// Shows a panic in the page's console.
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Whether the module carries its files (the `embed-pack` feature).
pub fn embedded() -> bool {
    cfg!(feature = "embed-pack")
}

/// Starts the game on the files the module carries.
#[cfg(feature = "embed-pack")]
pub fn start_embedded() -> Result<(), String> {
    start(GameFiles::unpack(PACK)?, None, false)
}

/// Starts the game on the files the module carries: this build carries none.
#[cfg(not(feature = "embed-pack"))]
pub fn start_embedded() -> Result<(), String> {
    Err("this build carries no files: the page fetches them by its list".into())
}

/// Reads the list of the files to fetch (Err: it is not one, or it breaks a cap), forgetting any files added before.
pub fn read_manifest(text: &str) -> Result<(), String> {
    let manifest = Manifest::parse(text)?;
    FETCHED.with_borrow_mut(|f| {
        *f = Some(Fetched {
            manifest,
            files: Vec::new(),
        })
    });
    Ok(())
}

/// Takes the fetched file `path` (Err, the file left out: the list does not name it, or its size or hash is not the list's).
pub fn add_file(path: &str, bytes: Vec<u8>) -> Result<(), String> {
    FETCHED.with_borrow_mut(|f| {
        let f = f.as_mut().ok_or("no list of files was read")?;
        let entry = f
            .manifest
            .file(path)
            .ok_or_else(|| format!("{path} is not in the list of files"))?;
        entry.check(&bytes)?;
        let path = entry.path.clone();
        f.files.retain(|(p, _)| *p != path);
        f.files.push((path, bytes));
        Ok(())
    })
}

/// The fetched files, taken (Err: one the list names is missing, or a pack is broken).
pub(crate) fn fetched_files() -> Result<GameFiles, String> {
    let Fetched { manifest, files } = FETCHED.take().ok_or("no list of files was read")?;
    if let Some(missing) = manifest
        .files
        .iter()
        .find(|m| !files.iter().any(|(p, _)| *p == m.path))
    {
        return Err(format!("{} was not fetched", missing.path));
    }
    GameFiles::from_fetched(files)
}

/// Starts the game on the fetched files (Err: one the list names is missing, or a pack is broken).
pub fn start_fetched() -> Result<(), String> {
    start(fetched_files()?, None, false)
}

/// Starts the host player's game on the fetched files: its server runs in the page's worker (`web_host`), and the client
/// reaches it over the local link ([`local_datagram`], [`set_local_sender`]).
pub fn start_host_client() -> Result<(), String> {
    start(fetched_files()?, Some(Target::Local), false)
}

/// Starts the game played alone on the fetched files: as [`start_host_client`], with nobody else let in and a menu that
/// offers neither hosting nor joining.
pub fn start_solo_client() -> Result<(), String> {
    start(fetched_files()?, Some(Target::Local), true)
}

/// Starts a client joined by an invite link on the fetched files: the page holds its data channel to the host (module
/// docs).
pub fn start_link_client() -> Result<(), String> {
    start(fetched_files()?, Some(Target::Page), false)
}

pub use torque::app::web_join::{
    link_datagram, link_ended, link_opened, local_datagram, set_link_sender, set_local_sender,
    switch_to,
};

fn start(files: GameFiles, target: Option<Target>, solo: bool) -> Result<(), String> {
    let target = match (target, query("room")) {
        (Some(target), _) => target,
        (None, Some(code)) => Target::Room {
            url: rooms_url()?,
            code,
        },
        (None, None) => Target::Server(socket_url()?),
    };
    let options = ClientOptions {
        name: query("name")
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Browser player".to_string()),
        ..ClientOptions::default()
    };
    let (socket, watch) = target.open()?;
    let mut game = ArenaGame::with_socket(files, options, socket)?;
    game.set_global("$Arena::JoinOnly", "1");
    if solo {
        game.set_global("$Arena::Solo", "1");
    }
    if target == Target::Page {
        game.set_global("$Arena::Invited", "1");
    }
    game.call("showMainMenu", &[])?;
    let mut web = WebArena {
        game,
        target,
        frames: 0,
        watch,
        ended: false,
        joined: false,
        in_link_game: false,
        asked_back: false,
    };
    // Played alone the game waits at the main menu; a link's join waits for its channel (module docs).
    if !solo && web.target != Target::Page {
        web.joined = true;
        web.join()?;
    } else if web.target == Target::Page {
        web.status("Waiting for the connection to the host...");
    }
    torque::app::window::run(web)
}

/// The arena, its state written on the page as it draws (module docs).
struct WebArena {
    game: ArenaGame,
    target: Target,
    frames: u64,
    /// The links of the present game socket.
    watch: LinkWatch,
    /// Whether the present link's end was shown.
    ended: bool,
    /// Whether the client joined on its link yet (a game played alone waits at the menu, a link's join for its channel).
    joined: bool,
    /// Whether the client was in an invite link's game at the last step (leaving it takes the page back to its own).
    in_link_game: bool,
    /// Whether the page was asked to take the game back to playing alone (once a game).
    asked_back: bool,
}

impl WebArena {
    /// Joins the page's game (over the game socket the client has).
    fn join(&mut self) -> Result<(), String> {
        self.game.call("connectTo", &[&SERVER.to_string()])?;
        Ok(())
    }

    /// The menu's Join (Play alone): the first join on the link the client has, then a new link each time.
    fn join_asked(&mut self) -> Result<(), String> {
        if self.joined {
            return self.rejoin();
        }
        let (open, ended) = {
            let w = self.watch.borrow();
            (w.link == Link::Channel, w.ended.is_some())
        };
        if self.target == Target::Page && !open {
            if ended {
                return self.rejoin();
            }
            self.status("Waiting for the connection to the host...");
            return Ok(());
        }
        self.joined = true;
        self.join()
    }

    /// Shows `text` on the main menu's status line, handed to `setText` as an argument (never built into script source).
    fn status(&mut self, text: &str) {
        let vm = self.game.vm_mut();
        if let Some(status) = vm.objects().find(b"MainStatus") {
            let _ = vm.call_method(status, b"setText", &[text.as_bytes().to_vec()]);
        }
    }

    /// Opens a new link and joins again (the menu's Join).
    fn rejoin(&mut self) -> Result<(), String> {
        let (socket, watch) = self.target.open()?;
        self.watch = watch;
        self.ended = false;
        self.game.set_socket(socket);
        self.join()
    }

    /// Switches the running game in place (module docs): out of any game, onto the link `mode` goes by, at the main menu.
    /// The status line keeps what it says (why an invite link's game ended).
    fn switch(&mut self, mode: Mode) -> Result<(), String> {
        if self.game.global("$Arena::InGame") == "1" {
            self.game.call("leaveGame", &[])?;
        } else {
            self.game.call("disconnect", &[])?;
        }
        let target = if mode == Mode::Link {
            Target::Page
        } else {
            Target::Local
        };
        let (socket, watch) = target.open()?;
        self.game.set_socket(socket);
        self.watch = watch;
        self.target = target;
        self.joined = false;
        self.ended = false;
        self.in_link_game = false;
        self.asked_back = false;
        let link = mode == Mode::Link;
        self.game
            .set_global("$Arena::Solo", if link { "0" } else { "1" });
        self.game
            .set_global("$Arena::Invited", if link { "1" } else { "0" });
        self.game.set_global("$Arena::JoinAsked", "0");
        self.game.set_global("$Arena::PageAsk", "");
        self.game.call("showMainMenu", &[])?;
        if link {
            self.status("Waiting for the connection to the host...");
        }
        Ok(())
    }

    /// A link that ended for good (a player's host left, or the room refused): out of the game, with why on the menu.
    fn show_end(&mut self) {
        let Some(text) = torque::app::web_join::ended_text(&self.watch) else {
            return;
        };
        self.ended = true;
        if self.game.global("$Arena::InGame") == "1" {
            let _ = self.game.call("leaveGame", &[]);
        }
        self.status(&text);
    }

    fn mark(&mut self) {
        let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        else {
            return;
        };
        let in_game = self.game.in_game();
        let map = if in_game {
            self.game.global("$Arena::Map")
        } else {
            String::new()
        };
        let own = self.game.own_look();
        let position = own
            .and_then(|o| o.position)
            .map(|p| format!("{} {} {}", p[0], p[1], p[2]))
            .unwrap_or_default();
        let figure = if in_game && !position.is_empty() {
            "1"
        } else {
            "0"
        };
        let own_ghost = u32::try_from(atoi(self.game.global("$Arena::OwnGhost").as_bytes())).ok();
        let others = self
            .game
            .with_world(|w| {
                w.records
                    .ghosts
                    .iter()
                    .filter(|(id, g)| {
                        g.class == Some(torque::ghosts::TgeClass::Player) && Some(**id) != own_ghost
                    })
                    .count()
            })
            .unwrap_or(0);
        let status = self
            .game
            .call("eval", &["return MainStatus.getValue();"])
            .unwrap_or_default();
        let _ = root.set_attribute("data-torque-frames", &self.frames.to_string());
        let _ = root.set_attribute("data-torque-map", &map);
        let _ = root.set_attribute("data-torque-figure", figure);
        let _ = root.set_attribute("data-torque-position", &position);
        let _ = root.set_attribute("data-torque-others", &others.to_string());
        let _ = root.set_attribute("data-torque-status", &status);
        // A window of about two seconds (eight marks).
        torque::app::web_join::mark_link(&self.watch, self.frames % 120 == 1);
    }
}

impl App for WebArena {
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
        if let Some(mode) = torque::app::web_join::take_switch()
            && let Err(e) = self.switch(mode)
        {
            self.status(&format!("Could not switch: {e}"));
        }
        if !self.ended {
            self.show_end();
        }
        // An invite link's game left (the Esc menu's Leave, the host left, the connection lost): the page's own game again.
        let in_game = self.game.global("$Arena::InGame") == "1";
        if self.target == Target::Page && self.in_link_game && !in_game && !self.asked_back {
            self.asked_back = true;
            torque::app::web_join::ask_page("back");
        }
        self.in_link_game = self.target == Target::Page && in_game;
        // The menus' Host with a link, Join with Link and Invite players (played alone), and an invited game's Play alone
        // instead: the page's invite panels, or the page's own game again, played alone.
        let ask = self.game.global("$Arena::PageAsk");
        if !ask.is_empty() {
            self.game.set_global("$Arena::PageAsk", "");
            torque::app::web_join::ask_page(&ask);
        }
        // A link's channel open: its join.
        if !self.joined && self.target == Target::Page && self.watch.borrow().link == Link::Channel
        {
            self.joined = true;
            if let Err(e) = self.join() {
                self.status(&format!("Could not join: {e}"));
            }
        }
        if self.game.take_flag("$Arena::JoinAsked")
            && let Err(e) = self.join_asked()
        {
            self.status(&format!("Could not join: {e}"));
        }
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let drawn = App::frame(&mut self.game, view);
        self.frames += 1;
        if self.frames % 15 == 1 {
            self.mark();
        }
        drawn
    }
}
