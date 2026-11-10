//! The example's client in a window (feature `window`): a client host ([`ClientHost`]) over a bare server host draws a small GUI
//! (a status line, and one box for each ghost the client holds, seen from above) through the client's window driver
//! ([`torque::client::platform::window`]) and the GUI pass of `torque-render-core`, while an [`ExampleClient`] keeps the ghosts an
//! example server sends it. With `local` the window runs that server too, in the same loop ([`DedicatedServer`]).
//!
//! The canvas is the window's client area; the keys and the mouse go to the GUI as on any client. The clock is the window's
//! (the host's ticks), and the example client and server tick on the same clock.

use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;

use torque::client::conn::tge::TgeConn;
use torque::client::host::{ClientHost, EnginePass};
use torque::client::platform::window::{self, WindowGame};
use torque::client::screen::{ScreenRules, TgeScreen};
use torque::client::state::ClientState;
use torque::fonts::{FontCache, NoFallback};
use torque::gui::DrawCache;
use torque::script::vm::vm::Vm;
use torque::server::server::{GameServer as _, Server};
use torque_render_core::wgpu;
use torque_render_core::window::{configure_surface_vsync, window_gpu};
use torque_render_core::{Gpu, GuiPass};
use winit::window::Window;

use crate::client::ExampleClient;
use crate::server::{DedicatedServer, ServerOptions};

/// The most ghosts the window draws a box for.
pub const MAX_BOXES: usize = 64;
/// How many world units a pixel of the map is.
pub const UNITS_PER_PIXEL: f32 = 0.25;

/// The GUI: a background, the status line, and [`MAX_BOXES`] hidden boxes.
const GUI: &str = r#"
new GuiControlProfile(ExampleBackProfile) { opaque = true; fillColor = "24 28 36 255"; };
new GuiControlProfile(ExampleTextProfile) { fontType = "Arial"; fontSize = 16; fontColor = "230 230 230 255"; };
new GuiControlProfile(ExampleBoxProfile) { opaque = true; fillColor = "240 180 60 255"; };
createCanvas("torque-example");
new GuiControl(ExampleGui) {
    profile = ExampleBackProfile; position = "0 0"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";
    new GuiTextCtrl(ExampleStatus) { profile = ExampleTextProfile; position = "8 8"; extent = "780 20"; text = "connecting"; };
};
for (%i = 0; %i < $ExampleBoxes; %i++)
    ExampleGui.add(new GuiControl("ExampleBox" @ %i) { profile = ExampleBoxProfile; extent = "6 6"; visible = false; });
Canvas.setContent(ExampleGui);
"#;

struct WindowGpu {
    /// The surface draws into the window; kept with it.
    _window: Arc<Window>,
    gpu: Gpu,
    surface: wgpu::Surface<'static>,
    pass: GuiPass,
    size: (u32, u32),
}

/// The example's window client.
pub struct ExampleWindow {
    host: ClientHost<TgeConn, Server>,
    client: ExampleClient,
    server: Option<DedicatedServer>,
    gpu: Option<WindowGpu>,
    /// The clock's milliseconds so far (the example client's and server's time).
    clock_ms: u32,
    /// The status line last shown.
    shown: String,
}

impl ExampleWindow {
    /// A window client of the example server at `server`; with `local` the window starts that server itself, on the loopback
    /// address (any port), and `server` is ignored.
    pub fn new(server: SocketAddr, local: bool) -> Result<ExampleWindow, String> {
        let (address, dedicated) = if local {
            let s =
                DedicatedServer::start(&ServerOptions::loopback(0)).map_err(|e| e.to_string())?;
            let a = s.local_addr().ok_or("the server has no address")?;
            (a, Some(s))
        } else {
            (server, None)
        };
        let client = ExampleClient::connect(address).map_err(|e| format!("{e:?}"))?;
        let mut vm = Vm::new();
        let gui = torque::gui::register(&mut vm);
        let state = Rc::new(RefCell::new(ClientState::<TgeConn>::default()));
        torque::client::actionmap::register(&mut vm, &state);
        torque::client::screen::register(&mut vm, &gui, &state);
        vm.set_global("$ExampleBoxes", &MAX_BOXES.to_string());
        vm.eval(GUI.as_bytes())
            .map_err(|e| format!("the GUI: {e}"))?;
        let fonts = DrawCache::new(Box::new(FontCache::new("fonts", Box::new(NoFallback))));
        let mut host = ClientHost::new(Server::new(vm), gui, state, Rc::new(EnginePass), fonts);
        host.set_windowed();
        Ok(ExampleWindow {
            host,
            client,
            server: dedicated,
            gpu: None,
            clock_ms: 0,
            shown: String::new(),
        })
    }

    /// The client's script engine (the GUI, the action map and the screen's natives).
    pub fn vm(&self) -> &Vm {
        &self.host.server.vm
    }

    /// The status line and the boxes as the client's ghosts are now.
    fn show_ghosts(&mut self) {
        let seen = self.client.seen();
        let status = if self.client.connected() {
            format!(
                "{} ghosts after {} packets",
                seen.len(),
                self.client.packets()
            )
        } else {
            "connecting".to_string()
        };
        let canvas = self.host.gui.0.borrow().canvas_size;
        let centre = (canvas.0 / 2, canvas.1 / 2);
        let mut script = String::new();
        if status != self.shown {
            script.push_str(&format!("ExampleStatus.setText(\"{status}\");\n"));
            self.shown = status;
        }
        for i in 0..MAX_BOXES {
            let at = seen.get(i).and_then(|g| g.position);
            match at {
                Some(p) => {
                    let x = centre.0 + (p[0] / UNITS_PER_PIXEL) as i32 - 3;
                    let y = centre.1 - (p[1] / UNITS_PER_PIXEL) as i32 - 3;
                    script.push_str(&format!(
                        "ExampleBox{i}.resize({x}, {y}, 6, 6); ExampleBox{i}.setVisible(true);\n"
                    ));
                }
                None => script.push_str(&format!("ExampleBox{i}.setVisible(false);\n")),
            }
        }
        if let Err(e) = self.host.server.vm_mut().eval(script.as_bytes()) {
            eprintln!("example-window: {e}");
        }
    }

    /// The status line shown now.
    pub fn status(&self) -> &str {
        &self.shown
    }

    /// Whether the box of the `i`-th ghost is shown, and where (the canvas's pixels).
    pub fn ghost_box(&mut self, i: usize) -> Option<(i32, i32)> {
        let vm = self.host.server.vm_mut();
        let text = vm
            .eval(
                format!("return ExampleBox{i}.isVisible() SPC ExampleBox{i}.getPosition();")
                    .as_bytes(),
            )
            .ok()?;
        let text = String::from_utf8_lossy(&text).into_owned();
        let mut words = text.split(' ');
        if words.next()? != "1" {
            return None;
        }
        Some((words.next()?.parse().ok()?, words.next()?.parse().ok()?))
    }

    /// Ends the example client's connection (and the local server).
    pub fn finish(mut self) {
        self.client.disconnect();
        self.server = None;
    }
}

impl WindowGame for ExampleWindow {
    type Conn = TgeConn;
    type Server = Server;

    fn host(&mut self) -> &mut ClientHost<TgeConn, Server> {
        &mut self.host
    }

    fn host_ref(&self) -> &ClientHost<TgeConn, Server> {
        &self.host
    }

    fn title(&self) -> String {
        "torque-example".to_string()
    }

    fn screen_rules(&self) -> &dyn ScreenRules {
        &TgeScreen
    }

    fn window_made(&mut self, window: &Arc<Window>, size: (u32, u32)) -> Result<(), String> {
        let (gpu, surface, _adapter) = window_gpu(window.clone()).map_err(|e| e.to_string())?;
        configure_surface_vsync(&gpu, &surface, size.0, size.1, true);
        println!("example-window: {}x{} on {}", size.0, size.1, gpu.adapter());
        let pass = GuiPass::new(&gpu, None);
        self.gpu = Some(WindowGpu {
            _window: window.clone(),
            gpu,
            surface,
            pass,
            size,
        });
        Ok(())
    }

    fn resized(&mut self, size: (u32, u32)) {
        if let Some(g) = self.gpu.as_mut() {
            g.size = size;
            configure_surface_vsync(&g.gpu, &g.surface, size.0, size.1, true);
        }
    }

    fn step(&mut self, ms: u32) {
        self.host.run(ms, |_| false);
        self.clock_ms = self.clock_ms.wrapping_add(ms);
        if let Some(s) = self.server.as_mut() {
            s.tick(self.clock_ms);
        }
        self.client.tick(self.clock_ms);
        for note in self.client.take_notes() {
            println!("example-window: {note:?}");
        }
        self.show_ghosts();
    }

    fn draw(&mut self) -> Result<Option<bool>, String> {
        let Some(g) = self.gpu.as_mut() else {
            return Ok(None);
        };
        self.host.surface_size(g.size);
        let draws = self.host.draw_frame();
        let frame = match g.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                configure_surface_vsync(&g.gpu, &g.surface, g.size.0, g.size.1, true);
                return Ok(None);
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        g.pass
            .draw(&g.gpu, &draws, &view, Some([0.0, 0.0, 0.0, 1.0]));
        g.gpu.present(frame);
        Ok(Some(false))
    }

    fn vsync(&self) -> bool {
        true
    }

    fn note(&mut self, line: &str) {
        println!("example-window: {line}");
    }
}

/// Opens the window and runs it until it is closed.
pub fn run(game: ExampleWindow) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::new().map_err(|e| e.to_string())?;
    match window::run(game, event_loop) {
        Ok(game) => {
            game.finish();
            Ok(())
        }
        Err((game, e)) => {
            game.finish();
            Err(e)
        }
    }
}
