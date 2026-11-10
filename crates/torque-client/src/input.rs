//! The client's input as TGE's Windows platform layer and game make it: window messages become engine events ([`Win32`]: the
//! window procedure, its event queue and key handler), and every engine event goes the way TGE's game sends it ([`route`],
//! `processInputEvent`): the global action map (the first of ActiveActionMapSet), else the canvas, else the other maps from the top.
//!
//! **Window messages** ([`Win32::message`]):
//! - `WM_MOUSEMOVE`: the client position (signed 16-bit halves) as a mouse move (the canvas only, [`gui::input::process_mouse_move`]).
//! - `WM_LBUTTONDOWN`/`UP`, `WM_RBUTTONDOWN`/`UP`, `WM_MBUTTONDOWN`/`UP`, `WM_XBUTTONDOWN`/`UP`: mouse button 0, 1, 2, 3 make/break
//!   (value 1/0).
//! - `WM_MOUSEWHEEL`: a message whose own delta is at least 120 either way is a Z axis move of that delta.
//! - `WM_KEYDOWN`/`UP`, `WM_SYSKEYDOWN`/`UP`: the key code of the virtual key ([`gui::keys::vk_to_key`]: every key its own; TGE's
//!   virtual-key table, else the scan code's, with [`gui::ext::GuiRule::TgeKeyCodes`]); with the extended
//!   bit Return is numpad Enter and Ctrl/Alt the right ones; Shift is the right one for scan code 0x36. A down whose previous state bit
//!   is set is a repeat. The layer keeps its own modifier bits and key states from these messages alone (Caps Lock toggles), and the
//!   event's ascii is `ToAscii` over those states (US layout: [`gui::keys::to_ascii`]). A modifier key's own event has no modifiers.
//!   `WM_CHAR` is not read.
//!
//! Nothing here touches the operating system: the window (the `platform` glue's key map) and the headless injection
//! ([`Injected`]) both feed it the same messages. A game or a test names keys and buttons with [`Key`] and [`MouseButton`]
//! ([`key_message`], [`button_message`]; the client host's `key`, `press`, `type_text` and `mouse_button`).
//!
//! Credit: follows TGE's Windows input layer and event routing as Torque3D carries them (MIT: `platformWin32/winInput.cpp`,
//! `windowManager/win32/winDispatch.cpp`, each Copyright (c) 2012 GarageGames, LLC; `gui/core/guiCanvas.cpp`, Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.).

use std::cell::RefCell;
use std::rc::Rc;

use gui::Gui;
use gui::input::InputEvent;
use gui::keys::{self, *};
use ts_vm::vm::{Vm, VmError};

use crate::actionmap;
use crate::conn::ConnGame;
use crate::state::ClientState;

#[path = "input_key.rs"]
mod key;
pub use key::{Key, MouseButton, button_message, key_message};

pub const WM_KEYDOWN: u32 = 0x100;
pub const WM_KEYUP: u32 = 0x101;
pub const WM_SYSKEYDOWN: u32 = 0x104;
pub const WM_SYSKEYUP: u32 = 0x105;
pub const WM_CHAR: u32 = 0x102;
pub const WM_MOUSEMOVE: u32 = 0x200;
pub const WM_LBUTTONDOWN: u32 = 0x201;
pub const WM_LBUTTONUP: u32 = 0x202;
pub const WM_RBUTTONDOWN: u32 = 0x204;
pub const WM_RBUTTONUP: u32 = 0x205;
pub const WM_MBUTTONDOWN: u32 = 0x207;
pub const WM_MBUTTONUP: u32 = 0x208;
pub const WM_MOUSEWHEEL: u32 = 0x20a;
pub const WM_XBUTTONDOWN: u32 = 0x20b;
pub const WM_XBUTTONUP: u32 = 0x20c;

/// The work one event's routing may do outside the scripts it calls, and the steps those scripts may take.
pub const MAX_EVENT_WORK: u64 = gui::input::MAX_EVENT_WORK;
pub const MAX_EVENT_STEPS: u64 = gui::input::MAX_EVENT_STEPS;

/// What a message became.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Engine {
    /// The mouse at a client position, with the modifiers.
    Move((i32, i32), u8),
    Input(InputEvent),
}

/// The platform input state: the modifier bits and the key states it keeps from the key messages.
#[derive(Debug, Clone)]
pub struct Win32 {
    pub modifiers: u8,
    keys: [u8; 256],
    /// TGE's key codes ([`gui::ext::GuiRule::TgeKeyCodes`]; [`post`] sets it from the GUI's extension), else the engine's.
    pub tge_key_codes: bool,
}

impl Default for Win32 {
    fn default() -> Win32 {
        Win32 {
            modifiers: 0,
            keys: [0; 256],
            tge_key_codes: false,
        }
    }
}

impl Win32 {
    /// Whether Caps Lock is on (the layer's own toggle, from the key messages).
    pub fn caps_lock(&self) -> bool {
        self.keys[0x14] & 1 != 0
    }

    /// One window message.
    pub fn message(&mut self, msg: u32, wparam: u32, lparam: u32) -> Option<Engine> {
        let button = |obj: u16, make: bool, mods: u8| {
            Engine::Input(InputEvent {
                device: MOUSE_DEVICE,
                device_inst: 0,
                obj_type: SI_BUTTON,
                obj_inst: obj,
                action: if make { SI_MAKE } else { SI_BREAK },
                modifier: mods,
                ascii: 0,
                value: if make { 1.0 } else { 0.0 },
            })
        };
        let m = self.modifiers;
        match msg {
            WM_MOUSEMOVE => {
                let x = i32::from(lparam as u16 as i16);
                let y = i32::from((lparam >> 16) as u16 as i16);
                Some(Engine::Move((x, y), m))
            }
            WM_LBUTTONDOWN => Some(button(KEY_BUTTON0, true, m)),
            WM_LBUTTONUP => Some(button(KEY_BUTTON0, false, m)),
            WM_RBUTTONDOWN => Some(button(KEY_BUTTON1, true, m)),
            WM_RBUTTONUP => Some(button(KEY_BUTTON1, false, m)),
            WM_MBUTTONDOWN => Some(button(KEY_BUTTON2, true, m)),
            WM_MBUTTONUP => Some(button(KEY_BUTTON2, false, m)),
            WM_XBUTTONDOWN => Some(button(0x103, true, m)),
            WM_XBUTTONUP => Some(button(0x103, false, m)),
            WM_MOUSEWHEEL => {
                let delta = i32::from((wparam >> 16) as u16 as i16);
                (delta.abs() >= 120).then_some(Engine::Input(InputEvent {
                    device: MOUSE_DEVICE,
                    obj_type: SI_ZAXIS,
                    action: SI_MOVE,
                    modifier: m,
                    value: delta as f32,
                    ..InputEvent::default()
                }))
            }
            WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => self.key(msg, wparam, lparam),
            _ => None,
        }
    }

    /// The key handler.
    fn key(&mut self, msg: u32, wparam: u32, lparam: u32) -> Option<Engine> {
        let vk = (wparam & 0xff) as u8;
        let scan = ((lparam >> 16) & 0xff) as u8;
        let extended = (lparam >> 24) & 1 != 0;
        let previous = (lparam >> 30) & 1 != 0;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let base = if self.tge_key_codes {
            keys::tge_vk_to_key(vk, scan)
        } else {
            keys::vk_to_key(vk, scan)
        };
        let code = match base {
            KEY_RETURN if extended => KEY_NUMPADENTER,
            KEY_CONTROL => KEY_LCONTROL + u16::from(extended),
            KEY_ALT => KEY_LALT + u16::from(extended),
            KEY_SHIFT => KEY_LSHIFT + u16::from(scan == 0x36),
            other => other,
        };
        let bit = match code {
            KEY_LCONTROL => Some(SI_LCTRL),
            KEY_RCONTROL => Some(SI_RCTRL),
            KEY_LALT => Some(SI_LALT),
            KEY_RALT => Some(SI_RALT),
            KEY_LSHIFT => Some(SI_LSHIFT),
            KEY_RSHIFT => Some(SI_RSHIFT),
            _ => None,
        };
        let event_mods = if bit.is_some() { 0 } else { self.modifiers };
        let slot = usize::from(vk);
        if down {
            if let Some(b) = bit {
                self.modifiers |= b;
            }
            if base == KEY_CAPSLOCK {
                if self.keys[slot] & 1 == 0 {
                    self.keys[slot] |= 1;
                } else {
                    self.keys[slot] &= 0xfe;
                }
            }
            self.keys[slot] |= 0x80;
        } else {
            if let Some(b) = bit {
                self.modifiers &= !b;
            }
            self.keys[slot] &= 0x7f;
        }
        let states = self.keys;
        let caps = states[0x14] & 1 != 0;
        let ascii = keys::to_ascii(vk, &|k| states[usize::from(k)] & 0x80 != 0, caps);
        Some(Engine::Input(InputEvent {
            device: KEYBOARD_DEVICE,
            device_inst: 0,
            obj_type: SI_KEY,
            obj_inst: code,
            action: if !down {
                SI_BREAK
            } else if previous {
                SI_REPEAT
            } else {
                SI_MAKE
            },
            modifier: event_mods,
            ascii: u16::from(ascii),
            value: if down { 1.0 } else { 0.0 },
        }))
    }
}

/// Who took an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    Global,
    Canvas,
    Map(u32),
    Nobody,
}

/// TGE's `processInputEvent`: the global map (the first member of ActiveActionMapSet), then the canvas, then the other
/// maps from the top down. Runs under the caller's budget.
pub fn route<G: ConnGame>(
    vm: &mut Vm,
    gui: &Gui,
    client: &Rc<RefCell<ClientState<G>>>,
    ev: &InputEvent,
) -> Result<Taken, VmError> {
    // The Escape filter of a server's scripts comes before any action map: a game's binding of Escape (its menu) must not see the key
    // the engine takes to close a server's menu, or to stop its scripts.
    if gui.client_script_escape(ev) {
        return Ok(Taken::Canvas);
    }
    let maps: Vec<u32> = vm
        .objects()
        .find(b"ActiveActionMapSet")
        .and_then(|set| vm.objects().members(set).map(|m| m.as_slice().to_vec()))
        .unwrap_or_default();
    if !vm.charge_work(maps.len() as u64 * 16) {
        return Err(VmError::StepLimit);
    }
    if let Some(&global) = maps.first()
        && vm.objects().contains(global)
        && actionmap::process_action(vm, client, global, ev)?
    {
        return Ok(Taken::Global);
    }
    // The user's own click or key on a GUI control (a mouse or axis move is none): only while the canvas handles it (not an action map's binding) may the
    // join screens' scripts arm an internet join permit ([`crate::net::Internet::arm`]).
    let press = ev.action != SI_MOVE;
    let was = std::mem::replace(&mut client.borrow_mut().user_input, press);
    let canvas = gui::input::process_input_event(vm, gui, ev);
    client.borrow_mut().user_input = was;
    if canvas? {
        return Ok(Taken::Canvas);
    }
    for &map in maps.iter().skip(1).rev() {
        if vm.objects().contains(map) && actionmap::process_action(vm, client, map, ev)? {
            return Ok(Taken::Map(map));
        }
    }
    Ok(Taken::Nobody)
}

/// One message through the platform layer and the game, under a host budget of its own.
pub fn post<G: ConnGame>(
    vm: &mut Vm,
    gui: &Gui,
    client: &Rc<RefCell<ClientState<G>>>,
    win: &mut Win32,
    msg: u32,
    wparam: u32,
    lparam: u32,
) -> Result<Taken, VmError> {
    win.tge_key_codes = gui.ext().rule(gui::ext::GuiRule::TgeKeyCodes);
    let Some(engine) = win.message(msg, wparam, lparam) else {
        return Ok(Taken::Nobody);
    };
    let saved = vm.begin_host_budget_with_runs(MAX_EVENT_WORK, MAX_EVENT_STEPS);
    let result = match engine {
        Engine::Move(pt, mods) => {
            gui::input::process_mouse_move(vm, gui, pt, mods).map(|()| Taken::Canvas)
        }
        Engine::Input(ev) => route(vm, gui, client, &ev),
    };
    vm.end_host_budget(saved);
    result
}

/// A headless injection: one line a message or a pause, in the injection's words:
/// `move X Y`, `down X Y`, `up X Y`, `rdown X Y`, `rup X Y`, `wheel D`, `keydown VK`, `keyup VK`, `key VK` (down and up),
/// `repeat VK`, `char C` (posted, unread), `sleep MS`, `activate` (nothing); `#` starts a comment. The scan codes are the US
/// layout's (`MapVirtualKey`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Injected {
    Message(u32, u32, u32),
    Sleep(u32),
}

/// The most injected items a text holds, and the longest pause one names.
pub const MAX_INJECTED: usize = 100_000;
pub const MAX_INJECT_SLEEP_MS: u32 = 60_000;

/// The messages and pauses of an injection text, or the first line it cannot read.
pub fn parse_injection(text: &str) -> Result<Vec<Injected>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        for item in line.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            if out.len() >= MAX_INJECTED {
                return Err(format!("more than {MAX_INJECTED} items"));
            }
            out.extend(parse_item(item).map_err(|e| format!("line {}: {e}: {item:?}", n + 1))?);
        }
    }
    Ok(out)
}

fn coords(x: i32, y: i32) -> u32 {
    ((y as u32 & 0xffff) << 16) | (x as u32 & 0xffff)
}

/// The lParam of an injected key message: repeat count 1, the scan code, the up bits for a key up, the previous-state bit for
/// a repeat.
pub fn key_lparam(vk: u8, up: bool, repeat: bool) -> u32 {
    let scan = u32::from(keys::map_virtual_key(vk));
    let mut l = 1 | (scan << 16);
    if up {
        l |= (1 << 30) | (1 << 31);
    } else if repeat {
        l |= 1 << 30;
    }
    l
}

/// One item of an injection (its words).
pub fn parse_item(item: &str) -> Result<Vec<Injected>, String> {
    let w: Vec<&str> = item.split_whitespace().collect();
    let num = |i: usize| -> Result<i32, String> {
        w.get(i)
            .ok_or_else(|| "a number is missing".to_string())?
            .parse::<i32>()
            .map_err(|e| e.to_string())
    };
    let vk = |i: usize| -> Result<u8, String> {
        let v = num(i)?;
        u8::try_from(v)
            .ok()
            .filter(|v| *v != 0 && *v != 0xff)
            .ok_or_else(|| format!("virtual key {v}"))
    };
    use Injected::Message as M;
    Ok(match w.first().copied().unwrap_or("") {
        "move" => vec![M(WM_MOUSEMOVE, 0, coords(num(1)?, num(2)?))],
        "down" => vec![M(WM_LBUTTONDOWN, 1, coords(num(1)?, num(2)?))],
        "up" => vec![M(WM_LBUTTONUP, 0, coords(num(1)?, num(2)?))],
        "rdown" => vec![M(WM_RBUTTONDOWN, 2, coords(num(1)?, num(2)?))],
        "rup" => vec![M(WM_RBUTTONUP, 0, coords(num(1)?, num(2)?))],
        "wheel" => vec![M(WM_MOUSEWHEEL, ((num(1)? as u32) & 0xffff) << 16, 0)],
        "keydown" => {
            let v = vk(1)?;
            vec![M(WM_KEYDOWN, u32::from(v), key_lparam(v, false, false))]
        }
        "keyup" => {
            let v = vk(1)?;
            vec![M(WM_KEYUP, u32::from(v), key_lparam(v, true, false))]
        }
        "repeat" => {
            let v = vk(1)?;
            vec![M(WM_KEYDOWN, u32::from(v), key_lparam(v, false, true))]
        }
        "key" => {
            let v = vk(1)?;
            vec![
                M(WM_KEYDOWN, u32::from(v), key_lparam(v, false, false)),
                M(WM_KEYUP, u32::from(v), key_lparam(v, true, false)),
            ]
        }
        "char" => vec![M(WM_CHAR, num(1)? as u32, 1)],
        "sleep" => {
            let ms = num(1)?;
            if !(0..=MAX_INJECT_SLEEP_MS as i32).contains(&ms) {
                return Err(format!("a pause of {ms} ms"));
            }
            vec![Injected::Sleep(ms as u32)]
        }
        "activate" => Vec::new(),
        other => return Err(format!("unknown word {other:?}")),
    })
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
