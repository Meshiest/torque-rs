//! A game in a window: [`run`] drives an [`App`] through the client's window driver ([`crate::client::platform::window`]),
//! drawing each frame into a [`WindowSurface`] (a window's GPU device and surface, kept to the window's size).

use std::sync::Arc;

use torque_render_core::window::{WINDOW_BACKENDS, configure_surface_vsync, window_gpu};
use torque_render_core::{Gpu, RenderError, wgpu};
use winit::window::Window;

use crate::client::conn::tge::TgeConn;
use crate::client::platform::window::{self as driver, WindowGame};
use crate::client::screen::{ScreenRules, TgeScreen};
use crate::server::server::Server;

use super::view::WorldView;
use super::{App, Host};

/// An offscreen device (pictures and tests) on the first of the engine's window backends ([`WINDOW_BACKENDS`]: Vulkan first)
/// with an adapter, so the pictures are drawn as the windows draw them.
pub fn offscreen_gpu() -> Result<Gpu, RenderError> {
    Gpu::with_backends(&WINDOW_BACKENDS)
}

pub struct WindowSurface {
    /// The surface draws into the window; kept with it.
    _window: Arc<Window>,
    pub gpu: Gpu,
    surface: wgpu::Surface<'static>,
    size: (u32, u32),
}

impl WindowSurface {
    /// The device and surface of `window` on the engine's window backend order ([`window_gpu`]: Vulkan first, `WGPU_BACKEND`
    /// ahead of it), its client area `size`, frames shown at the display's refresh.
    pub fn new(window: &Arc<Window>, size: (u32, u32)) -> Result<WindowSurface, String> {
        let (gpu, surface, _adapter) = window_gpu(window.clone()).map_err(|e| e.to_string())?;
        configure_surface_vsync(&gpu, &surface, size.0, size.1, true);
        Ok(WindowSurface {
            _window: window.clone(),
            gpu,
            surface,
            size,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.size = size;
        configure_surface_vsync(&self.gpu, &self.surface, size.0, size.1, true);
    }

    /// The next frame to draw into, or None when the surface has none now (it is configured again for the next try).
    pub fn frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
            _ => {
                self.resize(self.size);
                None
            }
        }
    }

    /// Shows a frame drawn into.
    pub fn present(&self, frame: wgpu::SurfaceTexture) {
        self.gpu.present(frame);
    }
}

/// An [`App`] in a window: its surface and world view once the window is made.
struct AppWindow<G> {
    game: G,
    surface: Option<WindowSurface>,
    view: Option<WorldView>,
}

impl<G: App> WindowGame for AppWindow<G> {
    type Conn = TgeConn;
    type Server = Server;

    fn host(&mut self) -> &mut Host {
        self.game.host()
    }

    fn host_ref(&self) -> &Host {
        self.game.host_ref()
    }

    fn title(&self) -> String {
        self.game.title()
    }

    fn screen_rules(&self) -> &dyn ScreenRules {
        &TgeScreen
    }

    fn window_made(&mut self, window: &Arc<Window>, size: (u32, u32)) -> Result<(), String> {
        let surface = WindowSurface::new(window, size)?;
        println!(
            "{}: {}x{} on {}",
            self.game.log_name(),
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
        self.game.advance(ms);
    }

    fn draw(&mut self) -> Result<Option<bool>, String> {
        let (Some(surface), Some(view)) = (self.surface.as_mut(), self.view.as_mut()) else {
            return Ok(None);
        };
        self.game.host().surface_size(surface.size());
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
        println!("{}: {line}", self.game.log_name());
    }
}

/// Opens the window and runs `game` until it is closed or quits.
pub fn run<G: App>(game: G) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::new().map_err(|e| e.to_string())?;
    let w = AppWindow {
        game,
        surface: None,
        view: None,
    };
    driver::run(w, event_loop).map(|_| ()).map_err(|(_, e)| e)
}
