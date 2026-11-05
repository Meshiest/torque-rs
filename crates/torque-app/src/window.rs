//! A game in a window: [`run`] drives an [`App`] through the client's window driver ([`torque_client::platform::window`]),
//! drawing each frame into a [`WindowSurface`] (a window's GPU device and surface, kept to the window's size).

use std::sync::Arc;

use torque_render_core::window::{WINDOW_BACKENDS, configure_surface_vsync, window_gpu};
use torque_render_core::{Gpu, RenderError, wgpu};
use winit::window::Window;

use torque_client::conn::tge::TgeConn;
use torque_client::platform::window::{self as driver, WindowGame};
use torque_client::screen::{ScreenRules, TgeScreen};
use torque_server::server::Server;

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
        Ok(WindowSurface::of(window, gpu, surface, size))
    }

    /// The surface of `window` on `gpu`, configured for `size`.
    fn of(
        window: &Arc<Window>,
        gpu: Gpu,
        surface: wgpu::Surface<'static>,
        size: (u32, u32),
    ) -> WindowSurface {
        configure_surface_vsync(&gpu, &surface, size.0, size.1, true);
        WindowSurface {
            _window: window.clone(),
            gpu,
            surface,
            size,
        }
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
    /// In a browser: the canvas's device, asked for when the window is made and answered later ([`WebDevice`]).
    #[cfg(target_arch = "wasm32")]
    web: Option<WebDevice>,
    /// In a browser: the loading stage last written on the page and the frame times ([`crate::web_page`]).
    #[cfg(target_arch = "wasm32")]
    stage: &'static str,
    #[cfg(target_arch = "wasm32")]
    frame_times: crate::web_page::FrameTimes,
}

/// A canvas's device on its way: the browser answers WebGPU's requests on its own event loop, so the window keeps running (and
/// draws nothing) until it is here.
#[cfg(target_arch = "wasm32")]
struct WebDevice {
    window: Arc<Window>,
    size: (u32, u32),
    answer: WebAnswer,
}

/// The browser's answer to a canvas's device request, once it has come.
#[cfg(target_arch = "wasm32")]
type WebAnswer =
    std::rc::Rc<std::cell::RefCell<Option<Result<(Gpu, wgpu::Surface<'static>), String>>>>;

#[cfg(target_arch = "wasm32")]
impl WebDevice {
    fn ask(window: &Arc<Window>, size: (u32, u32)) -> WebDevice {
        let answer: WebAnswer = std::rc::Rc::default();
        let slot = std::rc::Rc::clone(&answer);
        let target = window.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let made = torque_render_core::window::web_gpu(target)
                .await
                .map(|(gpu, surface, _adapter)| (gpu, surface))
                .map_err(|e| e.to_string());
            *slot.borrow_mut() = Some(made);
        });
        WebDevice {
            window: window.clone(),
            size,
            answer,
        }
    }
}

impl<G: App> AppWindow<G> {
    /// The window's surface and the game's view on its device.
    fn made(&mut self, surface: WindowSurface) {
        let size = surface.size();
        println!(
            "{}: {}x{} on {}",
            self.game.log_name(),
            size.0,
            size.1,
            surface.gpu.adapter()
        );
        self.view = Some(self.game.view(&surface.gpu));
        self.surface = Some(surface);
    }

    /// In a browser: the loading stage `stage` on the page, when it is a new one.
    #[cfg(target_arch = "wasm32")]
    fn stage(&mut self, stage: &'static str) {
        if self.stage != stage {
            self.stage = stage;
            crate::web_page::mark("stage", stage);
        }
    }

    /// In a browser: the canvas's device once it is here (Err: the browser has none to give).
    #[cfg(target_arch = "wasm32")]
    fn web_device(&mut self) -> Result<(), String> {
        let Some(web) = self.web.as_ref() else {
            return Ok(());
        };
        let Some(answer) = web.answer.borrow_mut().take() else {
            return Ok(());
        };
        let web = self.web.take().ok_or("no device asked for")?;
        let (gpu, surface) = answer.map_err(|e| format!("WebGPU: {e}"))?;
        let surface = WindowSurface::of(&web.window, gpu, surface, web.size);
        self.made(surface);
        Ok(())
    }
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
        #[cfg(target_arch = "wasm32")]
        {
            self.web = Some(WebDevice::ask(window, size));
            self.stage("gpu");
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let surface = WindowSurface::new(window, size)?;
            self.made(surface);
            Ok(())
        }
    }

    fn resized(&mut self, size: (u32, u32)) {
        #[cfg(target_arch = "wasm32")]
        if let Some(web) = self.web.as_mut() {
            web.size = size;
        }
        if let Some(s) = self.surface.as_mut() {
            s.resize(size);
        }
    }

    fn step(&mut self, ms: u32) {
        self.game.advance(ms);
    }

    fn draw(&mut self) -> Result<Option<bool>, String> {
        #[cfg(target_arch = "wasm32")]
        self.web_device()?;
        let (Some(surface), Some(view)) = (self.surface.as_mut(), self.view.as_mut()) else {
            return Ok(None);
        };
        self.game.host().surface_size(surface.size());
        // The page's settings: the quality it asked for, from this frame on.
        #[cfg(target_arch = "wasm32")]
        if let Some(quality) = crate::web_page::take_quality() {
            println!("{}: the {} quality", self.game.log_name(), quality.name());
            view.set_quality(quality);
        }
        let (gui, eye) = self.game.frame(view);
        let Some(frame) = surface.frame() else {
            return Ok(None);
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let drew = view.draw(&surface.gpu, &target, &gui, eye)?;
        surface.present(frame);
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(s) = self.frame_times.frame(web_time::Instant::now()) {
                crate::web_page::mark_frames(&s);
            }
            if self.stage != "ready" {
                self.stage(if self.game.loading() { "map" } else { "ready" });
            }
        }
        Ok(Some(drew))
    }

    fn vsync(&self) -> bool {
        true
    }

    fn note(&mut self, line: &str) {
        println!("{}: {line}", self.game.log_name());
    }
}

/// Opens the window and runs `game` until it is closed or quits. In a browser the window is a canvas on the page and this
/// returns at once: the page's event loop runs the game from then on.
pub fn run<G: App + 'static>(game: G) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::new().map_err(|e| e.to_string())?;
    let w = AppWindow {
        game,
        surface: None,
        view: None,
        #[cfg(target_arch = "wasm32")]
        web: None,
        #[cfg(target_arch = "wasm32")]
        stage: "",
        #[cfg(target_arch = "wasm32")]
        frame_times: crate::web_page::FrameTimes::new(),
    };
    #[cfg(target_arch = "wasm32")]
    {
        driver::spawn(w, event_loop);
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    driver::run(w, event_loop).map(|_| ()).map_err(|(_, e)| e)
}
