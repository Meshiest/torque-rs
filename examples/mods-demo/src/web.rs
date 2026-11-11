//! The soccer game in a browser, join-only, as the arena's page is (`examples/fps`'s `web`): its files fetched by the page
//! (the game's scripts, none of the mods), the engine's window as a canvas, and a game socket over WebSocket to the server
//! that served the page (`mods-demo-server --transport web` or `both`), with a WebRTC data channel beside it when the server
//! offers one (`--webrtc`), or to a desktop host by its room's code (`?room=<code>`, through the room service at `?rooms=`
//! or the page's own host). The page joins at once and shows the same download screen as the window; the server's mods are
//! kept in memory for as long as the page is open.
//!
//! **The page.** `?server=<ws or wss URL>` names a server elsewhere, `?name=<name>` the player's name. The page's root element
//! carries the game's state for a check to wait on: `data-torque-frames`, `data-torque-join` (`$Join::State`),
//! `data-torque-download` (`received total`, bytes), `data-torque-mods` (the mods the server plays, `id version`),
//! `data-torque-team`, `data-torque-figure` (`1` once the client holds its own player), `data-torque-position` (where,
//! `x y z`), `data-torque-attack` (the way its team attacks along y, `1` or `-1`) and `data-torque-status` (the menu's
//! status line), and the link's (`data-torque-link` and the rest, [`torque::app::web_join::mark_link`]).

use torque::app::files::GameFiles;
use torque::app::run::EngineRun as _;
use torque::app::view::WorldView;
use torque::app::web_join::{
    SERVER, Target, ended_text, mark_link, query, room_code, rooms_url, socket_url,
};
use torque::app::web_page::{self, mark};
use torque::app::{App, Host};
use torque::client::net::web_rtc::LinkWatch;
use torque::engine::Engine;
use torque::gui::GuiDraws;
use torque::mods::CacheChoice;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::client_builder;
use crate::game::{ClientOptions, SoccerGame};

/// This build carries no files: the page fetches them by its list.
pub fn start_embedded() -> Result<(), String> {
    Err("this build carries no files: the page fetches them by its list".into())
}

/// The fetched files: the pack the list names (Err: none, or it is broken).
fn fetched_files() -> Result<GameFiles, String> {
    let fetched = web_page::take_fetched()?;
    let (_, pack) = fetched
        .iter()
        .find(|(path, _)| path.eq_ignore_ascii_case(web_page::WEB_PACK))
        .ok_or("the list names no game.pack")?;
    GameFiles::unpack(pack)
}

/// Starts the game on the fetched files (Err: one the list names is missing, or a pack is broken).
pub fn start_fetched() -> Result<(), String> {
    let files = fetched_files()?;
    let target = match query("room") {
        Some(code) => Target::Room {
            url: rooms_url()?,
            code: room_code(&code)?,
        },
        None => Target::Server(socket_url()?),
    };
    let options = ClientOptions {
        name: query("name")
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Browser player".to_string()),
        ..ClientOptions::default()
    };
    let mut engine = client_builder(&files, CacheChoice::Memory)?;
    engine
        .app(move |engine| WebSoccer::new(engine, &files, options.clone(), target.clone()))
        .run()
        .map_err(|e| e.to_string())
}

/// What the join screen calls `target`: the host of the server's address (`ws://host:port/ws`), or the room by its code.
fn shown(target: &Target) -> String {
    match target {
        Target::Room { code, .. } => format!("room {code}"),
        Target::Server(url) => {
            let rest = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
            rest.split('/').next().unwrap_or(rest).to_string()
        }
        Target::Local | Target::Page => "the host".to_string(),
    }
}

/// The game, its state written on the page as it draws (module docs).
struct WebSoccer {
    game: SoccerGame,
    target: Target,
    /// What the join screen calls the page's game: its server's host, or the room's code.
    shown: String,
    watch: LinkWatch,
    /// Whether the present link's end was shown.
    ended: bool,
    frames: u64,
}

impl WebSoccer {
    fn new(
        engine: &Engine,
        files: &GameFiles,
        options: ClientOptions,
        target: Target,
    ) -> Result<WebSoccer, String> {
        let (socket, watch) = target.open()?;
        let mut game = SoccerGame::from_engine(engine, files, options, Some(socket))?;
        game.client.set_global("$Game::JoinOnly", "1");
        game.client.call("showMainMenu", &[])?;
        let mut web = WebSoccer {
            game,
            shown: shown(&target),
            target,
            watch,
            ended: false,
            frames: 0,
        };
        web.join()?;
        Ok(web)
    }

    /// Joins the page's game over the link the client has.
    fn join(&mut self) -> Result<(), String> {
        let server = SERVER.to_string();
        self.game
            .client
            .call("connectTo", &[&server, &self.shown])
            .map(drop)
    }

    /// The menu's Join: a new link to the page's game, joined.
    fn rejoin(&mut self) -> Result<(), String> {
        let (socket, watch) = self.target.open()?;
        self.watch = watch;
        self.ended = false;
        self.game.set_socket(socket);
        self.join()
    }

    fn mark(&mut self) {
        let client = &self.game.client;
        mark("frames", &self.frames.to_string());
        mark("join", &client.global("$Join::State"));
        mark(
            "download",
            &format!(
                "{} {}",
                client.global("$Join::Received"),
                client.global("$Join::Bytes")
            ),
        );
        mark(
            "mods",
            &format!(
                "{} {}",
                client.global("$Join::Mods"),
                client.global("$Join::Versions")
            ),
        );
        mark("team", &client.global("$Match::Team"));
        let figure = client
            .with_world(|w| w.control.object.is_some() && w.control.position.is_some())
            .unwrap_or(false);
        mark("figure", if figure { "1" } else { "0" });
        let position = client
            .with_world(|w| w.control.position)
            .flatten()
            .map(|[x, y, z]| format!("{x} {y} {z}"))
            .unwrap_or_default();
        mark("position", &position);
        mark("attack", &client.global("$Match::Attack"));
        let status = self
            .game
            .client
            .call("eval", &["return MainStatus.getValue();"])
            .unwrap_or_default();
        mark("status", &status);
        mark_link(&self.watch, self.frames % 120 == 1);
    }
}

impl App for WebSoccer {
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
        if !self.ended
            && let Some(text) = ended_text(&self.watch)
        {
            self.ended = true;
            if self.game.playing() {
                let _ = self.game.client.call("leaveMatch", &[]);
            }
            self.game.status(&text);
        }
        if self.game.client.take_flag("$Game::JoinAsked")
            && let Err(e) = self.rejoin()
        {
            self.game.status(&format!("Could not join: {e}"));
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
