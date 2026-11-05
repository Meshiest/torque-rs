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

use torque::app::view::WorldView;
use torque::app::web_page;
use torque::app::{App, Host};
use torque::gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::client::LevelDemo;
use crate::files::GameFiles;

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
    let mut demo = LevelDemo::new(files)?;
    if let Some(name) = web_page::query("map") {
        let want = format!("missions/{name}.mis");
        let index = demo
            .maps
            .iter()
            .position(|m| m.path.eq_ignore_ascii_case(&want))
            .ok_or_else(|| format!("no map {want}"))?;
        demo.start_map(index)?;
    }
    torque::app::window::run(WebDemo { demo, frames: 0 })
}

/// The level demo, its state written on the page as it draws (module docs).
struct WebDemo {
    demo: LevelDemo,
    frames: u64,
}

impl WebDemo {
    fn mark(&self) {
        let map = self
            .demo
            .playing
            .and_then(|i| self.demo.maps.get(i))
            .map(|m| m.path.as_str())
            .unwrap_or_default();
        let figure = self.demo.figure().map(|f| f.position);
        web_page::mark_play(self.frames, map, figure);
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
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let drawn = App::frame(&mut self.demo, view);
        self.frames += 1;
        if self.frames % 15 == 1 {
            self.mark();
        }
        drawn
    }

    /// A map is loading until the client holds its figure.
    fn loading(&self) -> bool {
        self.demo.playing.is_some() && self.demo.figure().is_none()
    }
}
