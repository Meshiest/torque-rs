//! The level demo in a browser: its files, fetched by the page or carried inside the module, and the engine's window as a
//! canvas on the page. The page's calls and its settings are the engine's web entry ([`torque::app::web_page`]).
//!
//! **Its files.** Built with the `embed-pack` feature, the module carries them (a pack made by `example-level-demo --web-pack`,
//! its path in `TORQUE_WEB_PACK` when it is built) and the page calls [`start_embedded`]. Without it the page fetches the files
//! its list names (`files.json`, which `example-level-demo --web-files` writes beside the pack) and calls [`start_fetched`].
//! Either way the files become the same `GameFiles`.
//!
//! **The page.** `?map=<name>` starts the map `missions/<name>.mis` at once instead of showing the menu. The page's root element
//! carries the game's state for a test to wait on ([`web_page::mark_play`]): the frames drawn, the map being played (empty on
//! the menu), whether the client holds its figure's ghost and where it sees it.
//!
//! **Joining the office** (as the soccer page joins its server, `examples/mods-demo`): a page opened with `?join` on an office
//! server that served it (`office-server --transport both`), with `?server=<ws or wss URL>` for one elsewhere, or with
//! `?room=<code>` for one hosted through a room service, joins it at once over its game socket (a WebRTC data channel beside
//! it when the server offers one), through the same join screen as the window: the catalog mod coming in, kept in memory for
//! as long as the page is open. The catalog's menu runs in the page's Worker when the page has the runner (`web/build.mjs
//! --client-scripts`), after the player allows it. `?name=<name>` is the player's name. Its menu still plays the maps alone,
//! and its Join the office joins again. The root element also carries `data-torque-join` (`$Join::State`),
//! `data-torque-joins` (every state the join went through, `$Join::Seen`),
//! `data-torque-download` (`received total`, bytes), `data-torque-mods` (the mods' names, then their versions),
//! `data-torque-realm` (the server scripts' state: `idle`, `waiting`, `asking`, `running` or `off`), `data-torque-allow` (the
//! middle of the consent prompt's Allow, as shares of the canvas's width and height, `x y`, while it is up),
//! `data-torque-cards` (the catalog's cards shown),
//! `data-torque-chair` (the middle of its Chair card, as shares), `data-torque-catalog-status` (its status line: empty after a
//! spawn the server took), `data-torque-catalog-recent` (the newest line of its list of spawns),
//! `data-torque-props` (the props the client holds), `data-torque-error` (the client's last console error), `data-torque-status` (the menu's status line) and the link's
//! ([`torque::app::web_join::mark_link`]).

use torque::app::run::EngineRun as _;
use torque::app::view::WorldView;
use torque::app::web_join::{SERVER, Target, ended_text, mark_link, query};
use torque::app::web_page::{self, mark};
use torque::app::{App, Host};
use torque::client::net::web_rtc::LinkWatch;
use torque::engine::Engine;
use torque::gui::GuiDraws;
use torque::gui::client_script::{PromptSpot, prompt_spots};
use torque::mods::CacheChoice;
use torque_client_script_host::{Runner, Trap, WebRunner, page_runner_ready};
use torque_plugin_client_script::{Choices, RealmState, RunnerFactory};
use torque_plugin_props::ClientProps;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::client::{LevelDemo, MatchOptions};
use crate::files::GameFiles;
use crate::office::{
    ABILITIES, ServerScripts, card_controls, center, office_engine, recent_of, status_of,
};
use crate::plugin::level_engine;

/// The files the build carries.
#[cfg(feature = "embed-pack")]
static PACK: &[u8] = include_bytes!(env!("TORQUE_WEB_PACK"));

/// Whether the module carries its files (the `embed-pack` feature).
pub const EMBEDDED: bool = cfg!(feature = "embed-pack");

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

/// Starts the game on the fetched files (Err: one the list names is missing, or a pack is broken).
pub fn start_fetched() -> Result<(), String> {
    start(GameFiles::from_fetched(web_page::take_fetched()?)?)
}

fn start(files: GameFiles) -> Result<(), String> {
    if let Some(target) = Target::from_page()? {
        return join(files, target);
    }
    level_engine(files)?
        .app(|engine| {
            let mut demo = LevelDemo::from_engine(engine)?;
            if let Some(name) = web_page::query("map") {
                let want = format!("missions/{name}.mis");
                let index = demo
                    .maps
                    .iter()
                    .position(|m| m.path.eq_ignore_ascii_case(&want))
                    .ok_or_else(|| format!("no map {want}"))?;
                demo.start_map(index)?;
            }
            Ok(WebDemo {
                demo,
                link: None,
                frames: 0,
            })
        })
        .run()
        .map_err(|e| e.to_string())
}

/// The page joins the office at `target` at once (module docs).
fn join(files: GameFiles, target: Target) -> Result<(), String> {
    let options = MatchOptions {
        name: query("name")
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Browser player".to_string()),
        join_only: true,
        ..MatchOptions::default()
    };
    office_engine(files, Vec::new(), CacheChoice::Memory)?
        .app(move |engine| WebDemo::joined(engine, options.clone(), target.clone()))
        .run()
        .map_err(|e| e.to_string())
}

/// The realm's guest in the page's Worker, when the page started its runner.
fn page_runner() -> Option<Box<dyn RunnerFactory>> {
    if !page_runner_ready() {
        return None;
    }
    let open = |limits| {
        WebRunner::open(&[], limits)
            .map(|runner| Box::new(runner) as Box<dyn Runner>)
            .map_err(|e| Trap::Fault(e.to_string()))
    };
    Some(Box::new(open))
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
        Target::WebTransport => "the server, over WebTransport".to_string(),
    }
}

/// The page's link to the office it joins.
struct Link {
    target: Target,
    shown: String,
    watch: LinkWatch,
    /// Whether the present link's end was shown.
    ended: bool,
}

/// The level demo, its state written on the page as it draws (module docs).
struct WebDemo {
    demo: LevelDemo,
    link: Option<Link>,
    frames: u64,
}

impl WebDemo {
    fn joined(engine: &Engine, options: MatchOptions, target: Target) -> Result<WebDemo, String> {
        let (socket, watch) = target.open()?;
        let scripts = ServerScripts::new(Choices::in_memory(), page_runner());
        let demo = LevelDemo::for_matches(engine, options, Some(socket), scripts)?;
        let mut web = WebDemo {
            demo,
            link: Some(Link {
                shown: shown(&target),
                target,
                watch,
                ended: false,
            }),
            frames: 0,
        };
        web.join()?;
        Ok(web)
    }

    /// Joins the page's office over the link the client has.
    fn join(&mut self) -> Result<(), String> {
        let shown = self
            .link
            .as_ref()
            .map(|l| l.shown.clone())
            .unwrap_or_default();
        self.demo
            .client
            .call("joinServer", &[&SERVER.to_string(), &shown])
            .map(drop)
    }

    /// The menu's Join the office: a new link to the page's office, joined.
    fn rejoin(&mut self) -> Result<(), String> {
        let link = self.link.as_mut().ok_or("this page joins no office")?;
        let (socket, watch) = link.target.open()?;
        link.watch = watch;
        link.ended = false;
        self.demo.set_socket(socket);
        self.join()
    }

    /// The share of the canvas's width and height `at` is (GUI pixels), as `x y`.
    fn shares(&self, at: (i32, i32)) -> String {
        let [w, h] = {
            let size = self.demo.client.host.gui.0.borrow().canvas_size;
            [size.0.max(1) as f32, size.1.max(1) as f32]
        };
        format!("{} {}", at.0 as f32 / w, at.1 as f32 / h)
    }

    fn mark(&self) {
        let map = self
            .demo
            .playing
            .and_then(|i| self.demo.maps.get(i))
            .map(|m| m.path.as_str())
            .unwrap_or_default();
        let figure = self.demo.figure().map(|f| f.position);
        web_page::mark_play(self.frames, map, figure);
        let Some(link) = &self.link else {
            return;
        };
        let client = &self.demo.client;
        mark("join", &client.global("$Join::State"));
        mark("joins", &client.global("$Join::Seen"));
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
        let realm = self
            .demo
            .scripts
            .as_ref()
            .map_or(RealmState::Idle, |s| s.realms.state());
        let word = match realm {
            RealmState::Idle => "idle",
            RealmState::Waiting => "waiting",
            RealmState::Asking => "asking",
            RealmState::Running => "running",
            RealmState::Off => "off",
        };
        mark("realm", word);
        let allow = (realm == RealmState::Asking)
            .then(|| {
                let size = client.host.gui.0.borrow().canvas_size;
                prompt_spots(size, ABILITIES)
                    .into_iter()
                    .find(|(spot, _)| *spot == PromptSpot::Allow)
                    .map(|(_, rect)| self.shares(center(rect)))
            })
            .flatten()
            .unwrap_or_default();
        mark("allow", &allow);
        let cards: Vec<_> = card_controls(client).collect();
        mark("cards", &cards.len().to_string());
        let chair = cards
            .iter()
            .find(|c| c.text == "Chair")
            .map(|c| self.shares(center(c.rect)))
            .unwrap_or_default();
        mark("chair", &chair);
        mark("catalog-status", &status_of(client));
        mark(
            "catalog-recent",
            recent_of(client).first().map_or("", String::as_str),
        );
        let props = self
            .demo
            .engine()
            .resource::<ClientProps>()
            .and_then(|book| book.with(|b| b.props().len()))
            .unwrap_or(0);
        mark("props", &props.to_string());
        let error = client
            .client_log
            .iter()
            .rev()
            .find(|l| l.starts_with("ERROR"))
            .map_or("", String::as_str);
        mark("error", error);
        mark_link(&link.watch, self.frames % 120 == 1);
    }

    /// The link's end, shown once: out of the office, the reason on the menu.
    fn watch_link(&mut self) {
        let Some(link) = self.link.as_mut() else {
            return;
        };
        if link.ended {
            return;
        }
        let Some(text) = ended_text(&link.watch) else {
            return;
        };
        link.ended = true;
        if !self.demo.client.global("$Join::State").is_empty() {
            let _ = self.demo.client.call("leaveJoin", &[&text]);
        }
        self.demo.status(&text);
    }
}

impl App for WebDemo {
    fn host(&mut self) -> &mut Host {
        self.demo.host()
    }

    fn host_ref(&self) -> &Host {
        self.demo.host_ref()
    }

    fn title(&self) -> String {
        self.demo.title()
    }

    fn log_name(&self) -> &str {
        self.demo.log_name()
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        self.demo.view(gpu)
    }

    fn advance(&mut self, ms: u32) {
        App::advance(&mut self.demo, ms);
        self.watch_link();
        if self.demo.client.take_flag("$Game::JoinAsked")
            && let Err(e) = self.rejoin()
        {
            self.demo.status(&format!("Could not join: {e}"));
        }
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let drawn = App::frame(&mut self.demo, view);
        self.frames += 1;
        if self.frames % 15 == 1 {
            self.mark();
            if self.link.is_some() {
                let status = self
                    .demo
                    .client
                    .call("eval", &["return MenuStatus.getValue();"])
                    .unwrap_or_default();
                mark("status", &status);
            }
        }
        drawn
    }

    /// A map is loading until the client holds its figure; a join shows its own screen meanwhile.
    fn loading(&self) -> bool {
        let joining = !matches!(
            self.demo.client.global("$Join::State").as_str(),
            "" | "playing"
        );
        self.demo.playing.is_some() && self.demo.figure().is_none() && !joining
    }
}
