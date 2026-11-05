//! The level demo in a browser: the files it carries (a pack made by `example-level-demo --web-pack`, embedded when it is
//! built: the path in `TORQUE_WEB_PACK`), and the engine's window as a canvas on the page.
//!
//! **The page.** `?map=<name>` starts the map `missions/<name>.mis` at once instead of showing the menu. The page's root element
//! carries the game's state for a test to wait on: `data-torque-frames` (the frames drawn), `data-torque-map` (the map being
//! played, empty on the menu), `data-torque-figure` (`1` once the client holds its figure's ghost) and `data-torque-position`
//! (where the client sees its figure, `x y z`).

use torque::app::view::WorldView;
use torque::app::{App, Host};
use torque::gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::client::LevelDemo;
use crate::files::GameFiles;

/// The files the build carries.
static PACK: &[u8] = include_bytes!(env!("TORQUE_WEB_PACK"));

/// Starts the game on the page (panics, shown in the page's console, when it cannot).
#[allow(clippy::panic)]
pub fn start() {
    console_error_panic_hook::set_once();
    if let Err(e) = run() {
        panic!("the level demo: {e}");
    }
}

fn run() -> Result<(), String> {
    let files = GameFiles::unpack(PACK)?;
    let mut demo = LevelDemo::new(files)?;
    if let Some(name) = query("map") {
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

/// The value of `name` in the page's query string.
fn query(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    search
        .trim_start_matches('?')
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// The level demo, its state written on the page as it draws (module docs).
struct WebDemo {
    demo: LevelDemo,
    frames: u64,
}

impl WebDemo {
    fn mark(&self) {
        let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        else {
            return;
        };
        let map = self
            .demo
            .playing
            .and_then(|i| self.demo.maps.get(i))
            .map(|m| m.path.as_str())
            .unwrap_or_default();
        let own = self.demo.figure();
        let figure = if own.is_some() { "1" } else { "0" };
        let position = own
            .map(|f| format!("{} {} {}", f.position[0], f.position[1], f.position[2]))
            .unwrap_or_default();
        let _ = root.set_attribute("data-torque-frames", &self.frames.to_string());
        let _ = root.set_attribute("data-torque-map", map);
        let _ = root.set_attribute("data-torque-figure", figure);
        let _ = root.set_attribute("data-torque-position", &position);
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
}
