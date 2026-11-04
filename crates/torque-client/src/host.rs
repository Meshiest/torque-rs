//! The client host: a game's server host (the script engine and its main loop, [`GameServer`]) with the GUI object model
//! ([`gui`]), the client natives' state ([`ClientState`]) and the platform input layer ([`input::Win32`]). No GPU, no window:
//! a window client drives it (`platform`, feature `window`), a test or a headless run calls it directly.
//!
//! **Clock.** The server host's virtual clock. Before every tick [`ClientHost::run`] does the client's own work: the game's
//! network part ([`ClientPass::net`]: its sockets, the connection's datagrams, the prediction and the moves), the HUD feed and the
//! world camera for the GUI, the queued callbacks, the game's other part ([`ClientPass::other`]: its notes, the audio), then one
//! canvas frame ([`gui::Gui::frame`]: the layout to the canvas) and its render pass (the canvas drawn, the first `onRender` of each
//! control drawn run: TGE's main loop renders after every pass; [`ClientHost::draw_each_tick`]). Callbacks run with no borrow held.
//!
//! **Input.** A window's messages go through TGE's platform layer and the action maps ([`ClientHost::post`],
//! [`ClientHost::mouse_motion`]); an injection plays a list of them on the clock ([`ClientHost::inject`]).
//!
//! **The window's parts** (a window client calls them; a headless run never does): the screen mode and the monitor
//! ([`crate::screen`]), the canvas's size, the frame rate ([`crate::fps`]), vertical sync, the display devices and the URL
//! opener.
//!
//! Credit: the order of a pass follows TGE's main loop as Torque3D (MIT) keeps it, `app/mainLoop.cpp`, and the split of the
//! socket's datagrams follows `sim/netInterface.cpp` (each Copyright (c) 2012 GarageGames, LLC). The code is this crate's
//! own.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gui::Gui;
pub use torque_server::server::GameServer;
use torque_server::server::{RunEnd, TICK_MS};
use ts_vm::vm::{Level, LogLine, Vm, VmError};

use crate::conn::ConnGame;
use crate::state::{ClientState, WebOpener};
use crate::{hud, input, net, screen};

/// The game's part of the client's work before every tick ([`ClientHost::run`]).
pub trait ClientPass<G: ConnGame> {
    /// The network: the game's sockets, the connection's datagrams, the prediction and the moves.
    fn net(&self, vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>);
    /// The rest, after the queued callbacks (timed as the client's other work).
    fn other(&self, vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>);
}

/// No work of a game's own: the socket's datagrams, as TGE's one `NetInterface` hands them out: a server query's answer to the
/// query ([`crate::query::take_datagram`]), the rest to the connection ([`crate::conn::take_datagram`]; what neither takes is
/// dropped); then the query's waiting pass when it is due ([`crate::query::tick`]) and the connection's tick
/// ([`crate::conn::tick`]). The query's script calls (`onServerQueryStatus`) run after the datagrams are read, with no borrow
/// held; an error in one is kept with the callbacks' errors.
pub struct EnginePass;

impl<G: ConnGame> ClientPass<G> for EnginePass {
    fn net(&self, vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>) {
        let socket = Rc::clone(&client.borrow().net);
        let now = vm.sim_time();
        let mut out = Vec::new();
        for _ in 0..net::MAX_DATAGRAMS_PER_TICK {
            let Some((from, data)) = socket.borrow_mut().recv() else {
                break;
            };
            if crate::query::take_datagram(client, from, &data, now, &mut out) {
                continue;
            }
            if crate::conn::take_datagram(client, from, data).is_err() {
                socket.borrow_mut().stats.stray += 1;
            }
        }
        let ran = crate::query::run_out(vm, out).and_then(|()| crate::query::tick(vm, client, now));
        if let Err(e) = ran {
            net::keep_error(client, e);
        }
        crate::conn::tick(vm, client);
    }

    fn other(&self, _vm: &mut Vm, _client: &Rc<RefCell<ClientState<G>>>) {}
}

/// Runs the callbacks queued so far (those queued while they run wait for the next call), each a run of its own.
pub fn run_queued<G: ConnGame>(vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>) {
    let calls: Vec<crate::state::Queued> = client.borrow_mut().queue.drain(..).collect();
    for call in calls {
        if !vm.objects().contains(call.object) {
            continue;
        }
        if let Err(e) = vm.call_method(call.object, &call.method, &call.args) {
            let mut state = client.borrow_mut();
            if state.errors.len() < crate::state::MAX_QUEUED {
                state.errors.push(e);
            }
        }
    }
}

/// The client host over the server host `S`, its connection's game half `G`.
pub struct ClientHost<G: ConnGame, S: GameServer> {
    pub server: S,
    pub gui: Gui,
    pub client: Rc<RefCell<ClientState<G>>>,
    /// The game's part of the work before each tick.
    pub pass: Rc<dyn ClientPass<G>>,
    /// "Unable to find object" lines drained so far ([`drain_log`](Self::drain_log)).
    pub missing_objects: Vec<String>,
    /// Errors of the callbacks [`draw`](Self::draw) ran.
    pub draw_errors: Vec<VmError>,
    /// The platform input layer (the window's or an injection's messages, [`input::Win32`]).
    pub win32: input::Win32,
    /// Errors of the scripts input events ran (at most [`MAX_MISSING_OBJECTS`]).
    pub input_errors: Vec<VmError>,
    /// The client's own draw cache: [`run`](Self::run) draws the canvas every tick as TGE's main loop renders a frame every pass
    /// (the script `onRender` a control runs on its first frame), and [`draw_frame`](Self::draw_frame) reuses it.
    pub cache: gui::DrawCache,
    /// Whether [`run`](Self::run) lays out and draws a frame every tick (on by default; a host that draws on its own may turn it off:
    /// a window that drew the frame before).
    pub draw_each_tick: bool,
    /// The wall time of the client's own work in [`run`](Self::run)'s passes; `take` it.
    pub perf: ClientPerf,
}

/// The wall time of the client's work before each pass of [`ClientHost::run`], summed.
#[derive(Debug, Clone, Default)]
pub struct ClientPerf {
    /// The connection's datagrams, prediction and moves ([`ClientPass::net`]).
    pub net: std::time::Duration,
    /// The canvas's layout pass (`Gui::frame`).
    pub layout: std::time::Duration,
    /// The canvas drawn for its `onRender` callbacks ([`ClientHost::draw_each_tick`]).
    pub draw: std::time::Duration,
    /// The rest (the HUD feed, the queued natives, [`ClientPass::other`]).
    pub other: std::time::Duration,
}

impl ClientPerf {
    /// The sums so far, cleared.
    pub fn take(&mut self) -> ClientPerf {
        std::mem::take(self)
    }
}

/// The most missing-object lines kept.
pub const MAX_MISSING_OBJECTS: usize = 1_024;

impl<G: ConnGame, S: GameServer> ClientHost<G, S> {
    /// The host over `server` with `gui`, the natives' state `client`, the game's part of each tick `pass` and the draw cache
    /// `cache` (its fonts the game's).
    pub fn new(
        server: S,
        gui: Gui,
        client: Rc<RefCell<ClientState<G>>>,
        pass: Rc<dyn ClientPass<G>>,
        cache: gui::DrawCache,
    ) -> ClientHost<G, S> {
        ClientHost {
            server,
            gui,
            client,
            pass,
            missing_objects: Vec::new(),
            draw_errors: Vec::new(),
            win32: input::Win32::default(),
            input_errors: Vec::new(),
            cache,
            draw_each_tick: true,
            perf: ClientPerf::default(),
        }
    }

    /// Runs the clock for at most `limit_ms` of simulation time (until `quit()` or `until` holds), the client's own work before every
    /// tick.
    pub fn run(&mut self, limit_ms: u32, mut until: impl FnMut(&mut S) -> bool) -> RunEnd {
        let gui = self.gui.clone();
        let client = Rc::clone(&self.client);
        let pass = Rc::clone(&self.pass);
        let draw = self.draw_each_tick;
        let mut cache = std::mem::take(&mut self.cache);
        let mut errors = std::mem::take(&mut self.draw_errors);
        let mut perf = std::mem::take(&mut self.perf);
        let end = self.server.run_ticks(limit_ms, |server| {
            let t = std::time::Instant::now();
            pass.net(server.vm_mut(), &client);
            let t_net = t.elapsed();
            gui.0.borrow_mut().world_camera = crate::conn::has_camera(&client);
            let feed = hud::feed(&client);
            gui.0.borrow_mut().hud = feed;
            run_queued(server.vm_mut(), &client);
            pass.other(server.vm_mut(), &client);
            let t_other = t.elapsed() - t_net;
            // The canvas's frame (its layout, then its draw for the callbacks), unless the host draws the frame after the pass
            // itself (`draw_each_tick` off).
            if draw {
                let t_layout = std::time::Instant::now();
                gui.frame(server.vm_mut());
                perf.layout += t_layout.elapsed();
                let t_draw = std::time::Instant::now();
                draw_callbacks(server.vm_mut(), &gui, &mut cache, &mut errors);
                perf.draw += t_draw.elapsed();
            }
            perf.net += t_net;
            perf.other += t_other;
            until(server)
        });
        self.cache = cache;
        self.draw_errors = errors;
        self.perf = perf;
        end
    }

    /// Where the client's socket may bind and send ([`net::NetPolicy`]; off by default). Set it before the start-up: its
    /// `setNetPort` binds under it.
    pub fn set_net_policy(&mut self, policy: net::NetPolicy) {
        self.client.borrow().net.borrow_mut().policy = policy;
    }

    /// The display device the client reports from now on (`getDisplayDeviceList`, `getResolutionList`, `setDisplayDevice`): the
    /// windowed client's renderer backend ("Direct3D 12", "Vulkan", "Metal", ...). A `$pref::Video::displayDevice` that names TGE's
    /// "OpenGL" is moved to it, so an options dialog shows it selected; empty puts TGE's "OpenGL" back (every headless run's).
    pub fn set_display_device(&mut self, name: &str) {
        let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
        self.client.borrow_mut().display_device = name.clone();
        let vm = self.server.vm_mut();
        let pref = vm.global("$pref::Video::displayDevice").unwrap_or_default();
        if !name.is_empty() && (pref.is_empty() || pref == screen::DISPLAY_DEVICE.as_bytes()) {
            vm.set_global("$pref::Video::displayDevice", &name);
        }
    }

    /// Every display device the window can draw with (its usable renderer backends, `name` among them), listed by
    /// `getDisplayDeviceList` and taken by `setDisplayDevice` for the next start. Call after [`ClientHost::set_display_device`].
    pub fn set_display_devices(&mut self, names: &[&str]) {
        let mut list: Vec<String> = names
            .iter()
            .map(|n| n.chars().filter(|c| !c.is_control()).take(64).collect())
            .collect();
        let current = self.client.borrow().display_device.clone();
        if !current.is_empty() && !list.contains(&current) {
            list.insert(0, current);
        }
        self.client.borrow_mut().display_devices = list;
    }

    /// The device `setDisplayDevice` chose for the next start since the last call.
    pub fn take_display_next(&mut self) -> Option<String> {
        self.client.borrow_mut().display_next.take()
    }

    /// The display device the profile asks for (`$pref::Video::displayDevice`).
    pub fn display_pref(&self) -> String {
        String::from_utf8_lossy(
            &self
                .server
                .vm()
                .global("$pref::Video::displayDevice")
                .unwrap_or_default(),
        )
        .into_owned()
    }

    /// One main loop pass's frame ([`crate::fps`]) at real time `real_ms`: the average, and `$fps::real` and
    /// `$fps::virtual` when they are due. The windowed client calls it after each frame it presents.
    pub fn fps_update(&mut self, real_ms: f64) {
        let virt = self.server.vm().sim_time() as f32 / 1000.0;
        let due = self
            .client
            .borrow_mut()
            .fps
            .update((real_ms / 1000.0) as f32, virt);
        if let Some((real, virtual_)) = due {
            let vm = self.server.vm_mut();
            vm.set_global("$fps::real", &real);
            vm.set_global("$fps::virtual", &virtual_);
        }
    }

    /// `setVerticalSync`'s request since the last call.
    pub fn take_vsync_request(&mut self) -> Option<bool> {
        self.client.borrow_mut().vsync_request.take()
    }

    /// Whether the profile asks for vertical sync: `!$pref::Video::disableVerticalSync` (as TGE's device activation does).
    pub fn vsync_pref(&self) -> bool {
        self.server
            .vm()
            .global("$pref::Video::disableVerticalSync")
            .is_none_or(|v| ts_text::atoi(&v) == 0)
    }

    /// A window takes the screen modes from here on ([`screen::ScreenState::windowed`]).
    pub fn set_windowed(&mut self) {
        self.client.borrow_mut().screen.windowed = true;
    }

    /// The monitor the window is on (the windowed client's, as the window is made and moved): `getResolutionList` lists its
    /// modes, `setScreenMode` chooses among them and `getDesktopResolution` is its size ([`screen::Display::of`]).
    /// None (every headless run) keeps the fixed list.
    pub fn set_monitor(&mut self, monitor: Option<screen::Monitor>) {
        self.client.borrow_mut().screen.monitor = monitor;
    }

    /// The window's client area is `size` (a resize the window made, or the user's). The canvas takes it; a size other
    /// than the current mode's is taken as a mode change, as TGE's device would make one (`setScreenMode`: the mode and
    /// `$pref::Video::resolution` kept, then `resetCanvas();` (the scripts' own re-layout at the new size)). TGE's own window cannot be resized by hand; this one can, and its HUD
    /// follows. A size equal to the mode is only the canvas's.
    pub fn window_resized(&mut self, size: (u32, u32)) {
        let (w, h) = (size.0.max(1), size.1.max(1));
        // The mode before the canvas takes the size: until a `setScreenMode` the mode is read from the canvas, so read after it a
        // resize always looked like the mode in use and no layout ran.
        let now = self.screen_mode();
        self.gui.set_canvas_size((w as i32, h as i32));
        if (now.width, now.height) == (w, h) {
            return;
        }
        // The canvas and its GUIs at the new size before the scripts lay them out (`resetCanvas` below reads the canvas's extent).
        self.gui.frame(self.server.vm_mut());
        let mode = screen::ScreenMode {
            width: w,
            height: h,
            ..now
        };
        self.client.borrow_mut().screen.current = Some(mode);
        let vm = self.server.vm_mut();
        vm.set_global(
            "$pref::Video::resolution",
            &format!("{} {} {} {}", mode.width, mode.height, mode.bpp, mode.hz),
        );
        if let Err(e) = vm.invoke(None, b"resetCanvas", &[]) {
            vm.error(format!("resetCanvas: {e}"));
        }
    }

    /// The window's surface is really `size` (the window says so before each frame). `setScreenMode` sizes the canvas to the mode
    /// at once, as TGE's device has resized its window by then, but the OS may not honour the window's request (a maximised or
    /// snapped window keeps its size): a canvas other than the surface is then laid out again at the surface's size, as a resize
    /// ([`Self::window_resized`]), so the menus are laid out at the size they are drawn at and `$pref::Video::resolution` says
    /// what the window really is.
    pub fn surface_size(&mut self, size: (u32, u32)) {
        let (w, h) = (size.0.max(1), size.1.max(1));
        let canvas = self.gui.0.borrow().canvas_size;
        if canvas != (w as i32, h as i32) {
            self.window_resized((w, h));
        }
    }

    /// The screen mode `setScreenMode` set since the last call ([`screen`]): the windowed client resizes its window to it or makes
    /// it full screen; a headless run never asks, and its canvas keeps its size.
    pub fn take_screen_mode(&mut self) -> Option<screen::ScreenMode> {
        self.client.borrow_mut().screen.take_request()
    }

    /// The screen mode the client is in (the start-up's, from the canvas and the preferences, until a `setScreenMode`).
    pub fn screen_mode(&self) -> screen::ScreenMode {
        screen::current(self.server.vm(), &self.gui, &self.client)
    }

    /// The URL opener `gotoWebPage` hands an http(s) URL to (after the game's checks); None (every headless run's default) opens
    /// nothing. Only the windowed client sets one.
    pub fn set_web_opener(&mut self, opener: Option<WebOpener>) {
        self.client.borrow_mut().web_opener = opener;
    }

    /// The next connection's datagrams are also written to `path` (a capture of the connection).
    pub fn record_relay(&mut self, path: Option<PathBuf>) {
        self.client.borrow_mut().conn.relay = path;
    }

    /// Where the client's socket may bind and send.
    pub fn net_policy(&self) -> net::NetPolicy {
        self.client.borrow().net.borrow().policy
    }

    /// Runs the clock with the wall for `wall` (one 32 ms tick each 32 ms of the wall; a stall is caught up a tick at a time, at most
    /// a second of it), or until `until` holds after a tick. A connection to a real server needs this (its time-outs and the server's
    /// are wall time).
    pub fn run_realtime(
        &mut self,
        wall: std::time::Duration,
        mut until: impl FnMut(&mut Self) -> bool,
    ) -> RunEnd {
        let start = std::time::Instant::now();
        let mut ran_ms: u64 = 0;
        loop {
            let elapsed = start.elapsed();
            if elapsed >= wall {
                return RunEnd::TimeLimit;
            }
            let due = elapsed.as_millis() as u64;
            if ran_ms + u64::from(TICK_MS) > due {
                let next = std::time::Duration::from_millis(ran_ms + u64::from(TICK_MS));
                std::thread::sleep(
                    next.saturating_sub(elapsed)
                        .min(std::time::Duration::from_millis(4)),
                );
                continue;
            }
            // Behind by more than a second: the rest is lost (the clock does not run a backlog of ticks at once).
            if due > ran_ms + 1000 {
                ran_ms = due - 1000;
            }
            let end = self.run(TICK_MS, |_| false);
            ran_ms += u64::from(TICK_MS);
            if !matches!(end, RunEnd::TimeLimit) {
                return end;
            }
            if until(self) {
                return RunEnd::Until;
            }
        }
    }

    /// One drawn frame with the host's own cache ([`draw`](Self::draw)).
    pub fn draw_frame(&mut self) -> gui::GuiDraws {
        let mut cache = std::mem::take(&mut self.cache);
        let draws = self.draw(&mut cache);
        self.cache = cache;
        draws
    }

    /// One canvas frame now (the layout to the canvas), as the main loop does after every pass.
    pub fn frame(&mut self) {
        self.gui.frame(self.server.vm_mut());
    }

    /// One drawn frame: the layout to the canvas, then the canvas as TGE's `renderFrame` draws it ([`gui::draw_canvas`]), then
    /// the script callbacks the draw asked for (a control's first `onRender`), run with no borrow held (the frame runs the fades'
    /// `onWait` and `onDone`). The GUI's clock is the simulation's. A callback's error is kept in [`ClientHost::draw_errors`].
    pub fn draw(&mut self, cache: &mut gui::DrawCache) -> gui::GuiDraws {
        self.gui.frame(self.server.vm_mut());
        let frame = gui::draw_canvas(self.server.vm(), &self.gui, cache);
        if let Err(e) = gui::run_callbacks(self.server.vm_mut(), &frame.callbacks)
            && self.draw_errors.len() < MAX_MISSING_OBJECTS
        {
            self.draw_errors.push(e);
        }
        frame.draws
    }

    /// The mouse at `pt` on the canvas: a `WM_MOUSEMOVE` there ([`post`](Self::post)).
    pub fn mouse_move(&mut self, pt: (i32, i32)) {
        self.post(
            input::WM_MOUSEMOVE,
            0,
            ((pt.1 as u32 & 0xffff) << 16) | (pt.0 as u32 & 0xffff),
        );
    }

    /// The mouse moved by `dx`, `dy` (the device's relative counts): DirectInput's X and Y axis moves, routed as TGE's game routes
    /// every event ([`input::route`]: the global map, the canvas, the other maps). The window sends these while the canvas's cursor
    /// is off (in game: moveMap's `yaw`/`pitch`); with the cursor on the mouse is the window's position ([`mouse_move`](Self::mouse_move)).
    pub fn mouse_motion(&mut self, dx: f32, dy: f32) {
        use gui::keys::{MOUSE_DEVICE, SI_MOVE, SI_XAXIS, SI_YAXIS};
        for (axis, value) in [(SI_XAXIS, dx), (SI_YAXIS, dy)] {
            if value == 0.0 || !value.is_finite() {
                continue;
            }
            let ev = gui::input::InputEvent {
                device: MOUSE_DEVICE,
                obj_type: axis,
                action: SI_MOVE,
                modifier: self.win32.modifiers,
                value,
                ..gui::input::InputEvent::default()
            };
            let vm = self.server.vm_mut();
            let saved =
                vm.begin_host_budget_with_runs(input::MAX_EVENT_WORK, input::MAX_EVENT_STEPS);
            let result = input::route(vm, &self.gui, &self.client, &ev);
            vm.end_host_budget(saved);
            if let Err(e) = result
                && self.input_errors.len() < MAX_MISSING_OBJECTS
            {
                self.input_errors.push(e);
            }
        }
    }

    /// Whether the game wants the mouse (the canvas's cursor off, or `lockMouse(1)`): the window grabs it then.
    pub fn wants_mouse(&self) -> bool {
        !self.gui.0.borrow().cursor_on || self.client.borrow().mouse_locked
    }

    /// Whether the window shows the system's cursor: the canvas's cursor is on and shown but the last frame drew none (no
    /// GuiCursor set, or its bitmap did not load), and the game does not hold the mouse. A game that sets a GuiCursor, or turns
    /// the cursor off, hides it again.
    pub fn system_cursor(&self) -> bool {
        let s = self.gui.0.borrow();
        s.cursor_on && s.show_cursor && !s.cursor_drawn && !self.client.borrow().mouse_locked
    }

    /// One window message through TGE's platform layer and the game ([`input::post`]): who took it. A script's error is kept in
    /// [`ClientHost::input_errors`].
    pub fn post(&mut self, msg: u32, wparam: u32, lparam: u32) -> input::Taken {
        match input::post(
            self.server.vm_mut(),
            &self.gui,
            &self.client,
            &mut self.win32,
            msg,
            wparam,
            lparam,
        ) {
            Ok(t) => t,
            Err(e) => {
                if self.input_errors.len() < MAX_MISSING_OBJECTS {
                    self.input_errors.push(e);
                }
                input::Taken::Nobody
            }
        }
    }

    /// An injection ([`input::parse_injection`]): each message posted, each pause run on the clock (the client's own work every tick).
    pub fn inject(&mut self, items: &[input::Injected]) {
        for item in items {
            match *item {
                input::Injected::Message(m, w, l) => {
                    self.post(m, w, l);
                }
                input::Injected::Sleep(ms) => {
                    self.run(ms, |_| false);
                }
            }
        }
    }

    /// Takes the log out of the `Vm`, noting the missing-object lines for the report.
    pub fn drain_log(&mut self) -> Vec<LogLine> {
        let lines = self.server.vm_mut().drain_log();
        for line in &lines {
            if self.missing_objects.len() < MAX_MISSING_OBJECTS
                && line.level != Level::Normal
                && let Some(found) = missing_object(&line.text)
            {
                self.missing_objects.push(found);
            }
        }
        lines
    }
}

/// One frame drawn for its script callbacks (the draw list is dropped): a tick's render pass in [`ClientHost::run`].
fn draw_callbacks(vm: &mut Vm, gui: &Gui, cache: &mut gui::DrawCache, errors: &mut Vec<VmError>) {
    if gui.canvas().is_none() {
        return;
    }
    let frame = gui::draw_canvas(vm, gui, cache);
    if frame.callbacks.is_empty() {
        return;
    }
    if let Err(e) = gui::run_callbacks(vm, &frame.callbacks)
        && errors.len() < MAX_MISSING_OBJECTS
    {
        errors.push(e);
    }
}

/// `X.f` from `... Unable to find object: 'X' attempting to call function 'f'`.
pub fn missing_object(text: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(text);
    let rest = text.split_once("Unable to find object: '")?.1;
    let (object, rest) = rest.split_once('\'')?;
    let function = rest
        .split_once("function '")
        .and_then(|(_, f)| f.split_once('\''))
        .map_or("", |(f, _)| f);
    Some(format!("{object}.{function}"))
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
