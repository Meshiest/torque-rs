//! A client's window (feature `window`): the winit event loop over a [`ClientHost`] ([`run`]). The window's client area is the
//! canvas (resized with it); the mouse, its buttons and wheel and the keys are the Windows messages TGE's platform layer reads
//! ([`super::keymap`]), posted to the host; while the game has the mouse the window holds the system cursor
//! ([`super::cursor_lock`]) and the mouse's own counts are the game's axes. Over the menus the canvas draws its own cursor; while
//! it draws none (no GuiCursor set, or its bitmap missing) the system's is shown instead ([`ClientHost::system_cursor`]).
//! `setScreenMode` ([`crate::screen`]) resizes the window, centered on its monitor, or makes it full screen (the monitor's own mode
//! of that size when it has one, else borderless), as the start-up does for `$pref::Video::fullScreen`; the canvas follows the
//! client size as on any resize. The clock follows the wall (at most [`MAX_FRAME_MS`] a frame), and while no world is drawn the
//! frames are capped ([`super::frame_cap`]).
//!
//! What the window draws, and with what, is the game's ([`WindowGame`]): it makes its GPU state when the window is made, draws
//! each frame and runs the host's clock (so it can time it).
//!
//! Credit: the window's half of the screen modes and the cursor's hold follow TGE's Windows platform layer as Torque3D (MIT)
//! keeps it, `platformWin32/winWindow.cpp` (Copyright (c) 2012 GarageGames, LLC). The code is this crate's own.

use std::sync::Arc;
use web_time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, CursorIcon, Fullscreen, Window, WindowId};

use super::{cursor_lock, frame_cap, keymap};
use crate::conn::ConnGame;
use crate::host::{ClientHost, CursorShape, GameServer};
use crate::input::{Key, MouseButton};
use crate::screen::{Monitor, ScreenMode, ScreenRules};

/// The most simulation time one frame runs (a stall is not caught up beyond it).
pub const MAX_FRAME_MS: u32 = 250;

/// The index in `modes` (`(width, height, bits, millihertz)`, a monitor's video modes) of the one full screen `mode` takes: its size
/// and depth, the refresh nearest its rate (the first of equals); None when the monitor has no mode of that size and depth.
pub fn exclusive_mode(mode: &ScreenMode, modes: &[(u32, u32, u16, u32)]) -> Option<usize> {
    let want = u64::from(mode.hz) * 1000;
    modes
        .iter()
        .enumerate()
        .filter(|(_, m)| m.0 == mode.width && m.1 == mode.height && u32::from(m.2) == mode.bpp)
        .min_by_key(|(_, m)| u64::from(m.3).abs_diff(want))
        .map(|(i, _)| i)
}

/// Puts `window` in `mode` (`OpenGLDevice::setScreenMode`'s window half); the line saying what it did.
pub fn apply_screen_mode(window: &Window, mode: &ScreenMode) -> String {
    let monitor = window.current_monitor();
    if mode.full_screen {
        let handles: Vec<_> = monitor.iter().flat_map(|m| m.video_modes()).collect();
        let modes: Vec<_> = handles
            .iter()
            .map(|v| {
                (
                    v.size().width,
                    v.size().height,
                    v.bit_depth(),
                    v.refresh_rate_millihertz(),
                )
            })
            .collect();
        let full = match exclusive_mode(mode, &modes) {
            Some(i) => Fullscreen::Exclusive(handles[i].clone()),
            None => Fullscreen::Borderless(monitor.clone()),
        };
        window.set_fullscreen(Some(full));
        return format!("full screen {}x{}", mode.width, mode.height);
    }
    window.set_fullscreen(None);
    window.set_decorations(!mode.borderless);
    let inner = PhysicalSize::new(mode.width.max(1), mode.height.max(1));
    let _ = window.request_inner_size(inner);
    if let Some(m) = monitor {
        // Centred, as TGE's `SetWindowPos` puts it: the frame is what the window has now beyond its client area.
        let (now_outer, now_inner) = (window.outer_size(), window.inner_size());
        let frame = (
            now_outer.width.saturating_sub(now_inner.width),
            now_outer.height.saturating_sub(now_inner.height),
        );
        let outer = (inner.width + frame.0, inner.height + frame.1);
        let (ms, mp) = (m.size(), m.position());
        let x = mp.x + (i64::from(ms.width) - i64::from(outer.0)).max(0) as i32 / 2;
        let y = mp.y + (i64::from(ms.height) - i64::from(outer.1)).max(0) as i32 / 2;
        window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
    }
    format!("window {}x{}", mode.width, mode.height)
}

/// What the host is told of `monitor` ([`Monitor`]): its size and its video modes' sizes.
pub fn monitor_info(monitor: &winit::monitor::MonitorHandle) -> Monitor {
    let size = monitor.size();
    let mut video_modes: Vec<(u32, u32)> = monitor
        .video_modes()
        .map(|v| (v.size().width, v.size().height))
        .collect();
    video_modes.sort_unstable();
    video_modes.dedup();
    Monitor {
        size: (size.width, size.height),
        video_modes,
    }
}

/// The window's first client size: the canvas's (the saved `$pref::Video::resolution`'s, or the default when no monitor is
/// known), or on a fresh profile the monitor's start size under `rules` ([`Monitor::start_size`]).
pub fn start_size(
    fresh: bool,
    canvas: (u32, u32),
    monitor: Option<&Monitor>,
    rules: &dyn ScreenRules,
) -> (u32, u32) {
    match monitor {
        Some(m) if fresh => m.start_size(rules),
        _ => canvas,
    }
}

/// Does `action` to the system cursor over `window` (see [`cursor_lock`]); `system`: whether the system cursor stands in for
/// the canvas's ([`ClientHost::system_cursor`]). Returns whether the system cursor is shown now. A window that is not focused
/// and `visible` never clips or moves the cursor ([`cursor_lock::guard`]), whatever `action` says.
pub fn hold_cursor(
    window: &Window,
    action: cursor_lock::Action,
    system: bool,
    visible: bool,
) -> bool {
    use cursor_lock::Action;
    // a browser reports its focus through events only (the lock's); elsewhere the window itself is asked.
    let focused = cfg!(target_arch = "wasm32") || window.has_focus();
    let action = cursor_lock::guard(action, focused, visible);
    let size = window.inner_size();
    let centre =
        winit::dpi::PhysicalPosition::new(f64::from(size.width / 2), f64::from(size.height / 2));
    let lock = |w: &Window| {
        // winit's lock clips the cursor to where it is (the centre); a platform without one confines it to the window.
        let _ = w.set_cursor_position(centre);
        if w.set_cursor_grab(CursorGrabMode::Locked).is_err() {
            let _ = w.set_cursor_grab(CursorGrabMode::Confined);
        }
    };
    match action {
        Action::Hold { .. } | Action::Centre { renew: true } => lock(window),
        Action::Centre { renew: false } => {
            let _ = window.set_cursor_position(centre);
        }
        Action::Free { restore, show } => {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            if let Some((x, y)) = restore {
                let _ = window.set_cursor_position(winit::dpi::PhysicalPosition::new(
                    f64::from(x),
                    f64::from(y),
                ));
            }
            // a window that lost the focus (or a Windows key's overlay) shows the system cursor, as TGE's `setMouseClipping`.
            if show {
                window.set_cursor_visible(true);
                return true;
            }
        }
    }
    // TGE hides the system cursor over its window whatever the hold (the canvas draws its own), unless the canvas draws none.
    window.set_cursor_visible(system);
    system
}

/// What a game gives the window: its client host, its GPU state and its frames.
pub trait WindowGame {
    /// The connection's game half and the server host of the client host.
    type Conn: ConnGame;
    type Server: GameServer;

    /// The client host the window's events go to.
    fn host(&mut self) -> &mut ClientHost<Self::Conn, Self::Server>;
    fn host_ref(&self) -> &ClientHost<Self::Conn, Self::Server>;

    /// The window's title.
    fn title(&self) -> String;

    /// The rules a fresh profile's first size follows ([`Monitor::start_size`]).
    fn screen_rules(&self) -> &dyn ScreenRules;

    /// Whether the profile has no saved resolution (the window then starts at the monitor's start size).
    fn fresh_resolution(&self) -> bool {
        false
    }

    /// The window is made, its client area `size`: the game makes its surface and renderers. An error ends the loop.
    fn window_made(&mut self, window: &Arc<Window>, size: (u32, u32)) -> Result<(), String>;

    /// The window's client area is `size` now (the surface follows it). The host's canvas is told after.
    fn resized(&mut self, size: (u32, u32));

    /// One pass of the clock: `ms` of simulation time ([`ClientHost::run`], or a game's own pass over it).
    fn step(&mut self, ms: u32);

    /// The pass's upkeep after the step and before the cursor and the screen mode are looked at.
    fn upkeep(&mut self) {}

    /// Draws a frame: None when none was presented, else whether it drew a world (the frames are capped while none is).
    fn draw(&mut self) -> Result<Option<bool>, String>;

    /// The frame cap's pref ([`frame_cap::cap_of`]; None: the display's refresh).
    fn frame_cap_pref(&self) -> Option<&str> {
        None
    }

    /// Whether frames wait for the display's refresh.
    fn vsync(&self) -> bool;

    /// Whether the window may hold the cursor at all.
    fn may_grab(&self) -> bool {
        true
    }

    /// The window's wheel's part of a notch not yet posted as a whole one (for smooth scrolling).
    fn wheel_part(&mut self, part: f32) {
        let _ = part;
    }

    /// Wall time the loop spent in a pass (not the frames: [`WindowGame::draw`] times those itself).
    fn pass_work(&mut self, work: Duration) {
        let _ = work;
    }

    /// A line about the window (its size, mode, cap).
    fn note(&mut self, line: &str) {
        println!("window: {line}");
    }
}

/// The window's loop state over the game `G`.
struct WindowApp<G: WindowGame> {
    game: G,
    window: Option<Arc<Window>>,
    last: Instant,
    /// Real milliseconds not yet run (the clock runs in whole milliseconds).
    owed: f64,
    error: Option<String>,
    /// The last client position the mouse was at.
    cursor: (i32, i32),
    /// Whether the game holds the mouse (the canvas's cursor is off: in game, mouse look).
    grabbed: bool,
    /// The keys, buttons, wheel and focus as the messages they post ([`keymap::WindowInput`]).
    input: keymap::WindowInput,
    /// The window's hold on the system cursor while the game has the mouse.
    lock: cursor_lock::CursorLock,
    /// Whether the system cursor is shown over the window now.
    system_cursor: bool,
    /// The shape the window's system cursor was last given.
    cursor_shape: CursorShape,
    started: Instant,
    /// The frame cap while no world is drawn, the frames a second it holds now (0: none), and whether the last frame drew one.
    cap: frame_cap::FrameCap,
    cap_fps: u32,
    drew_world: bool,
    /// The refresh rate of the monitor the window is on (millihertz), when it says (read as the window is made, moved or sized).
    display_mhz: Option<u32>,
    /// The monitor the host lists the modes of ([`ClientHost::set_monitor`]): read again when the window moves to another.
    monitor: Option<winit::monitor::MonitorHandle>,
}

/// Runs `game`'s window on `event_loop` until it is closed or a script quits; gives the game back for its end.
pub fn run<G: WindowGame>(game: G, event_loop: EventLoop<()>) -> Result<G, (G, String)> {
    let mut app = window_app(game, &event_loop);
    let ran = event_loop.run_app(&mut app);
    let error = match ran {
        Err(e) => Some(format!("the event loop: {e}")),
        Ok(()) => app.error,
    };
    match error {
        Some(e) => Err((app.game, e)),
        None => Ok(app.game),
    }
}

/// [`run`] in a browser: the page's event loop drives `game`'s canvas from now on, and this returns at once (the game is
/// never given back; an error ends the loop and is logged).
#[cfg(target_arch = "wasm32")]
pub fn spawn<G: WindowGame + 'static>(game: G, event_loop: EventLoop<()>) {
    use winit::platform::web::EventLoopExtWebSys as _;
    let app = window_app(game, &event_loop);
    event_loop.spawn_app(app);
}

fn window_app<G: WindowGame>(game: G, event_loop: &EventLoop<()>) -> WindowApp<G> {
    event_loop.set_control_flow(ControlFlow::Poll);
    WindowApp {
        game,
        window: None,
        last: Instant::now(),
        owed: 0.0,
        error: None,
        cursor: (0, 0),
        grabbed: false,
        input: keymap::WindowInput::default(),
        lock: if cfg!(target_arch = "wasm32") {
            cursor_lock::CursorLock::click_required()
        } else {
            cursor_lock::CursorLock::new()
        },
        system_cursor: false,
        cursor_shape: CursorShape::Arrow,
        started: Instant::now(),
        cap: frame_cap::FrameCap::new(),
        cap_fps: u32::MAX,
        drew_world: false,
        display_mhz: None,
        monitor: None,
    }
}
impl<G: WindowGame> WindowApp<G> {
    /// The host's monitor is the one `window` is on (its modes read once each time the window comes to another monitor).
    fn read_monitor(&mut self, window: &Window) {
        let now = window.current_monitor();
        if now.is_none() || now == self.monitor {
            return;
        }
        let info = now.as_ref().map(monitor_info);
        if let Some(m) = &info {
            let line = format!(
                "monitor {}x{}, {} video modes",
                m.size.0,
                m.size.1,
                m.video_modes.len()
            );
            self.game.note(&line);
        }
        self.game.host().set_monitor(info);
        self.monitor = now;
    }

    /// The focus lost: the buttons still held let go in the game ([`keymap::WindowInput::focus_lost`]).
    fn focus_lost(&mut self) {
        for (m, w, l) in self.input.focus_lost(self.cursor) {
            self.game.host().post(m, w, l);
        }
    }

    /// Applies a release of the cursor hold now (in the event that caused it, not at the next pass).
    fn release_cursor(&mut self, release: Option<cursor_lock::Action>) {
        if let (Some(action), Some(window)) = (release, &self.window) {
            let system = self.game.host_ref().system_cursor();
            self.system_cursor = hold_cursor(window, action, system, self.lock.visible());
        }
    }

    /// Shows or hides the system cursor as the canvas's needs it ([`ClientHost::system_cursor`]), when that changed (an
    /// unfocused window keeps the one its release showed).
    fn sync_system_cursor(&mut self, window: &Window) {
        if !window.has_focus() {
            return;
        }
        let want = self.game.host_ref().system_cursor() && !self.lock.held();
        if want != self.system_cursor {
            window.set_cursor_visible(want);
            self.system_cursor = want;
        }
        if want {
            let shape = self.game.host_ref().cursor_shape();
            if shape != self.cursor_shape {
                window.set_cursor(match shape {
                    CursorShape::Arrow => CursorIcon::Default,
                    CursorShape::Pointer => CursorIcon::Pointer,
                    CursorShape::Text => CursorIcon::Text,
                });
                self.cursor_shape = shape;
            }
        }
    }
}

impl<G: WindowGame> ApplicationHandler for WindowApp<G> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // A fresh profile starts at the monitor's start size; a saved resolution is kept.
        let canvas = self.game.host_ref().gui.0.borrow().canvas_size;
        let canvas = (canvas.0.max(1) as u32, canvas.1.max(1) as u32);
        let first = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())
            .map(|m| monitor_info(&m));
        let want = start_size(
            self.game.fresh_resolution(),
            canvas,
            first.as_ref(),
            self.game.screen_rules(),
        );
        if want != canvas {
            let line = format!(
                "a fresh profile: the window starts at {}x{}",
                want.0, want.1
            );
            self.game.note(&line);
            self.game.host().set_monitor(first);
            self.game.host().window_resized(want);
        }
        let size = self.game.host_ref().gui.0.borrow().canvas_size;
        let attrs = Window::default_attributes()
            .with_title(self.game.title())
            .with_inner_size(PhysicalSize::new(
                size.0.max(1) as u32,
                size.1.max(1) as u32,
            ));
        // In a browser the window is a canvas added to the page's body.
        #[cfg(target_arch = "wasm32")]
        let attrs = {
            use winit::platform::web::WindowAttributesExtWebSys as _;
            attrs.with_append(true)
        };
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.error = Some(format!("the window: {e}"));
                event_loop.exit();
                return;
            }
        };
        // TGE draws its own cursor (the canvas's): the system's is hidden over the window.
        window.set_cursor_visible(false);
        // The modes an options dialog lists are the window's monitor's.
        self.read_monitor(&window);
        // The start-up's mode (`Video::setDevice` with the preferences): full screen when `$pref::Video::fullScreen` says so.
        let start = self.game.host_ref().screen_mode();
        if start.full_screen {
            let line = apply_screen_mode(&window, &start);
            self.game.note(&line);
        }
        let inner = window.inner_size();
        let size = (inner.width.max(1), inner.height.max(1));
        if let Err(e) = self.game.window_made(&window, size) {
            self.error = Some(e);
            event_loop.exit();
            return;
        }
        self.game
            .host()
            .gui
            .set_canvas_size((size.0 as i32, size.1 as i32));
        self.display_mhz = window
            .current_monitor()
            .and_then(|m| m.refresh_rate_millihertz());
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Moved(_) => {
                if let Some(window) = self.window.clone() {
                    self.display_mhz = window
                        .current_monitor()
                        .and_then(|m| m.refresh_rate_millihertz());
                    self.read_monitor(&window);
                }
            }
            WindowEvent::Resized(size) => {
                self.lock.on_resize();
                if let Some(window) = &self.window {
                    self.display_mhz = window
                        .current_monitor()
                        .and_then(|m| m.refresh_rate_millihertz());
                    let size = (size.width.max(1), size.height.max(1));
                    self.game.resized(size);
                    // a size the window was given by a screen mode or by hand lays the HUD out again.
                    self.game.host().window_resized(size);
                }
            }
            // losing the focus, or being hidden or minimised, frees the cursor in the same event.
            WindowEvent::Focused(f) => {
                let release = self.lock.on_focus(f);
                self.release_cursor(release);
                if !f {
                    self.focus_lost();
                }
            }
            WindowEvent::Occluded(hidden) => {
                let release = self.lock.on_visible(!hidden);
                self.release_cursor(release);
                if hidden {
                    self.focus_lost();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                // TGE posts no mouse moves while the window holds the cursor (it is put back at the centre each pass).
                if !self.lock.held() {
                    self.cursor = (position.x as i32, position.y as i32);
                    self.game.host().mouse_move(self.cursor);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed {
                    // in a browser a click may ask for the pointer lock again (the player left it with Esc).
                    let again = self.lock.on_click();
                    self.release_cursor(again);
                }
                if let Some(button) = MouseButton::from_winit(button) {
                    let (m, w, l) =
                        self.input
                            .button(button, state == ElementState::Pressed, self.cursor);
                    self.game.host().post(m, w, l);
                }
            }
            // in the menus the window's wheel; while the game holds the mouse the device's (below), so a held, hidden cursor
            // cannot keep the inventory's wheel from the game.
            WindowEvent::MouseWheel { delta, .. } if !self.grabbed => {
                for (m, w, l) in self.input.wheel(delta) {
                    self.game.host().post(m, w, l);
                }
                // A high-resolution wheel's or a touchpad's part of a notch, for smooth scrolling.
                let part = self.input.wheel_part();
                self.game.wheel_part(part);
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Some(key) = Key::from_winit(code)
                {
                    let down = event.state == ElementState::Pressed;
                    // a Windows key lets go of the cursor before the shell's shortcut opens its overlay.
                    if down && !is_synthetic && matches!(key, Key::SuperLeft | Key::SuperRight) {
                        let release = self.lock.on_system_key();
                        self.release_cursor(release);
                    }
                    if let Some((m, w, l)) = self.input.key(key, down, event.repeat, is_synthetic) {
                        self.game.host().post(m, w, l);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let began = Instant::now();
                match self.game.draw() {
                    Ok(Some(world)) => {
                        self.drew_world = world;
                        self.cap.frame_drawn(began, self.cap_fps, world);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        self.error = Some(e);
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        // The mouse's own counts while the game holds it (in game: moveMap's yaw and pitch).
        if let DeviceEvent::MouseMotion { delta } = event
            && self.grabbed
        {
            self.game
                .host()
                .mouse_motion(delta.0 as f32, delta.1 as f32);
        }
        // the wheel while the game holds the mouse (a `moveMap`'s `mouse0 zaxis`).
        if let DeviceEvent::MouseWheel { delta } = event
            && self.grabbed
        {
            for (m, w, l) in self.input.wheel(delta) {
                self.game.host().post(m, w, l);
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let began = Instant::now();
        self.pass(event_loop);
        self.game.pass_work(began.elapsed());
    }

    /// In a browser nobody is given the error back ([`spawn`]): it is raised as a panic, which the page's console shows.
    #[cfg(target_arch = "wasm32")]
    #[allow(clippy::panic)]
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(e) = self.error.take() {
            panic!("{e}");
        }
    }
}

impl<G: WindowGame> WindowApp<G> {
    /// One pass of the window's loop: the clock's step, the upkeep, the cursor and the next frame asked for.
    fn pass(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        self.owed += now.duration_since(self.last).as_secs_f64() * 1000.0;
        self.last = now;
        let ms = (self.owed as u32).min(MAX_FRAME_MS);
        if ms > 0 {
            self.owed -= f64::from(ms);
            self.owed = self.owed.min(f64::from(MAX_FRAME_MS));
            // One pass of TGE's main loop over the frame's milliseconds: the server ticks for every 32 ms boundary its time
            // crosses (a pass cut into ticks of its own would run no tick for a frame under 32 ms).
            self.game.step(ms);
        }
        if self.game.host_ref().server.vm().quit_requested() {
            event_loop.exit();
            return;
        }
        self.game.upkeep();
        let wants = self.game.host_ref().wants_mouse();
        self.grabbed = wants;
        let Some(window) = self.window.clone() else {
            return;
        };
        let now_ms = self.started.elapsed().as_millis() as u64;
        let may_grab = self.game.may_grab();
        // a window made without the focus (started in the background) gets no event for it: the window is asked.
        #[cfg(not(target_arch = "wasm32"))]
        if window.has_focus() != self.lock.focused() {
            let release = self.lock.on_focus(window.has_focus());
            self.release_cursor(release);
        }
        if let Some(action) = self.lock.pass(wants, may_grab, self.cursor, now_ms) {
            let system = self.game.host_ref().system_cursor();
            self.system_cursor = hold_cursor(&window, action, system, self.lock.visible());
        }
        self.sync_system_cursor(&window);
        if let Some(mode) = self.game.host().take_screen_mode() {
            let line = apply_screen_mode(&window, &mode);
            self.game.note(&line);
        }
        // While no world is drawn the frames are capped (the display's refresh, or the pref's number): the loop sleeps until
        // the next frame's time (the server's milliseconds owed meanwhile are run when it wakes).
        let pref = self
            .game
            .frame_cap_pref()
            .and_then(|name| self.game.host_ref().server.vm().global(name))
            .unwrap_or_default();
        let cap = frame_cap::cap_of(&String::from_utf8_lossy(&pref));
        let fps = frame_cap::fps_for(cap, self.game.vsync(), self.display_mhz);
        if fps != self.cap_fps {
            self.cap_fps = fps;
            match fps {
                0 => self.game.note("the menus' frames not capped"),
                n => self
                    .game
                    .note(&format!("the menus' frames capped at {n} a second")),
            }
        }
        match self.cap.wait_until(Instant::now(), fps, self.drew_world) {
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => {
                event_loop.set_control_flow(ControlFlow::Poll);
                window.request_redraw();
            }
        }
    }
}

#[cfg(test)]
#[path = "window_tests.rs"]
mod tests;
