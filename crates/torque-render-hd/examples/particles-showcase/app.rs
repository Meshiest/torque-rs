//! The window: a row of tabs along the top switches between the shows (the number keys and Page Up and Page Down too), Space nudges the one that has a
//! key (the circle traces again, the railgun fires, a shell goes up), the mouse orbits the camera about the show's subject
//! (drag; the wheel moves in and out; R puts it back), Escape closes. A corner label names the show with its particles alive
//! and the frame's wall time. Switching tears the show down and builds the next (a moment while its pipelines compile).

use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use torque_gui::draw::GuiDraws;
use torque_render_core::window::{SURFACE_FORMAT, configure_surface_vsync, window_gpu};
use torque_render_core::{Gpu, GuiPass, wgpu};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::font;
use crate::show::{SHOWS, Show, View};

/// The tabs' row: its height and the text scale, pixels.
const TAB_HEIGHT: i32 = 30;
const TEXT_SCALE: u32 = 2;
/// The first frame's simulation step (ms; no frame before it to time), and the longest: a slow frame stretches rather than
/// stalls.
const FIRST_STEP_MS: f32 = 16.0;
const MAX_STEP_MS: f32 = 50.0;

/// Runs the window until it closes.
pub fn run() -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = App::default();
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.error.map_or(Ok(()), Err)
}

/// The camera about the show's subject.
struct Orbit {
    pivot: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    fov: u8,
}

impl Orbit {
    fn of(view: View) -> Orbit {
        let pivot = Vec3::from_array(view.target);
        let d = Vec3::from_array(view.eye) - pivot;
        let distance = d.length().max(0.5);
        Orbit {
            pivot,
            yaw: d.x.atan2(-d.y),
            pitch: (d.z / distance).clamp(-1.0, 1.0).asin(),
            distance,
            fov: view.fov,
        }
    }

    fn position(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.pivot + Vec3::new(sy * cp, -cy * cp, sp) * self.distance
    }

    fn eye(&self) -> Eye {
        let mut eye = look_at(self.position().to_array(), self.pivot.to_array());
        eye.fov = Some(self.fov);
        eye
    }

    fn drag(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * 0.005;
        self.pitch = (self.pitch + dy * 0.005).clamp(-0.3, 1.5);
    }

    fn zoom(&mut self, steps: f32) {
        self.distance = (self.distance * 0.9f32.powf(steps)).clamp(0.5, 200.0);
    }
}

/// The window's device, surface and GUI pass.
struct Gfx {
    gpu: Gpu,
    surface: wgpu::Surface<'static>,
    size: (u32, u32),
    gui: GuiPass,
}

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    tab: usize,
    show: Option<Show>,
    orbit: Option<Orbit>,
    /// The cursor, and whether the left button holds it.
    cursor: (f32, f32),
    dragging: bool,
    clock: Option<Instant>,
    /// Simulation time owed (ms), the frame's wall time smoothed (ms), the particles alive and when they were last asked.
    owed_ms: f32,
    frame_ms: f32,
    live: u32,
    frames: u32,
    error: Option<String>,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, message: String) {
        eprintln!("{message}");
        self.error = Some(message);
        event_loop.exit();
    }

    fn open(&mut self, tab: usize) -> Result<(), String> {
        let Some(gfx) = &self.gfx else { return Ok(()) };
        self.show = None;
        let show = Show::new(&gfx.gpu, SHOWS[tab].0, gfx.size)?;
        self.orbit = Some(Orbit::of(show.view));
        self.show = Some(show);
        self.tab = tab;
        self.owed_ms = 0.0;
        self.live = 0;
        Ok(())
    }

    /// The tabs' rectangles (left, top, right, bottom) in the shows' order; a tab that would pass `width` starts the next
    /// row.
    fn tabs(width: f32) -> Vec<[f32; 4]> {
        let (mut x, mut row) = (8.0, 0.0);
        SHOWS
            .iter()
            .map(|s| {
                let w = font::width(s.1, TEXT_SCALE) as f32 + 24.0;
                if x + w > width - 8.0 {
                    (x, row) = (8.0, row + 1.0);
                }
                let top = row * TAB_HEIGHT as f32;
                let r = [x, top + 4.0, x + w, top + (TAB_HEIGHT - 2) as f32];
                x += w + 4.0;
                r
            })
            .collect()
    }

    fn gui(&self, size: (u32, u32)) -> GuiDraws {
        let mut draws = GuiDraws {
            size,
            ..GuiDraws::default()
        };
        let tabs = Self::tabs(size.0 as f32);
        let bar = tabs.last().map_or(TAB_HEIGHT as f32, |r| r[3] + 2.0);
        font::rect(
            &mut draws,
            [0.0, 0.0, size.0 as f32, bar],
            [12, 14, 20, 200],
        );
        for (k, (r, s)) in tabs.into_iter().zip(SHOWS).enumerate() {
            let active = k == self.tab;
            font::rect(
                &mut draws,
                r,
                if active {
                    [70, 110, 190, 255]
                } else {
                    [40, 44, 56, 255]
                },
            );
            let x = r[0] + 12.0;
            let y = r[1] as i32 - 4 + (TAB_HEIGHT - font::ROWS as i32 * TEXT_SCALE as i32) / 2;
            font::text(
                &mut draws,
                x as i32,
                y,
                TEXT_SCALE,
                s.1,
                if active {
                    [255; 4]
                } else {
                    [190, 195, 205, 255]
                },
            );
        }
        let label = format!(
            "{}  {} particles  {:.1} ms a frame",
            SHOWS[self.tab].1, self.live, self.frame_ms
        );
        let y = size.1 as i32 - 10 - (font::ROWS * TEXT_SCALE) as i32;
        font::text(&mut draws, 11, y + 1, TEXT_SCALE, &label, [0, 0, 0, 160]);
        font::text(&mut draws, 10, y, TEXT_SCALE, &label, [230, 235, 245, 255]);
        draws
    }

    fn redraw(&mut self) -> Result<(), String> {
        let now = Instant::now();
        let dt_ms = self.clock.map_or(FIRST_STEP_MS, |c| {
            now.duration_since(c).as_secs_f32() * 1000.0
        });
        self.clock = Some(now);
        self.frame_ms += (dt_ms - self.frame_ms) * 0.1;
        // One step a frame of the real frame time, in whole ms (the fraction carried to the next frame), capped: the
        // simulation moves with the picture however the display is timed (fixed 16 ms quanta against a 16.7 ms frame doubled
        // a step every twelfth frame, and skipped every other frame at 144 Hz). The recorder keeps its own fixed steps.
        self.owed_ms = (self.owed_ms + dt_ms).min(MAX_STEP_MS);
        let ms = self.owed_ms.round().max(1.0);
        self.owed_ms -= ms;
        let steps = [ms as u32];
        let (Some(gfx), Some(show), Some(orbit)) = (&mut self.gfx, &mut self.show, &self.orbit)
        else {
            return Ok(());
        };
        let frame = match gfx.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            _ => {
                configure_surface_vsync(&gfx.gpu, &gfx.surface, gfx.size.0, gfx.size.1, true);
                return Ok(());
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        show.frame(&steps, Some(orbit.eye()), (&view, SURFACE_FORMAT))?;
        self.frames += 1;
        if self.frames.is_multiple_of(30) {
            self.live = show.live()?;
        }
        let size = gfx.size;
        let draws = self.gui(size);
        let gfx = self.gfx.as_mut().expect("the window's device");
        gfx.gui.draw(&gfx.gpu, &draws, &view, None);
        gfx.gpu.present(frame);
        Ok(())
    }

    fn key(&mut self, code: KeyCode, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let tab = match code {
            KeyCode::Digit1 => Some(0),
            KeyCode::Digit2 => Some(1),
            KeyCode::Digit3 => Some(2),
            KeyCode::Digit4 => Some(3),
            KeyCode::Digit5 => Some(4),
            KeyCode::Digit6 => Some(5),
            KeyCode::Digit7 => Some(6),
            KeyCode::Digit8 => Some(7),
            KeyCode::Digit9 => Some(8),
            KeyCode::Digit0 => Some(9),
            _ => None,
        };
        match (code, tab) {
            (_, Some(tab)) if tab < SHOWS.len() => self.open(tab)?,
            (KeyCode::PageDown, _) => self.open((self.tab + 1) % SHOWS.len())?,
            (KeyCode::PageUp, _) => self.open((self.tab + SHOWS.len() - 1) % SHOWS.len())?,
            (KeyCode::Space, _) => {
                if let Some(show) = &mut self.show {
                    show.trigger();
                }
            }
            (KeyCode::KeyR, _) => {
                if let Some(show) = &self.show {
                    self.orbit = Some(Orbit::of(show.view));
                }
            }
            (KeyCode::Escape, _) => event_loop.exit(),
            _ => {}
        }
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Particles showcase")
            .with_inner_size(PhysicalSize::new(1280u32, 720u32));
        let window = match event_loop.create_window(attributes) {
            Ok(w) => Arc::new(w),
            Err(e) => return self.fail(event_loop, format!("no window: {e}")),
        };
        let (gpu, surface, _) = match window_gpu(window.clone()) {
            Ok(g) => g,
            Err(e) => return self.fail(event_loop, format!("no device: {e}")),
        };
        let size = window.inner_size();
        let size = (size.width.max(1), size.height.max(1));
        configure_surface_vsync(&gpu, &surface, size.0, size.1, true);
        let gui = GuiPass::new(&gpu, None);
        self.gfx = Some(Gfx {
            gpu,
            surface,
            size,
            gui,
        });
        self.window = Some(window);
        if let Err(e) = self.open(0) {
            self.fail(event_loop, e);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let outcome = match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                Ok(())
            }
            WindowEvent::Resized(size) => {
                if let Some(gfx) = &mut self.gfx {
                    gfx.size = (size.width.max(1), size.height.max(1));
                    configure_surface_vsync(&gfx.gpu, &gfx.surface, gfx.size.0, gfx.size.1, true);
                    if let Some(show) = &mut self.show {
                        show.resize(gfx.size);
                    }
                }
                Ok(())
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: ElementState::Pressed,
                        repeat: false,
                        ..
                    },
                ..
            } => self.key(code, event_loop),
            WindowEvent::CursorMoved { position, .. } => {
                let at = (position.x as f32, position.y as f32);
                if self.dragging
                    && let Some(orbit) = &mut self.orbit
                {
                    orbit.drag(at.0 - self.cursor.0, at.1 - self.cursor.1);
                }
                self.cursor = at;
                Ok(())
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let pressed = state == ElementState::Pressed;
                let width = self.gfx.as_ref().map_or(0.0, |g| g.size.0 as f32);
                let hit = pressed
                    .then(|| {
                        Self::tabs(width).iter().position(|r| {
                            self.cursor.0 >= r[0]
                                && self.cursor.0 < r[2]
                                && self.cursor.1 >= r[1]
                                && self.cursor.1 < r[3]
                        })
                    })
                    .flatten();
                match hit {
                    Some(tab) => self.open(tab),
                    None => {
                        self.dragging = pressed;
                        Ok(())
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                if let Some(orbit) = &mut self.orbit {
                    orbit.zoom(steps);
                }
                Ok(())
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => Ok(()),
        };
        if let Err(e) = outcome {
            self.fail(event_loop, e);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
