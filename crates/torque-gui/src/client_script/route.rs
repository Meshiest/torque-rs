//! Input for the host layers, taken before the canvas sees it. Every Escape goes to the Escape filter first. Other keys go to the
//! top engine prompt while one is up (it is modal: the game gets none of them), else to the realm's focused text field as text
//! and navigation only. The pointer goes to the chip, then the engine layer, then the realm's focused field and the server
//! layer. The realm is given nothing while a game dialog or an engine prompt is up.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use super::controls::{Notch, hit, origin, wheel};
use super::layers::{
    LayerAction, Release, chip_wanted, focus_on_top, prompt_up, release_holds, server_takes_input,
    turn_off,
};
use crate::Gui;
use crate::canvas::hit_test_gui;
use crate::control;
use crate::custom::{self, Pointer};
use crate::input::{self, GuiEvent, InputEvent};
use crate::keys::{
    KEY_BACKSPACE, KEY_BUTTON0, KEY_DELETE, KEY_END, KEY_ESCAPE, KEY_HOME, KEY_LEFT,
    KEY_NUMPADENTER, KEY_RETURN, KEY_RIGHT, KEYBOARD_DEVICE, MOUSE_DEVICE, SI_BREAK, SI_BUTTON,
    SI_MAKE, SI_REPEAT, SI_SHIFT, SI_XAXIS, SI_YAXIS, SI_ZAXIS, normalise_modifiers,
};

/// Where a control sits: it, and its top left corner on the canvas.
#[derive(Clone, Copy, Debug)]
struct Target {
    id: ObjId,
    origin: (i32, i32),
}

fn cursor(gui: &Gui) -> (i32, i32) {
    gui.0.borrow().cursor_pos
}

/// The engine layer's control under `pt` (the chip first, then the prompts from the top): the control itself, or for a prompt
/// the deepest control under the point. A prompt takes the point wherever it is inside the prompt's rectangle.
fn engine_hit(vm: &Vm, gui: &Gui, pt: (i32, i32)) -> Option<Target> {
    let (chip, engine) = {
        let s = gui.0.borrow();
        (s.client_script.chip, s.client_script.engine.clone())
    };
    let chip = chip.filter(|_| chip_wanted(vm, gui));
    for id in chip.into_iter().chain(engine.into_iter().rev()) {
        if !vm.objects().contains(id) {
            continue;
        }
        let (pos, ext) = control::bounds(vm, id);
        let local = (pt.0 - pos.0, pt.1 - pos.1);
        if !(0..ext.0).contains(&local.0) || !(0..ext.1).contains(&local.1) {
            continue;
        }
        let id = match chip {
            Some(c) if c == id => id,
            _ => hit_test_gui(vm, gui, id, pt, crate::canvas::MAX_HIT_VISITS),
        };
        return Some(Target {
            id,
            origin: origin(vm, gui, id),
        });
    }
    None
}

/// The server layer's control under `pt`, when the layer may be given input: never in the chip's column.
fn server_hit(vm: &Vm, gui: &Gui, pt: (i32, i32)) -> Option<Target> {
    if !server_takes_input(vm, gui) || gui.0.borrow().client_script.in_column(pt) {
        return None;
    }
    if let Some((id, origin)) = focus_on_top(vm, gui) {
        let (_, ext) = control::bounds(vm, id);
        if (origin.0..origin.0 + ext.0).contains(&pt.0)
            && (origin.1..origin.1 + ext.1).contains(&pt.1)
        {
            return Some(Target { id, origin });
        }
    }
    let root = gui.0.borrow().client_script.server?;
    hit(vm, gui, root, pt).map(|(id, origin)| Target { id, origin })
}

/// The control the pointer is over: the engine layer's, else the server layer's.
fn target_at(vm: &Vm, gui: &Gui, pt: (i32, i32)) -> Option<Target> {
    engine_hit(vm, gui, pt).or_else(|| server_hit(vm, gui, pt))
}

fn deliver(
    vm: &mut Vm,
    gui: &Gui,
    to: Target,
    which: Pointer,
    ev: &InputEvent,
) -> Result<(), VmError> {
    let event = GuiEvent {
        modifier: ev.modifier,
        point: cursor(gui),
        clicks: 1,
        ..GuiEvent::default()
    };
    match custom::pointer_at(vm, gui, to.id, which, event, to.origin) {
        Some(result) => result,
        // A control the game gave the engine layer: its own handlers.
        None => stock_pointer(vm, gui, to.id, which, event),
    }
}

/// The canvas's handler for `which`, for a control that is not a plugin's.
fn stock_pointer(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    which: Pointer,
    event: GuiEvent,
) -> Result<(), VmError> {
    match which {
        Pointer::Down => input::on_mouse_down(vm, gui, id, event),
        Pointer::Up => input::on_mouse_up(vm, gui, id, event),
        Pointer::Move => input::on_mouse_move(vm, gui, id, event),
        Pointer::Dragged => input::on_mouse_dragged(vm, gui, id, event),
        Pointer::Enter => input::on_mouse_enter(vm, gui, id, event),
        Pointer::Leave => input::on_mouse_leave(vm, gui, id, event),
    }
}

/// Takes the event before the canvas: true when a layer consumed it.
pub(crate) fn route(vm: &mut Vm, gui: &Gui, ev: &InputEvent) -> Result<bool, VmError> {
    if ev.device == KEYBOARD_DEVICE && ev.obj_inst == KEY_ESCAPE {
        return Ok(escape(gui, ev));
    }
    let prompt = prompt_up(gui);
    if ev.device == KEYBOARD_DEVICE && prompt {
        return prompt_key(vm, gui, ev);
    }
    let (active, cursor_on) = {
        let s = gui.0.borrow();
        (s.client_script.active(), s.cursor_on)
    };
    if !active && !prompt {
        return Ok(false);
    }
    match ev.device {
        KEYBOARD_DEVICE => key(vm, gui, ev),
        MOUSE_DEVICE if cursor_on => mouse(vm, gui, ev),
        MOUSE_DEVICE => {
            // A press with the cursor off (mouselook) is the game's, and is not on the field.
            if ev.obj_type == SI_BUTTON && ev.action == SI_MAKE {
                gui.0.borrow_mut().client_script.blur();
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// The Escape filter. Shift+Escape stops server scripts for the connection. With an engine prompt up, Escape is the host's: the
/// prompt does not get it, nor does the game under it. Escape with the realm holding focus, the cursor or the camera drops the
/// holds and tells the realm `closed`. A press taken here has its release and repeats taken too; with nothing held and no
/// prompt the key goes to the game. The realm never gets it.
fn escape(gui: &Gui, ev: &InputEvent) -> bool {
    let (active, held, prompt, swallowing) = {
        let s = gui.0.borrow();
        let l = &s.client_script;
        (
            l.active(),
            l.holds_any() || l.interactive,
            !l.engine.is_empty(),
            l.escape_swallow,
        )
    };
    match ev.action {
        SI_MAKE => {
            let shift = normalise_modifiers(ev.modifier) & SI_SHIFT != 0;
            if active && shift {
                // The panic key ends the realm for the connection only: the server is asked again next time.
                turn_off(gui, LayerAction::Stop);
            } else if active && held {
                release_holds(gui, Release::Closed);
            } else if !prompt {
                return false;
            }
            gui.0.borrow_mut().client_script.escape_swallow = true;
            true
        }
        SI_BREAK | SI_REPEAT => {
            if ev.action == SI_BREAK {
                gui.0.borrow_mut().client_script.escape_swallow = false;
            }
            swallowing
        }
        _ => false,
    }
}

/// A key while an engine prompt is up: to the key responder of the top prompt (the control in it clicked last that takes
/// keys), else nowhere. The game gets only the releases (and not the repeats) of the keys it had the press of, so none stay
/// held.
fn prompt_key(vm: &mut Vm, gui: &Gui, ev: &InputEvent) -> Result<bool, VmError> {
    let Ok(key_code) = u8::try_from(ev.obj_inst) else {
        return Ok(true);
    };
    let had = {
        let l = &mut gui.0.borrow_mut().client_script;
        match ev.action {
            SI_MAKE => {
                l.prompt_keys.insert(key_code);
                true
            }
            SI_BREAK => l.prompt_keys.take(key_code),
            _ => l.prompt_keys.contains(key_code),
        }
    };
    if !had {
        return Ok(ev.action != SI_BREAK);
    }
    let top = gui.0.borrow().client_script.engine.last().copied();
    let responder = top
        .and_then(|t| gui.with(t, |c| c.first_responder).flatten())
        .filter(|f| vm.objects().contains(*f) && gui.is_awake(*f));
    if let Some(f) = responder {
        let event = GuiEvent {
            ascii: ev.ascii,
            modifier: ev.modifier,
            key_code,
            action: ev.action,
            ..GuiEvent::default()
        };
        match ev.action {
            SI_MAKE | SI_REPEAT => {
                input::on_key_down(vm, gui, f, event)?;
            }
            SI_BREAK => {
                input::on_key_up_at(vm, gui, f, event, 0)?;
            }
            _ => {}
        }
    }
    Ok(true)
}

/// Whether the key is text or navigation, the only keys the realm's text field takes.
fn is_text_or_navigation(ev: &InputEvent) -> bool {
    matches!(
        ev.obj_inst,
        KEY_BACKSPACE
            | KEY_DELETE
            | KEY_LEFT
            | KEY_RIGHT
            | KEY_HOME
            | KEY_END
            | KEY_RETURN
            | KEY_NUMPADENTER
    ) || (ev.ascii >= 32 && ev.ascii != 127)
}

/// A key for the realm's focused text field. Only the releases of the presses the field took are kept from the game, and only
/// their repeats go to the field. The field loses the focus when the cursor is off.
fn key(vm: &mut Vm, gui: &Gui, ev: &InputEvent) -> Result<bool, VmError> {
    let Ok(key_code) = u8::try_from(ev.obj_inst) else {
        return Ok(false);
    };
    if ev.action == SI_BREAK {
        return Ok(gui.0.borrow_mut().client_script.field_keys.take(key_code));
    }
    let (focus, cursor_on, had) = {
        let s = gui.0.borrow();
        let l = &s.client_script;
        (l.focus, s.cursor_on, l.field_keys.contains(key_code))
    };
    let Some(focus) = focus.filter(|f| vm.objects().contains(*f)) else {
        return Ok(false);
    };
    if !cursor_on {
        gui.0.borrow_mut().client_script.blur();
        return Ok(false);
    }
    if !server_takes_input(vm, gui) || !is_text_or_navigation(ev) {
        return Ok(false);
    }
    match ev.action {
        SI_MAKE => {}
        SI_REPEAT if had => {}
        _ => return Ok(false),
    }
    let event = GuiEvent {
        ascii: ev.ascii,
        modifier: ev.modifier,
        key_code,
        action: ev.action,
        ..GuiEvent::default()
    };
    let taken = custom::key_down(vm, gui, focus, event)
        .transpose()?
        .unwrap_or(false);
    if taken && ev.action == SI_MAKE {
        gui.0.borrow_mut().client_script.field_keys.insert(key_code);
    }
    Ok(taken)
}

fn mouse(vm: &mut Vm, gui: &Gui, ev: &InputEvent) -> Result<bool, VmError> {
    match ev.obj_type {
        SI_BUTTON => button(vm, gui, ev),
        SI_ZAXIS => {
            let pt = cursor(gui);
            if engine_hit(vm, gui, pt).is_some() {
                return Ok(true);
            }
            let notch = if ev.value >= 0.0 {
                Notch::Up
            } else {
                Notch::Down
            };
            Ok(server_hit(vm, gui, pt).is_some_and(|t| wheel(vm, gui, t.id, notch)))
        }
        _ => Ok(false),
    }
}

fn button(vm: &mut Vm, gui: &Gui, ev: &InputEvent) -> Result<bool, VmError> {
    let left = ev.obj_inst == KEY_BUTTON0;
    if ev.action != SI_MAKE {
        // A release goes to the control the press went to.
        let captured = gui.0.borrow_mut().client_script.captured.take();
        let Some(id) = captured.filter(|c| vm.objects().contains(*c)) else {
            return Ok(false);
        };
        let target = Target {
            id,
            origin: origin(vm, gui, id),
        };
        deliver(vm, gui, target, Pointer::Up, ev)?;
        return Ok(true);
    }
    let target = target_at(vm, gui, cursor(gui));
    {
        // A press anywhere but on the focused field (the game's controls, the chip, a prompt, another control of the
        // realm's) takes the focus away, so typing meant for something else never reaches the realm.
        let l = &mut gui.0.borrow_mut().client_script;
        if l.focus.is_some() && target.map(|t| t.id) != l.focus {
            l.blur();
        }
    }
    let Some(target) = target else {
        return Ok(false);
    };
    if left {
        gui.0.borrow_mut().client_script.captured = Some(target.id);
        deliver(vm, gui, target, Pointer::Down, ev)?;
    }
    Ok(true)
}

/// After the canvas took a pointer move: the layers' hover, enter and leave, and a drag of the control the press went to.
pub(crate) fn after_canvas(vm: &mut Vm, gui: &Gui, ev: &InputEvent) -> Result<(), VmError> {
    if ev.device != MOUSE_DEVICE || !matches!(ev.obj_type, SI_XAXIS | SI_YAXIS) {
        return Ok(());
    }
    {
        let s = gui.0.borrow();
        if !(s.client_script.active() || !s.client_script.engine.is_empty()) || !s.cursor_on {
            return Ok(());
        }
    }
    let pt = cursor(gui);
    let (captured, hover) = {
        let s = gui.0.borrow();
        (s.client_script.captured, s.client_script.hover)
    };
    if let Some(id) = captured.filter(|c| vm.objects().contains(*c)) {
        let target = Target {
            id,
            origin: origin(vm, gui, id),
        };
        return deliver(vm, gui, target, Pointer::Dragged, ev);
    }
    let over = target_at(vm, gui, pt);
    if over.map(|t| t.id) != hover {
        if let Some(old) = hover.filter(|h| vm.objects().contains(*h)) {
            let target = Target {
                id: old,
                origin: origin(vm, gui, old),
            };
            deliver(vm, gui, target, Pointer::Leave, ev)?;
        }
        gui.0.borrow_mut().client_script.hover = over.map(|t| t.id);
        if let Some(target) = over {
            deliver(vm, gui, target, Pointer::Enter, ev)?;
        }
    }
    if let Some(target) = over {
        deliver(vm, gui, target, Pointer::Move, ev)?;
    }
    Ok(())
}
