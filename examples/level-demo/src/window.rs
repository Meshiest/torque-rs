//! The example in a window, through the client's window driver ([`torque::client::platform::window`]).

use std::sync::Arc;

use torque::client::conn::tge::TgeConn;
use torque::client::host::ClientHost;
use torque::client::platform::window::{self, WindowGame};
use torque::client::screen::{ScreenRules, TgeScreen};
use torque::server::server::Server;
use torque_example_common::view::WorldView;
use torque_example_common::window::WindowSurface;
use torque_render_core::wgpu;
use winit::window::Window;

use crate::client::LevelDemo;

pub struct LevelDemoWindow {
    pub game: LevelDemo,
    surface: Option<WindowSurface>,
    view: Option<WorldView>,
}

impl LevelDemoWindow {
    pub fn new(game: LevelDemo) -> LevelDemoWindow {
        LevelDemoWindow {
            game,
            surface: None,
            view: None,
        }
    }
}

impl WindowGame for LevelDemoWindow {
    type Conn = TgeConn;
    type Server = Server;

    fn host(&mut self) -> &mut ClientHost<TgeConn, Server> {
        &mut self.game.host
    }

    fn host_ref(&self) -> &ClientHost<TgeConn, Server> {
        &self.game.host
    }

    fn title(&self) -> String {
        "torque-rs level demo".to_string()
    }

    fn screen_rules(&self) -> &dyn ScreenRules {
        &TgeScreen
    }

    fn window_made(&mut self, window: &Arc<Window>, size: (u32, u32)) -> Result<(), String> {
        let surface = WindowSurface::new(window, size)?;
        println!(
            "example-level-demo: {}x{} on {}",
            size.0,
            size.1,
            surface.gpu.adapter()
        );
        self.view = Some(self.game.view(&surface.gpu));
        self.surface = Some(surface);
        Ok(())
    }

    fn resized(&mut self, size: (u32, u32)) {
        if let Some(s) = self.surface.as_mut() {
            s.resize(size);
        }
    }

    fn step(&mut self, ms: u32) {
        self.game.step(ms);
        self.game.add_frame_time(ms as f32 / 1000.0);
    }

    fn draw(&mut self) -> Result<Option<bool>, String> {
        let (Some(surface), Some(view)) = (self.surface.as_mut(), self.view.as_mut()) else {
            return Ok(None);
        };
        self.game.host.surface_size(surface.size());
        let (gui, eye) = self.game.frame(view);
        let Some(frame) = surface.frame() else {
            return Ok(None);
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let drew = view.draw(&surface.gpu, &target, &gui, eye)?;
        surface.present(frame);
        Ok(Some(drew))
    }

    fn vsync(&self) -> bool {
        true
    }

    fn note(&mut self, line: &str) {
        println!("example-level-demo: {line}");
    }
}

/// Opens the window and runs the example until it is closed or quits.
pub fn run(game: LevelDemo) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::new().map_err(|e| e.to_string())?;
    window::run(LevelDemoWindow::new(game), event_loop)
        .map(|_| ())
        .map_err(|(_, e)| e)
}
