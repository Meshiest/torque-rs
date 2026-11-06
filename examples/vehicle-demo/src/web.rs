//! The vehicle demo in a browser, played alone: its files fetched by the page (`files.json`, which `vehicle-demo --web-files`
//! writes beside the pack), the server on an in-memory link in the page (no socket: [`ClientOptions::alone_only`]), and the
//! engine's window as a canvas on the page. The page's calls and its settings are the engine's web entry
//! ([`torque::app::web_page`]). Natively only [`alone`] is of use (the tests).
//!
//! The game starts at once: the box figure beside its buggy on the hills. The page's root element carries the game's state for
//! a test to wait on (`web_page::mark_play`): the frames drawn, the map (empty off it), whether the client holds its figure
//! and where it sees it.

use torque::app::view::WorldView;
use torque::app::web_page;
use torque::app::{App, Host};
use torque::gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::client::{ClientOptions, VehicleDemo};
use crate::files::GameFiles;
#[cfg(target_arch = "wasm32")]
use crate::files::MISSION;

/// The module carries no files: the page fetches them.
pub const EMBEDDED: bool = false;

/// This build carries no files: the page fetches them by its list.
pub fn start_embedded() -> Result<(), String> {
    Err("this build carries no files: the page fetches them by its list".into())
}

/// Starts the game, played alone, on the fetched files (Err: one the list names is missing, or a pack is broken).
pub fn start_fetched() -> Result<(), String> {
    let files = GameFiles::from_fetched(web_page::take_fetched()?)?;
    torque::app::window::run(WebVehicle {
        game: alone(files)?,
        frames: 0,
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

/// The vehicle demo, its state written on the page as it draws (module docs).
struct WebVehicle {
    game: VehicleDemo,
    frames: u64,
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
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let drawn = App::frame(&mut self.game, view);
        self.frames += 1;
        #[cfg(target_arch = "wasm32")]
        if self.frames % 15 == 1 {
            let map = if self.game.in_world() { MISSION } else { "" };
            web_page::mark_play(self.frames, map, self.game.figure().map(|f| f.position));
        }
        drawn
    }

    /// The map is loading until the client is in the world and holds its figure.
    fn loading(&self) -> bool {
        self.game.server.is_some() && (!self.game.in_world() || self.game.figure().is_none())
    }
}
