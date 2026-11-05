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

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddrV4};

use torque::app::manifest::Manifest;
use torque::app::view::WorldView;
use torque::app::{App, Host};
use torque::client::net::web_rtc::{LinkWatch, WebRtcDatagrams};
use torque::client::net::web_socket::WebSocketDatagrams;
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

/// The address the client names the server by: the game socket goes to the page's server whatever it is sent to.
const SERVER: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_000);

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
    start(GameFiles::unpack(PACK)?)
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

/// Starts the game on the fetched files (Err: one the list names is missing, or a pack is broken).
pub fn start_fetched() -> Result<(), String> {
    let Fetched { manifest, files } = FETCHED.take().ok_or("no list of files was read")?;
    if let Some(missing) = manifest
        .files
        .iter()
        .find(|m| !files.iter().any(|(p, _)| *p == m.path))
    {
        return Err(format!("{} was not fetched", missing.path));
    }
    start(GameFiles::from_fetched(files)?)
}

fn start(files: GameFiles) -> Result<(), String> {
    let url = socket_url()?;
    let options = ClientOptions {
        name: query("name")
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Browser player".to_string()),
        ..ClientOptions::default()
    };
    let socket = WebRtcDatagrams::new(WebSocketDatagrams::open(&url, SERVER)?);
    let watch = socket.watch();
    let mut game = ArenaGame::with_socket(files, options, Box::new(socket))?;
    game.set_global("$Arena::JoinOnly", "1");
    game.call("showMainMenu", &[])?;
    let mut web = WebArena {
        game,
        url,
        frames: 0,
        watch,
    };
    web.join()?;
    torque::app::window::run(web)
}

/// The value of `name` in the page's query string, decoded.
fn query(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let raw = search
        .trim_start_matches('?')
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.replace('+', " "))?;
    js_sys::decode_uri_component(&raw)
        .ok()
        .and_then(|v| v.as_string())
}

/// The game socket's URL: `?server=`, else the page's own host at `/ws`.
fn socket_url() -> Result<String, String> {
    if let Some(url) = query("server") {
        if url.starts_with("ws://") || url.starts_with("wss://") {
            return Ok(url);
        }
        return Err(format!(
            "?server= takes a ws:// or wss:// address, not {url:?}"
        ));
    }
    let location = web_sys::window().ok_or("no window")?.location();
    let protocol = location.protocol().map_err(|e| format!("{e:?}"))?;
    let host = location.host().map_err(|e| format!("{e:?}"))?;
    let scheme = if protocol == "https:" { "wss" } else { "ws" };
    // The server's web transport opens game sockets on this path (its `WEB_SOCKET_PATH`).
    Ok(format!("{scheme}://{host}/ws"))
}

/// The arena, its state written on the page as it draws (module docs).
struct WebArena {
    game: ArenaGame,
    url: String,
    frames: u64,
    /// The links of the present game socket.
    watch: LinkWatch,
}

impl WebArena {
    /// Joins the page's game (over the game socket the client has).
    fn join(&mut self) -> Result<(), String> {
        self.game.call("connectTo", &[&SERVER.to_string()])?;
        Ok(())
    }

    /// Opens a new game socket and joins again (the menu's Join).
    fn rejoin(&mut self) -> Result<(), String> {
        let socket = WebRtcDatagrams::new(WebSocketDatagrams::open(&self.url, SERVER)?);
        self.watch = socket.watch();
        self.game.set_socket(Box::new(socket));
        self.join()
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
        let mut link = self.watch.borrow_mut();
        let flag = |b: bool| if b { "1" } else { "0" };
        let _ = root.set_attribute("data-torque-link", link.link.name());
        let _ = root.set_attribute("data-torque-rtc-tried", flag(link.tried));
        let _ = root.set_attribute("data-torque-rtc-failed", flag(link.failed));
        let _ = root.set_attribute("data-torque-rtc-in", &link.messages_in.to_string());
        let _ = root.set_attribute("data-torque-rtc-gap", &format!("{:.0}", link.max_gap_ms));
        // A window of about two seconds (eight marks).
        if self.frames % 120 == 1 {
            link.max_gap_ms = 0.0;
        }
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
        if self.game.take_flag("$Arena::JoinAsked")
            && let Err(e) = self.rejoin()
        {
            let text = format!("Could not join: {e}");
            let _ = self.game.call(
                "eval",
                &[&format!(
                    "MainStatus.setText(\"{}\");",
                    text.replace('"', "'")
                )],
            );
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
