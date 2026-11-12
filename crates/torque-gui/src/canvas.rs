//! `createCanvas` and `GuiCanvas`: the content and dialog layers, the cursor. Every method acts on the one canvas (TGE's console
//! methods use the global `Canvas`, whatever object they are called on).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/core/guiCanvas.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Native, NativeCall, Vm, VmError};

use crate::control::{self, children, is_a};
use crate::{Gui, MAX_CHILDREN_WALKED, atoi, done};

/// The window's size before the preferences say otherwise (1280 by 720, 16:9; a game's extension may name another,
/// [`crate::GuiExtension::default_canvas_size`]).
pub const DEFAULT_CANVAS_SIZE: (i32, i32) = (1280, 720);

/// The largest window side taken from `$pref::Video::resolution` (a hostile preference falls back to the default).
const MAX_CANVAS_SIDE: i32 = 16_384;

/// The window size `$pref::Video::resolution` asks for (`strtok(" x")`, width then height, `default`'s when absent),
/// else `default` for a size that is not taken.
pub fn resolution(vm: &Vm, default: (i32, i32)) -> (i32, i32) {
    let text = vm.global("$pref::Video::resolution").unwrap_or_default();
    let mut words = text
        .split(|b| *b == b' ' || *b == b'x')
        .filter(|w| !w.is_empty())
        .map(atoi);
    match (words.next(), words.next()) {
        (None, _) => default,
        (Some(w), None) if (1..=MAX_CANVAS_SIDE).contains(&w) => (w, default.1),
        (Some(w), Some(h))
            if (1..=MAX_CANVAS_SIDE).contains(&w) && (1..=MAX_CANVAS_SIDE).contains(&h) =>
        {
            (w, h)
        }
        _ => default,
    }
}

/// `createCanvas(title)`, headless: the awake `GuiCanvas` named `Canvas`, sized by `$pref::Video::resolution` (else the
/// extension's [`crate::GuiExtension::default_canvas_size`]; the
/// extension's [`crate::GuiExtension::before_canvas`] runs first). True once the canvas exists; a second call is refused.
pub fn create_canvas(vm: &mut Vm, gui: &Gui) -> Result<bool, VmError> {
    if gui.canvas().is_some_and(|c| vm.objects().contains(c)) {
        return Ok(false);
    }
    let forced = gui.0.borrow().forced_size;
    let size = forced.unwrap_or_else(|| resolution(vm, gui.ext().default_canvas_size()));
    gui.set_canvas_size(size);
    gui.ext().before_canvas(vm)?;
    vm.eval(b"new GuiCanvas(Canvas);")?;
    let Some(canvas) = vm
        .objects()
        .find(b"Canvas")
        .filter(|c| is_a(vm, *c, "GuiCanvas"))
    else {
        return Ok(false);
    };
    let mut state = gui.0.borrow_mut();
    state.canvas = Some(canvas);
    if let Some(c) = state.controls.get_mut(&canvas) {
        // `GuiCanvas::GuiCanvas`: awake from the start.
        c.awake = true;
    }
    Ok(true)
}

/// The most controls one hit test visits (a wide hostile GUI is searched to here and the best hit so far is the answer).
pub const MAX_HIT_VISITS: u32 = 1 << 16;

/// `GuiControl::findHitControl` (TGE 1.3): the children from the last (the top) down; the first visible one whose rectangle holds
/// the point is searched in turn, and its hit is the answer when that control's profile is `modal`; else the control itself. Every
/// visit counts against [`MAX_HIT_VISITS`] (shared by the whole search) and is charged as work; at the cap the search ends with the
/// control it is in.
fn find_hit(
    vm: &Vm,
    gui: Option<&Gui>,
    id: ObjId,
    pt: (i32, i32),
    depth: usize,
    seen: &mut u32,
) -> ObjId {
    if depth >= crate::MAX_TREE_DEPTH {
        return id;
    }
    for child in children(vm, id).into_iter().rev() {
        if *seen >= MAX_HIT_VISITS {
            return id;
        }
        *seen += 1;
        vm.charge_work(16);
        if !vm.objects().contains(child)
            || !is_a(vm, child, "GuiControl")
            || !control::flag(vm, child, b"visible")
        {
            continue;
        }
        let (pos, ext) = control::bounds(vm, child);
        let inside = pt.0 >= pos.0
            && pt.1 >= pos.1
            && pt.0 < pos.0.wrapping_add(ext.0)
            && pt.1 < pos.1.wrapping_add(ext.1);
        if !inside {
            continue;
        }
        let local = (pt.0.wrapping_sub(pos.0), pt.1.wrapping_sub(pos.1));
        // GuiScrollCtrl's `findHitControl`: off the content area (on a bar or the border) the scroll control itself.
        let hit = match gui {
            Some(g)
                if is_a(vm, child, "GuiScrollCtrl")
                    && !crate::scroll::hits_content(vm, g, child, local) =>
            {
                child
            }
            _ => find_hit(vm, gui, child, local, depth + 1, seen),
        };
        let modal = vm
            .objects()
            .field(hit, b"profile", b"")
            .and_then(|p| vm.objects().find(&p))
            .is_some_and(|p| control::flag(vm, p, b"modal"));
        if modal {
            return hit;
        }
    }
    id
}

/// The control `findHitControl` finds under `pt` from `root`, visiting at most `cap` controls (and how many it visited).
pub fn hit_test(vm: &Vm, root: ObjId, pt: (i32, i32), cap: u32) -> ObjId {
    let mut seen = MAX_HIT_VISITS.saturating_sub(cap);
    find_hit(vm, None, root, pt, 0, &mut seen)
}

/// [`hit_test`] with the classes' own hit tests (GuiScrollCtrl's content area), which need the GUI's state.
pub fn hit_test_gui(vm: &Vm, gui: &Gui, root: ObjId, pt: (i32, i32), cap: u32) -> ObjId {
    let mut seen = MAX_HIT_VISITS.saturating_sub(cap);
    find_hit(vm, Some(gui), root, pt, 0, &mut seen)
}

/// The window's mouse moved to `pt` (through the canvas's dispatch, [`crate::input::process_mouse_move`]): the cursor
/// follows, the control under it is entered (a button's `mMouseOver`, which its drawing shows, and its script `onMouseEnter`), a
/// captured control is dragged. A callback's error is returned.
pub fn mouse_move(vm: &mut Vm, gui: &Gui, pt: (i32, i32)) -> Option<VmError> {
    crate::input::host_mouse_move(vm, gui, pt, 0)
}

/// The first awake, visible control whose class name holds "Button" and whose `command` holds `part`, depth first from the control
/// named `root`: its screen position and extent (a test driver's way to a button).
pub fn find_button(
    vm: &Vm,
    gui: &Gui,
    root: &[u8],
    part: &[u8],
) -> Option<((i32, i32), (i32, i32))> {
    fn walk(
        vm: &Vm,
        gui: &Gui,
        id: ObjId,
        part: &[u8],
        depth: usize,
        seen: &mut usize,
    ) -> Option<ObjId> {
        if depth > crate::MAX_TREE_DEPTH || *seen >= 4000 {
            return None;
        }
        *seen += 1;
        if !is_a(vm, id, "GuiControl") {
            return None;
        }
        let class = vm
            .objects()
            .get(id)
            .map(|o| vm.objects().classes.name(o.class()).to_vec())?;
        let command = vm.objects().field(id, b"command", b"").unwrap_or_default();
        if class.windows(6).any(|w| w == b"Button")
            && (part.is_empty() || command.windows(part.len()).any(|w| w == part))
            && gui.is_awake(id)
            && control::flag(vm, id, b"visible")
        {
            return Some(id);
        }
        children(vm, id)
            .into_iter()
            .find_map(|c| walk(vm, gui, c, part, depth + 1, seen))
    }
    let root = vm.objects().find(root)?;
    let mut seen = 0;
    let found = walk(vm, gui, root, part, 0, &mut seen)?;
    Some((
        control::screen_position(vm, found),
        control::point(vm, found, b"extent"),
    ))
}

/// The control named `name` (a GUI control the model knows): its screen position and extent.
pub fn control_rect(vm: &Vm, gui: &Gui, name: &[u8]) -> Option<((i32, i32), (i32, i32))> {
    let id = vm.objects().find(name).filter(|i| gui.is_control(*i))?;
    Some((
        control::screen_position(vm, id),
        control::point(vm, id, b"extent"),
    ))
}

/// A focused control: its class, name, screen position and extent.
pub type Focus = (String, String, (i32, i32), (i32, i32));

/// The canvas's first responder: its class and name, screen position and extent.
pub fn focus(vm: &Vm, gui: &Gui) -> Option<Focus> {
    let id = crate::input::canvas_first_responder(gui).filter(|i| vm.objects().contains(*i))?;
    let o = vm.objects().get(id)?;
    let class = String::from_utf8_lossy(vm.objects().classes.name(o.class())).into_owned();
    let name = o
        .name()
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default();
    Some((
        class,
        name,
        control::screen_position(vm, id),
        control::point(vm, id, b"extent"),
    ))
}

fn find_control(vm: &Vm, gui: &Gui, text: &[u8]) -> Option<ObjId> {
    vm.objects().find(text).filter(|id| gui.is_control(*id))
}

fn invalid_control(vm: &mut Vm, c: &NativeCall) {
    vm.print(
        format!(
            "{}(): Invalid control: {}",
            String::from_utf8_lossy(c.arg(0)),
            String::from_utf8_lossy(c.arg(2))
        )
        .as_bytes(),
    );
}

/// The canvas's member at `index`.
fn member(vm: &Vm, canvas: ObjId, index: usize) -> Option<ObjId> {
    vm.objects().members(canvas).and_then(|m| m.get(index))
}

/// The layer of a control (0 for one the model does not know).
fn layer(gui: &Gui, id: ObjId) -> i32 {
    gui.layer(id)
}

/// Moves `id` to just before `target` in `set` (`SimSet::reOrder`): every member from `target` on, but `id`, goes to the back in turn.
fn reorder_before(vm: &mut Vm, set: ObjId, id: ObjId, target: ObjId) {
    let members = children(vm, set);
    let Some(at) = members.iter().position(|m| *m == target) else {
        return;
    };
    for m in members[at..].iter().filter(|m| **m != id) {
        vm.objects_mut().move_member(set, *m, false);
    }
}

/// `GuiCanvas::setContentControl` (TGE 1.4's loop): walks the canvas's children from the front and moves each one it meets
/// to GuiGroup (sleeping it); it steps past the new content and every dialog of a layer other than 0 before moving it, so the child
/// after either is passed over and stays. Then the content is added (woken) and put first.
pub fn set_content(vm: &mut Vm, gui: &Gui, content: ObjId) -> Result<(), VmError> {
    let Some(canvas) = gui.canvas() else {
        return Ok(());
    };
    let gui_group = vm.objects().find(b"GuiGroup");
    let mut index = 0;
    let mut rounds = 0;
    while let Some(ctrl) = member(vm, canvas, index) {
        rounds += 1;
        if rounds > MAX_CHILDREN_WALKED {
            break;
        }
        if ctrl == content || layer(gui, ctrl) != 0 {
            index += 1;
        }
        control::remove_child(vm, gui, canvas, ctrl)?;
        if let Some(group) = gui_group {
            vm.objects_mut().add(group, ctrl);
        }
    }
    let first = member(vm, canvas, 0);
    if first != Some(content) {
        control::add_child(vm, gui, canvas, content)?;
        if vm.objects().members(canvas).is_some_and(|m| m.len() >= 2) {
            vm.objects_mut().move_member(canvas, content, true);
        }
    }
    // The first responder: the content's first tabable, taken as it is; the accelerators from the top down to a modal child; the
    // control under the cursor again.
    let first = crate::input::find_first_tabable(vm, gui, content);
    take_responder(vm, gui, first)?;
    crate::input::rebuild_accelerators(vm, gui, true);
    crate::input::refresh_mouse_control(vm, gui)
}

/// The canvas's first responder becomes `new` as setContent, pushDialog and popDialog set it (no `canKeyFocus` test); the old
/// one, when another, gets `onLoseFirstResponder`.
fn take_responder(vm: &mut Vm, gui: &Gui, new: Option<ObjId>) -> Result<(), VmError> {
    let old = std::mem::replace(&mut gui.0.borrow_mut().input.first_responder, new);
    if let Some(c) = gui.canvas() {
        gui.with_mut(c, |s| s.first_responder = new);
    }
    match old {
        Some(o) if Some(o) != new && vm.objects().contains(o) => {
            crate::input::on_lose_first_responder(vm, gui, o)
        }
        _ => Ok(()),
    }
}

/// `GuiCanvas::pushDialogControl`: the layer, the add (which wakes it; a dialog already on the canvas keeps its place), then before
/// the first child of a higher layer.
pub fn push_dialog(vm: &mut Vm, gui: &Gui, dialog: ObjId, layer_no: i32) -> Result<(), VmError> {
    let Some(canvas) = gui.canvas() else {
        return Ok(());
    };
    if let Some(c) = gui.0.borrow_mut().controls.get_mut(&dialog) {
        c.layer = layer_no;
    }
    control::add_child(vm, gui, canvas, dialog)?;
    let higher = children(vm, canvas)
        .into_iter()
        .find(|m| layer(gui, *m) > layer_no);
    if let Some(target) = higher {
        reorder_before(vm, canvas, dialog, target);
    }
    // `onDialogPush` (no class of this crate has one), the dialog's first tabable, the accelerators from the top child, the
    // control under the cursor.
    let first = crate::input::find_first_tabable(vm, gui, dialog);
    take_responder(vm, gui, first)?;
    crate::input::rebuild_accelerators(vm, gui, false);
    crate::input::refresh_mouse_control(vm, gui)
}

/// `GuiCanvas::popDialogControl(gui)`: a control not on the canvas is left alone; the top one when none is named. It sleeps and goes
/// to GuiGroup.
pub fn pop_dialog(vm: &mut Vm, gui: &Gui, dialog: Option<ObjId>) -> Result<(), VmError> {
    let Some(canvas) = gui.canvas() else {
        return Ok(());
    };
    let members = children(vm, canvas);
    let target = match dialog {
        Some(d) if members.contains(&d) => d,
        Some(_) => return Ok(()),
        None => match members.last() {
            Some(last) => *last,
            None => return Ok(()),
        },
    };
    control::remove_child(vm, gui, canvas, target)?;
    if let Some(group) = vm.objects().find(b"GuiGroup") {
        vm.objects_mut().add(group, target);
    }
    // The new top child's remembered first responder, the accelerators from the top, the control under the cursor.
    let top = children(vm, canvas).last().copied();
    let remembered = top.and_then(|t| gui.with(t, |c| c.first_responder).flatten());
    take_responder(vm, gui, remembered)?;
    crate::input::rebuild_accelerators(vm, gui, false);
    crate::input::refresh_mouse_control(vm, gui)
}

fn reg(
    vm: &mut Vm,
    gui: &Gui,
    name: &str,
    min: i32,
    max: i32,
    usage: &str,
    f: impl Fn(&mut Vm, &Gui, &NativeCall) -> Native + 'static,
) {
    let gui = gui.clone();
    vm.register_in("GuiCanvas", name, min, max, usage, move |vm, c| {
        f(vm, &gui, c)
    });
}

pub(crate) fn register(vm: &mut Vm, gui: &Gui) {
    {
        let gui = gui.clone();
        vm.register(
            "createCanvas",
            2,
            2,
            "(string windowTitle)",
            move |vm, _| match create_canvas(vm, &gui) {
                Ok(made) => Native::Bool(made),
                Err(e) => Native::Fail(e),
            },
        );
    }
    reg(vm, gui, "getContent", 2, 2, "()", |vm, gui, _| {
        let content = gui.canvas().and_then(|c| member(vm, c, 0));
        Native::Int(content.map_or(-1, |id| id as i32))
    });
    reg(
        vm,
        gui,
        "setContent",
        3,
        3,
        "(GuiControl ctrl)",
        |vm, gui, c| {
            if c.arg(2).is_empty() {
                return Native::Void;
            }
            match find_control(vm, gui, c.arg(2)) {
                Some(content) => done(set_content(vm, gui, content)),
                None => {
                    invalid_control(vm, c);
                    Native::Void
                }
            }
        },
    );
    reg(
        vm,
        gui,
        "pushDialog",
        3,
        4,
        "(GuiControl ctrl, int layer)",
        |vm, gui, c| match find_control(vm, gui, c.arg(2)) {
            Some(dialog) => {
                let layer = if c.argc() == 4 { c.int(3) } else { 0 };
                done(push_dialog(vm, gui, dialog, layer))
            }
            None => {
                invalid_control(vm, c);
                Native::Void
            }
        },
    );
    reg(
        vm,
        gui,
        "popDialog",
        2,
        3,
        "(GuiControl ctrl=NULL)",
        |vm, gui, c| {
            if c.argc() == 3 {
                match find_control(vm, gui, c.arg(2)) {
                    Some(dialog) => done(pop_dialog(vm, gui, Some(dialog))),
                    None => {
                        invalid_control(vm, c);
                        Native::Void
                    }
                }
            } else {
                done(pop_dialog(vm, gui, None))
            }
        },
    );
    reg(vm, gui, "popLayer", 2, 3, "(int layer)", |vm, gui, c| {
        let layer_no = if c.argc() == 3 { c.int(2) } else { 0 };
        let Some(canvas) = gui.canvas() else {
            return Native::Void;
        };
        let top = children(vm, canvas)
            .into_iter()
            .rev()
            .find(|m| layer(gui, *m) == layer_no);
        // TGE 1.4: the walk ends on the first child when no layer matches, and that one is popped.
        let target = top.or_else(|| member(vm, canvas, 0));
        match target {
            Some(t) => done(pop_dialog(vm, gui, Some(t))),
            None => Native::Void,
        }
    });
    reg(vm, gui, "cursorOn", 2, 2, "", |_, gui, _| {
        let mut g = gui.0.borrow_mut();
        g.cursor_on = true;
        g.cursor_calls += 1;
        Native::Void
    });
    // The cursor off, and the mouse control (`mMouseControl`) dropped.
    reg(vm, gui, "cursorOff", 2, 2, "", |_, gui, _| {
        let mut g = gui.0.borrow_mut();
        g.cursor_on = false;
        g.cursor_calls += 1;
        g.input.mouse_control = None;
        Native::Void
    });
    reg(vm, gui, "isCursorOn", 2, 2, "", |_, gui, _| {
        Native::Bool(gui.0.borrow().cursor_on)
    });
    reg(vm, gui, "showCursor", 2, 2, "", |_, gui, _| {
        gui.0.borrow_mut().show_cursor = true;
        Native::Void
    });
    reg(vm, gui, "hideCursor", 2, 2, "", |_, gui, _| {
        gui.0.borrow_mut().show_cursor = false;
        Native::Void
    });
    reg(
        vm,
        gui,
        "setCursor",
        3,
        3,
        "(bool visible)",
        |vm, gui, c| {
            let text = c.arg(2);
            let cursor = if text.is_empty() {
                None
            } else {
                match vm
                    .objects()
                    .find(text)
                    .filter(|id| is_a(vm, *id, "GuiCursor"))
                {
                    Some(id) => Some(id),
                    None => {
                        let name = String::from_utf8_lossy(text);
                        let m = crate::ext::GuiMessage::InvalidCursor { name: &name };
                        vm.print(crate::message_text(gui, &m).as_bytes());
                        return Native::Void;
                    }
                }
            };
            gui.0.borrow_mut().cursor = cursor;
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        "renderFront",
        3,
        3,
        "(bool enable)",
        |_, gui, c| {
            gui.0.borrow_mut().render_front = c.boolean(2);
            Native::Void
        },
    );
    reg(vm, gui, "repaint", 2, 2, "()", |vm, gui, _| {
        gui.frame(vm);
        Native::Void
    });
    reg(vm, gui, "reset", 2, 2, "()", |_, _, _| Native::Void);
    reg(vm, gui, "getCursorPos", 2, 2, "()", |_, gui, _| {
        let (x, y) = gui.0.borrow().cursor_pos;
        Native::Str(format!("{x} {y}").into_bytes())
    });
    reg(
        vm,
        gui,
        "setCursorPos",
        3,
        4,
        "(Point2I pos)",
        |_, gui, c| {
            let pos = if c.argc() == 4 {
                (c.int(2), c.int(3))
            } else {
                let text = c.arg(2);
                let mut words = text
                    .split(|b| *b == b' ')
                    .filter(|w| !w.is_empty())
                    .map(atoi);
                (words.next().unwrap_or(0), words.next().unwrap_or(0))
            };
            gui.0.borrow_mut().cursor_pos = pos;
            Native::Void
        },
    );
    // `GuiCanvas::tabNext`.
    reg(
        vm,
        gui,
        "tabNext",
        2,
        2,
        "",
        |vm, gui, _| match crate::input::tab_next(vm, gui) {
            Ok(_) => Native::Void,
            Err(e) => Native::Fail(e),
        },
    );
    reg(vm, gui, "canTabFocus", 3, 3, "", |_, _, _| Native::Void);
}

#[cfg(test)]
#[path = "canvas_tests.rs"]
mod tests;
