//! `setScreenMode`, `toggleFullScreen` and `isFullScreen`: TGE's console functions (`( int width, int height, int bpp, bool
//! fullScreen )`, 5 arguments) over `OpenGLDevice::setScreenMode` (`forceIt` 0, `repaint` 1), with TGE 1.4's rules as Torque's
//! ([`TgeScreen`]); a game gives its own ([`ScreenRules`], [`crate::game::ClientGame::screen_rules`]).
//!
//! **The mode** ([`choose`]): a window as wide or as tall as the desktop warns `can't switch to resolution larger than desktop in
//! windowed mode!` and takes the last listed mode narrower and shorter than the desktop (the first when none is). A mode under
//! 480x360 warns `can't go smaller than 480x360!` and answers 0. Full screen takes the listed mode equal to the one asked, else the
//! first nearest (|dw| + |dh| + 1 for another bpp); a window takes the desktop's colour depth. A mode equal to the current one answers
//! 1 with nothing changed; another prints `Setting screen mode to WxHxBPP (fs|w)...`.
//!
//! **What it sets**: `$pref::Video::fullScreen` "1" or "0", `$pref::Video::resolution` `"w h bpp"`, the current mode, then
//! `resetCanvas();` (`repaint`), and answers 1. The windowed client takes the mode ([`ScreenState::take_request`]) and resizes its
//! window (centred, as TGE's `SetWindowPos` does) or makes it full screen; the canvas follows the window's client size as it does on
//! any resize. A headless run's canvas keeps its fixed size (nothing takes the request): the engine-only part.
//!
//! **The modes** ([`Display`]): headless runs list the rules' fixed table ([`ScreenRules::fixed_display`]; Torque's
//! [`DISPLAY_MODES`] under a fixed desktop, [`DESKTOP`]), so their menus are the same on every machine. The windowed client tells
//! the host its monitor ([`Monitor`]); from then on the list is [`WINDOW_MODES`] (16:9 and 16:10) that fit it, plus its own size,
//! and the desktop is the monitor's (engine-only).
//!
//! The device's own lines (`OpenGLDevice::setScreenMode ...`, the window and context lines) describe hardware the engine does not drive;
//! only the warnings and `Setting screen mode to ...` are printed.
//!
//! Credit: follows TGE's video layer as Torque3D carries its window and canvas parts (MIT: `gui/core/guiCanvas.cpp`, Copyright
//! (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.).

use std::cell::RefCell;
use std::rc::Rc;

use gui::Gui;
use ts_vm::vm::{Native, Vm};

use crate::conn::ConnGame;
use crate::game::client_game;
use crate::state::ClientState;

/// The modes the windowed client lists (engine-only): 16:9 and 16:10, ordered by width then height as TGE's
/// device orders its own (the window's clamp walks it from the end). Only those that fit the window's monitor are listed.
pub const WINDOW_MODES: [(u32, u32); 12] = [
    (1280, 720),
    (1280, 800),
    (1366, 768),
    (1440, 900),
    (1600, 900),
    (1680, 1050),
    (1920, 1080),
    (1920, 1200),
    (2560, 1440),
    (2560, 1600),
    (3200, 1800),
    (3840, 2160),
];

/// The windowed client's first size on a fresh profile (no saved `$pref::Video::resolution`; engine-only).
pub const WINDOW_START_MODE: (u32, u32) = (1366, 768);

/// The smallest mode TGE's device takes (`can't go smaller than 480x360!`).
pub const MIN_MODE: (u32, u32) = (480, 360);

/// The monitor the window is on (the windowed client's, from winit): its desktop size and the sizes of its video modes (physical
/// pixels).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Monitor {
    pub size: (u32, u32),
    pub video_modes: Vec<(u32, u32)>,
}

impl Monitor {
    /// Whether a `w` x `h` mode fits the monitor: within its desktop or within one of its video modes.
    fn fits(&self, (w, h): (u32, u32)) -> bool {
        std::iter::once(self.size)
            .chain(self.video_modes.iter().copied())
            .any(|(mw, mh)| w <= mw && h <= mh)
    }

    /// The modes listed for this monitor: [`WINDOW_MODES`] that fit it, and its own size when that is not among them (never one
    /// under the rules' smallest, [`ScreenRules::min_mode`]), ordered by width then height.
    pub fn modes(&self, rules: &dyn ScreenRules) -> Vec<(u32, u32)> {
        let mut list: Vec<_> = WINDOW_MODES
            .iter()
            .copied()
            .filter(|m| self.fits(*m))
            .collect();
        let native = self.size;
        let min = rules.min_mode();
        if native.0 >= min.0 && native.1 >= min.1 && !list.contains(&native) {
            list.push(native);
            list.sort_unstable();
        }
        list
    }

    /// The window's size on a fresh profile: [`WINDOW_START_MODE`] when a window of that size fits the desktop
    /// ([`ScreenRules::window_fits`]), else the largest listed mode (by area) that does, else the smallest listed one (else the
    /// rules' smallest).
    pub fn start_size(&self, rules: &dyn ScreenRules) -> (u32, u32) {
        let fits = |&m: &(u32, u32)| rules.window_fits(self.size, m);
        if fits(&WINDOW_START_MODE) {
            return WINDOW_START_MODE;
        }
        let list = self.modes(rules);
        list.iter()
            .copied()
            .filter(fits)
            .max_by_key(|&(w, h)| (u64::from(w) * u64::from(h), w))
            .or_else(|| list.first().copied())
            .unwrap_or_else(|| rules.min_mode())
    }
}

/// The display the device chooses modes on: the desktop (`w, h, bpp`) and the listed modes (`w, h`, all at 32 bits and 60 Hz).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    pub desktop: (u32, u32, u32),
    pub modes: Vec<(u32, u32)>,
}

impl Display {
    /// The window's monitor's ([`Monitor::modes`], its size the desktop at 32 bits); a monitor with no mode listed keeps the
    /// rules' fixed list.
    pub fn of_monitor(monitor: &Monitor, rules: &dyn ScreenRules) -> Display {
        let modes = monitor.modes(rules);
        if modes.is_empty() {
            return rules.fixed_display();
        }
        Display {
            desktop: (monitor.size.0, monitor.size.1, DESKTOP.2),
            modes,
        }
    }

    /// The client's: its window's monitor when the windowed client gave one, else the rules' fixed list.
    pub fn of<G: ConnGame>(state: &ClientState<G>, rules: &dyn ScreenRules) -> Display {
        state
            .screen
            .monitor
            .as_ref()
            .map_or_else(|| rules.fixed_display(), |m| Display::of_monitor(m, rules))
    }
}

/// A screen mode: the client size, colour depth, refresh and whether it is full screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenMode {
    pub width: u32,
    pub height: u32,
    pub bpp: u32,
    pub hz: u32,
    pub full_screen: bool,
    /// The rules' borderless window when the mode was set ([`ScreenRules::borderless`]; never in TGE's).
    pub borderless: bool,
}

/// The device's mode state.
#[derive(Debug, Default)]
pub struct ScreenState {
    /// The mode the last `setScreenMode` set (None: the start-up's, from the canvas and the preferences).
    pub current: Option<ScreenMode>,
    /// A mode set and not yet taken by the window.
    pub request: Option<ScreenMode>,
    /// A window takes the modes (the windowed client): the canvas takes a mode's size at once, as TGE's does (its
    /// `setScreenMode` sizes the window before `resetCanvas`, so the content's next `onRender` lays the HUD out at the new size).
    /// Headless runs keep their fixed canvas.
    pub windowed: bool,
    /// The monitor the window is on (the windowed client's): the modes listed and the desktop ([`Display::of`]). None in every
    /// headless run (the fixed list).
    pub monitor: Option<Monitor>,
}

impl ScreenState {
    /// The mode set since the last call, for the window to apply (the windowed client's; headless never asks).
    pub fn take_request(&mut self) -> Option<ScreenMode> {
        self.request.take()
    }
}

/// What [`choose`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Too small: answer 0.
    TooSmall,
    /// The mode to set, and whether the windowed size was clamped (the warning).
    Mode { mode: ScreenMode, clamped: bool },
}

/// TGE 1.4's `OpenGLDevice::setScreenMode` choice of mode on `display` for `(w, h, bpp, hz, full)` (the listed modes are 32-bit;
/// `hz` is kept as asked).
pub fn choose(display: &Display, w: u32, h: u32, bpp: u32, hz: u32, full: bool) -> Choice {
    let (mut w, mut h, mut bpp) = (w, h, bpp);
    let list: Vec<_> = display.modes.iter().map(|&(w, h)| (w, h, 32)).collect();
    let (dw, dh) = (display.desktop.0, display.desktop.1);
    let mut clamped = false;
    if !full && !list.is_empty() && (w >= dw || h >= dh) {
        clamped = true;
        let mut i = list.len() - 1;
        while i != 0 {
            let (mw, mh, _) = list[i];
            if mw < dw && mh < dh {
                break;
            }
            i -= 1;
        }
        (w, h, bpp) = list[i];
    }
    if w < MIN_MODE.0 || h < MIN_MODE.1 {
        return Choice::TooSmall;
    }
    if full {
        let mut best: Option<(u64, (u32, u32, u32))> = None;
        for &m in &list {
            if m == (w, h, bpp) {
                best = Some((0, m));
                break;
            }
            let d = u64::from(w.abs_diff(m.0)) + u64::from(h.abs_diff(m.1)) + u64::from(bpp != m.2);
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, m));
            }
        }
        if let Some((_, m)) = best {
            (w, h, bpp) = m;
        }
    } else {
        // A window has the desktop's colour depth.
        bpp = display.desktop.2;
    }
    Choice::Mode {
        mode: ScreenMode {
            width: w,
            height: h,
            bpp,
            hz,
            full_screen: full,
            borderless: false,
        },
        clamped,
    }
}

/// A game's rules for `setScreenMode` ([`crate::game::ClientGame::screen_rules`]). Every method has TGE 1.4's answer by default
/// ([`TgeScreen`]).
pub trait ScreenRules {
    /// The console function's least and most arguments and its usage (a sixth argument, when allowed, is the refresh).
    fn usage(&self) -> (i32, i32, &'static str) {
        (5, 5, "( int width, int height, int bpp, bool fullScreen )")
    }

    /// The line the console function prints before the device runs (TGE's prints none).
    fn call_line(&self, w: u32, h: u32, bpp: u32, full: bool) -> Option<String> {
        let _ = (w, h, bpp, full);
        None
    }

    /// The headless display: the fixed desktop and modes ([`DESKTOP`], [`DISPLAY_MODES`]).
    fn fixed_display(&self) -> Display {
        Display {
            desktop: DESKTOP,
            modes: DISPLAY_MODES.to_vec(),
        }
    }

    /// The smallest mode the device takes ([`MIN_MODE`]).
    fn min_mode(&self) -> (u32, u32) {
        MIN_MODE
    }

    /// Whether the window is to have no frame (TGE's always has one).
    fn borderless(&self, vm: &Vm) -> bool {
        let _ = vm;
        false
    }

    /// The device's choice of mode on `display` for `(w, h, bpp, hz)` ([`choose`]).
    fn choose(
        &self,
        display: &Display,
        (w, h, bpp, hz): (u32, u32, u32, u32),
        full: bool,
        borderless: bool,
    ) -> Choice {
        let _ = borderless;
        choose(display, w, h, bpp, hz, full)
    }

    /// Whether a window of `size` fits a `desktop` (TGE's: narrower and shorter than it).
    fn window_fits(&self, desktop: (u32, u32), size: (u32, u32)) -> bool {
        size.0 < desktop.0 && size.1 < desktop.1
    }

    /// The lines of a chosen mode (`same`: it is the current one), and whether it is to be set. TGE's: the current mode is
    /// kept without a word; another prints `Setting screen mode to WxHxBPP (fs|w)...`.
    fn announce(&self, vm: &mut Vm, mode: &ScreenMode, same: bool) -> bool {
        if same {
            return false;
        }
        vm.print(
            format!(
                "Setting screen mode to {}x{}x{} ({})...",
                mode.width,
                mode.height,
                mode.bpp,
                if mode.full_screen { "fs" } else { "w" }
            )
            .as_bytes(),
        );
        true
    }

    /// `$pref::Video::resolution`'s text for a mode (TGE's: `"w h bpp"`).
    fn resolution_text(&self, mode: &ScreenMode) -> String {
        format!("{} {} {}", mode.width, mode.height, mode.bpp)
    }
}

/// TGE 1.4's rules.
#[derive(Debug, Clone, Copy, Default)]
pub struct TgeScreen;

impl ScreenRules for TgeScreen {}

/// The mode the client starts in: the canvas's size, the preference's depth, 60 Hz, `$pref::Video::fullScreen`, the rules'
/// borderless window.
fn start_mode(vm: &Vm, gui: &Gui) -> ScreenMode {
    let (w, h) = gui.0.borrow().canvas_size;
    let flag = |name: &str| ts_vm::fields::atob(&vm.global(name).unwrap_or_default());
    ScreenMode {
        width: w.max(0) as u32,
        height: h.max(0) as u32,
        bpp: current_bpp(vm),
        hz: 60,
        full_screen: flag("$pref::Video::fullScreen"),
        borderless: client_game(vm).screen_rules().borderless(vm),
    }
}

/// The current mode (the start-up's until a `setScreenMode`).
pub fn current<G: ConnGame>(
    vm: &Vm,
    gui: &Gui,
    client: &Rc<RefCell<ClientState<G>>>,
) -> ScreenMode {
    let set = client.borrow().screen.current;
    set.unwrap_or_else(|| start_mode(vm, gui))
}

fn int(arg: &[u8]) -> u32 {
    // `atoi` into the device's unsigned fields: a negative size is a huge one (and so clamped or refused).
    ts_vm::text::atoi(arg) as u32
}

/// OpenGLDevice's `setScreenMode`: the mode the rules choose ([`ScreenRules::choose`]), refused under their smallest (false),
/// announced, and kept with the preferences and the canvas reset unless it is the mode already in use.
#[allow(clippy::too_many_arguments)]
fn device_set_mode<G: ConnGame>(
    vm: &mut Vm,
    gui: &Gui,
    client: &Rc<RefCell<ClientState<G>>>,
    w: u32,
    h: u32,
    bpp: u32,
    hz: u32,
    full: bool,
) -> bool {
    let rules = client_game(vm).screen_rules();
    let borderless = rules.borderless(vm);
    let display = Display::of(&client.borrow(), &*rules);
    let (mode, clamped) = match rules.choose(&display, (w, h, bpp, hz), full, borderless) {
        Choice::TooSmall => {
            let (mw, mh) = rules.min_mode();
            vm.warn(format!(
                "OpenGLDevice::setScreenMode -- can't go smaller than {mw}x{mh}!"
            ));
            return false;
        }
        Choice::Mode { mode, clamped } => (mode, clamped),
    };
    if clamped {
        vm.warn(
                "OpenGLDevice::setScreenMode -- can't switch to resolution larger than desktop in windowed mode!"
                    .into(),
            );
    }
    let now = current(vm, gui, client);
    if !rules.announce(vm, &mode, now == mode) {
        return true;
    }
    vm.set_global(
        "$pref::Video::fullScreen",
        if mode.full_screen { "1" } else { "0" },
    );
    vm.set_global("$pref::Video::resolution", &rules.resolution_text(&mode));
    let windowed = {
        let mut state = client.borrow_mut();
        state.screen.current = Some(mode);
        state.screen.request = Some(mode);
        state.screen.windowed
    };
    // The device has resized its window when `resetCanvas` runs (`SetWindowPos`, `ChangeDisplaySettings`), so the
    // canvas is the new size by the content's next `onRender` (a HUD lays itself out from `getRes()`).
    if windowed {
        gui.set_canvas_size((mode.width as i32, mode.height as i32));
    }
    // `repaint`: no borrow is held while the script runs.
    if let Err(e) = vm.invoke(None, b"resetCanvas", &[]) {
        vm.error(format!("resetCanvas: {e}"));
    }
    true
}

pub fn register<G: ConnGame>(vm: &mut Vm, gui: &Gui, client: &Rc<RefCell<ClientState<G>>>) {
    {
        let (gui, client) = (gui.clone(), Rc::clone(client));
        let rules = client_game(vm).screen_rules();
        let (min, max, usage) = rules.usage();
        vm.register("setScreenMode", min, max, usage, move |vm, c| {
            let a = |i: usize| crate::arg(c, i);
            // A sixth argument, where the rules take one, is the refresh.
            let hz = if c.argv.len() == 6 { int(a(5)) } else { 60 };
            let full = ts_vm::fields::atob(a(4));
            let (w, h, bpp) = (int(a(1)), int(a(2)), int(a(3)));
            if let Some(line) = rules.call_line(w, h, bpp, full) {
                vm.print(line.as_bytes());
            }
            Native::Bool(device_set_mode(vm, &gui, &client, w, h, bpp, hz, full))
        });
    }
    // `toggleFullScreen()` over the device's own toggle: `setScreenMode` of the current mode with full screen flipped (no
    // line of the console function's). It answers whether the mode was set.
    {
        let (gui, client) = (gui.clone(), Rc::clone(client));
        vm.register("toggleFullScreen", 1, 1, "", move |vm, _| {
            let now = current(vm, &gui, &client);
            Native::Bool(device_set_mode(
                vm,
                &gui,
                &client,
                now.width,
                now.height,
                now.bpp,
                now.hz,
                !now.full_screen,
            ))
        });
    }
    let (gui, client) = (gui.clone(), Rc::clone(client));
    vm.register(
        "isFullScreen",
        1,
        1,
        "Is the game running full-screen?",
        move |vm, _| Native::Bool(current(vm, &gui, &client).full_screen),
    );
}

/// The display device the engine reports by default (TGE's device table holds what the machine's drivers offer; its OpenGL
/// device is the one named here): what a headless run answers. The windowed client reports its renderer's backend instead
/// ([`ClientState::display_device`]).
pub const DISPLAY_DEVICE: &str = "OpenGL";

/// The modes Torque's headless device reports (`width, height`, all at 32 bits and 60 Hz; a fixed list of common modes, so the
/// options' menus are the same on every machine; TGE's lists what the drivers offer). The windowed client lists its monitor's
/// ([`Display`]).
pub const DISPLAY_MODES: [(u32, u32); 7] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 1024),
    (1600, 900),
    (1920, 1080),
];

/// The headless desktop (`getDesktopResolution`).
pub const DESKTOP: (u32, u32, u32) = (1920, 1080, 32);

/// The colour depth of the current mode: the third word of `$pref::Video::resolution` when it is 16 or 32, else 32.
pub fn current_bpp(vm: &Vm) -> u32 {
    let pref = vm.global("$pref::Video::resolution").unwrap_or_default();
    let word = pref
        .split(|b| *b == b' ' || *b == b'\t')
        .filter(|w| !w.is_empty())
        .nth(2)
        .map(ts_vm::text::atoi);
    match word {
        Some(16) => 16,
        _ => 32,
    }
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
