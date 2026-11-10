//! `GuiControl`: the behaviour every GUI class runs (`onAdd`, `onRemove`), the tree walks (`awaken`, `sleep`), the layout
//! (`resize`, `parentResized`) and the base class's methods. See the crate documentation for what is modelled.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `gui/core/guiControl.cpp` (Copyright (c)
//! 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.), `gui/core/guiTypes.cpp` (Copyright (c) 2012 GarageGames, LLC);
//! the code here is this crate's own.

use std::rc::Rc;

use torque_ts_vm::classes::EngineClass;
use torque_ts_vm::fields::{FieldType, Value};
use torque_ts_vm::sim::{ObjId, Objects, SimObject};
use torque_ts_vm::vm::{Native, NativeCall, Vm, VmError};

use crate::{
    Control, Gui, MAX_CHILDREN_WALKED, MAX_TREE_DEPTH, arg, atob, atoi, classes, done, this,
};

/// What one visit of a walk costs (`Vm::charge_work` units).
const VISIT_WORK: u64 = 64;

/// The classes whose constructor sets `mActive` (GuiControl's constructor leaves it false): GuiTextCtrl (so its subclasses:
/// the text edit, the window, the popup), GuiButtonBaseCtrl, GuiArrayCtrl (so the text list and the console), GuiSliderCtrl
/// and GuiMLTextCtrl. GuiScrollCtrl's, GuiBitmapCtrl's, GuiProgressCtrl's and GuiConsoleTextCtrl's do not; an extension
/// names its own classes ([`crate::GuiExtension::starts_active`]).
const ACTIVE_CLASSES: [&str; 5] = [
    "GuiTextCtrl",
    "GuiButtonBaseCtrl",
    "GuiArrayCtrl",
    "GuiSliderCtrl",
    "GuiMLTextCtrl",
];

/// The class name test of a registered object (`is_a` by name; false when either is unknown).
pub fn is_a(vm: &Vm, id: ObjId, class: &str) -> bool {
    let objects = vm.objects();
    let (Some(object), Some(ancestor)) = (objects.get(id), objects.classes.find(class.as_bytes()))
    else {
        return false;
    };
    objects.classes.is_a(object.class(), ancestor)
}

/// The children of a control, in order (at most [`MAX_CHILDREN_WALKED`]).
pub fn children(vm: &Vm, id: ObjId) -> Vec<ObjId> {
    // The copy is paid for (four bytes an id): a walk over a control a script filled with children costs it.
    let len = vm
        .objects()
        .members(id)
        .map_or(0, |m| m.len().min(MAX_CHILDREN_WALKED));
    vm.charge_work((len as u64).saturating_mul(4));
    vm.objects().members(id).map_or_else(Vec::new, |m| {
        m.as_slice()
            .iter()
            .copied()
            .take(MAX_CHILDREN_WALKED)
            .collect()
    })
}

/// The parent of a control when it is a GUI control (the GuiGroup and other plain groups are not).
pub fn parent(vm: &Vm, id: ObjId) -> Option<ObjId> {
    let group = vm.objects().get(id)?.group()?;
    is_a(vm, group, "GuiControl").then_some(group)
}

/// Two integers of a `Point2I` field.
pub fn point(vm: &Vm, id: ObjId, field: &[u8]) -> (i32, i32) {
    if let Some(Value::Ints2([x, y])) = vm.objects().static_value(id, field) {
        return (*x, *y);
    }
    let text = vm.objects().field(id, field, b"").unwrap_or_default();
    let mut words = text
        .split(|b| *b == b' ')
        .filter(|w| !w.is_empty())
        .map(atoi);
    (words.next().unwrap_or(0), words.next().unwrap_or(0))
}

/// `mBounds`: (position, extent).
pub fn bounds(vm: &Vm, id: ObjId) -> ((i32, i32), (i32, i32)) {
    (point(vm, id, b"position"), point(vm, id, b"extent"))
}

fn set_point(vm: &mut Vm, id: ObjId, field: &[u8], p: (i32, i32)) {
    vm.set_object_field(id, field, b"", format!("{} {}", p.0, p.1).as_bytes());
}

/// `mBounds.set(position, extent)`.
pub(crate) fn set_bounds(vm: &mut Vm, id: ObjId, position: (i32, i32), extent: (i32, i32)) {
    set_point(vm, id, b"position", position);
    set_point(vm, id, b"extent", extent);
}

fn field_text(vm: &Vm, id: ObjId, field: &[u8]) -> Vec<u8> {
    vm.objects().field(id, field, b"").unwrap_or_default()
}

/// A Bool field.
pub fn flag(vm: &Vm, id: ObjId, field: &[u8]) -> bool {
    if let Some(Value::Bool(on)) = vm.objects().static_value(id, field) {
        return *on;
    }
    atob(&field_text(vm, id, field))
}

/// The virtual `resize`: GuiScrollCtrl's (then `computeSizes`), GuiWindowCtrl's and GuiMLTextCtrl's by class
/// (the classes' own), else `GuiControl::resize`: the extent clamped to `minExtent`, every child's
/// `parentResized(old, new)`, the bounds, then the parent's `childResized` (a GuiScrollCtrl parent recomputes its sizes). Runs no
/// script.
pub fn resize(vm: &mut Vm, gui: &Gui, id: ObjId, position: (i32, i32), extent: (i32, i32)) {
    resize_at(vm, gui, id, position, extent, 0);
}

pub(crate) fn resize_at(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    position: (i32, i32),
    extent: (i32, i32),
    depth: usize,
) {
    if depth > MAX_TREE_DEPTH || !vm.charge_work(VISIT_WORK) {
        return;
    }
    if classes::resize(vm, gui, id, position, extent, depth)
        || crate::custom::resize(vm, gui, id, position, extent, depth)
    {
        return;
    }
    base_resize(vm, gui, id, position, extent, depth);
}

/// `GuiControl::resize` itself, for the classes whose own `resize` calls it.
pub(crate) fn base_resize(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    position: (i32, i32),
    extent: (i32, i32),
    depth: usize,
) {
    let min = point(vm, id, b"minExtent");
    let actual = (extent.0.max(min.0), extent.1.max(min.1));
    let old = point(vm, id, b"extent");
    for child in children(vm, id) {
        if vm.objects().contains(child) && is_a(vm, child, "GuiControl") {
            parent_resized_at(vm, gui, child, old, actual, depth + 1);
        }
    }
    set_bounds(vm, id, position, actual);
    // The parent's `childResized`: only GuiScrollCtrl's does anything (`computeSizes`).
    if let Some(p) = parent(vm, id)
        && is_a(vm, p, "GuiScrollCtrl")
    {
        crate::scroll::compute_sizes(vm, gui, p);
    }
}

/// The virtual `parentResized` (GuiControl's: TGE 1.4's rules): the control's new bounds after its parent's
/// extent went from `old` to `new`, by its `horizSizing` and `vertSizing`, then [`resize`]. GuiTextEditCtrl and GuiMLTextCtrl have
/// their own.
pub fn parent_resized(vm: &mut Vm, gui: &Gui, id: ObjId, old: (i32, i32), new: (i32, i32)) {
    parent_resized_at(vm, gui, id, old, new, 0);
}

pub(crate) fn parent_resized_at(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    old: (i32, i32),
    new: (i32, i32),
    depth: usize,
) {
    if classes::parent_resized(vm, gui, id, old, new, depth) {
        return;
    }
    base_parent_resized(vm, gui, id, old, new, depth);
}

/// `GuiControl::parentResized` itself.
pub(crate) fn base_parent_resized(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    old: (i32, i32),
    new: (i32, i32),
    depth: usize,
) {
    let ((mut px, mut py), (mut ex, mut ey)) = bounds(vm, id);
    let (width, height) = (ex, ey);
    let dx = new.0.wrapping_sub(old.0);
    let dy = new.1.wrapping_sub(old.1);
    match field_text(vm, id, b"horizSizing").as_slice() {
        b"center" => px = new.0.wrapping_sub(width) >> 1,
        b"width" => ex = ex.wrapping_add(dx),
        b"left" => px = px.wrapping_add(dx),
        b"relative" if old.0 != 0 => {
            let left = scale(px, new.0, old.0);
            let right = scale(px.wrapping_add(ex), new.0, old.0);
            px = left;
            ex = right.wrapping_sub(left);
        }
        _ => {}
    }
    match field_text(vm, id, b"vertSizing").as_slice() {
        b"center" => py = new.1.wrapping_sub(height) >> 1,
        b"height" => ey = ey.wrapping_add(dy),
        b"top" => py = py.wrapping_add(dy),
        b"relative" if old.1 != 0 => {
            let top = scale(py, new.1, old.1);
            let bottom = scale(py.wrapping_add(ey), new.1, old.1);
            py = top;
            ey = bottom.wrapping_sub(top);
        }
        _ => {}
    }
    resize_at(vm, gui, id, (px, py), (ex, ey), depth);
}

/// `(a * b) / c` in S32 as the engine computes it (the product wraps; C division truncates).
fn scale(a: i32, b: i32, c: i32) -> i32 {
    a.wrapping_mul(b).checked_div(c).unwrap_or(0)
}

/// `ControlIsChild`: whether `child` is below `id` (a bounded walk up its parents).
pub(crate) fn is_ancestor(vm: &Vm, id: ObjId, child: ObjId) -> bool {
    let mut at = parent(vm, child);
    for _ in 0..crate::MAX_TREE_DEPTH {
        match at {
            Some(p) if p == id => return true,
            Some(p) => at = parent(vm, p),
            None => return false,
        }
    }
    false
}

/// The control's position on the screen: its own plus every ancestor's (`localToGlobalCoord(0, 0)`).
pub fn screen_position(vm: &Vm, id: ObjId) -> (i32, i32) {
    let mut at = point(vm, id, b"position");
    let mut walk = vm.objects().get(id).and_then(|o| o.group());
    let mut depth = 0;
    while let Some(group) = walk {
        if depth > MAX_TREE_DEPTH || !is_a(vm, group, "GuiControl") {
            break;
        }
        let p = point(vm, group, b"position");
        at = (at.0.wrapping_add(p.0), at.1.wrapping_add(p.1));
        walk = vm.objects().get(group).and_then(|o| o.group());
        depth += 1;
    }
    at
}

/// `GuiControl::awaken`: every child that is not awake is woken, in order, then the control's own `onWake`.
/// The tree is re-read after every script, which may change it.
pub fn awaken(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    awaken_at(vm, gui, id, 0)
}

fn awaken_at(vm: &mut Vm, gui: &Gui, id: ObjId, depth: usize) -> Result<(), VmError> {
    if depth > MAX_TREE_DEPTH || gui.is_awake(id) || !gui.is_control(id) {
        return Ok(());
    }
    if !vm.charge_work(VISIT_WORK) {
        return Err(VmError::StepLimit);
    }
    let mut index = 0;
    while index < MAX_CHILDREN_WALKED {
        let Some(child) = vm.objects().members(id).and_then(|m| m.get(index)) else {
            break;
        };
        if !gui.is_awake(child) {
            awaken_at(vm, gui, child, depth + 1)?;
        }
        index += 1;
    }
    if gui.is_awake(id) || !vm.objects().contains(id) {
        return Ok(());
    }
    on_wake(vm, gui, id)
}

/// `GuiControl::onWake`: the profile a control without one gets, the flag, the layer, the script `onWake`.
fn on_wake(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    if field_text(vm, id, b"profile").is_empty() {
        let class = vm
            .objects()
            .get(id)
            .map(|o| vm.objects().classes.name(o.class()).to_vec())
            .unwrap_or_default();
        let mut found = false;
        // The class name up to the first "Ctrl" (none, or at 0: no try), then "Profile".
        if let Some(pos) = class.windows(4).position(|w| w == b"Ctrl")
            && pos != 0
        {
            let mut name = class[..pos].to_vec();
            name.extend_from_slice(b"Profile");
            if vm
                .objects()
                .find(&name)
                .is_some_and(|p| is_a(vm, p, "GuiControlProfile"))
            {
                vm.set_object_field(id, b"profile", b"", &name);
                found = true;
            }
        }
        if !found {
            vm.set_object_field(id, b"profile", b"", b"GuiDefaultProfile");
        }
    }
    let parent_layer = parent(vm, id)
        .filter(|p| Some(*p) != gui.canvas())
        .map(|p| gui.layer(p));
    {
        let mut state = gui.0.borrow_mut();
        let Some(control) = state.controls.get_mut(&id) else {
            return Ok(());
        };
        control.awake = true;
        if let Some(layer) = parent_layer {
            control.layer = layer;
        }
    }
    // The remembered first responder, a tabable control that takes key focus.
    crate::input::on_wake(vm, gui, id)?;
    classes::on_wake(vm, gui, id);
    if !gui.is_realm(id) {
        vm.call_method(id, b"onWake", &[])?;
    }
    // What a class's `onWake` does after `GuiControl::onWake` returned (the script's `onWake` ran inside it).
    if vm.objects().contains(id) && gui.is_control(id) {
        classes::after_wake(vm, gui, id);
        crate::custom::wake(vm, gui, id)?;
    }
    Ok(())
}

/// `GuiControl::sleep`: every awake child sleeps, in order, then the control's own `onSleep` (the script's, then the flag).
pub fn sleep(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    sleep_at(vm, gui, id, 0)
}

fn sleep_at(vm: &mut Vm, gui: &Gui, id: ObjId, depth: usize) -> Result<(), VmError> {
    if depth > MAX_TREE_DEPTH || !gui.is_awake(id) {
        return Ok(());
    }
    if !vm.charge_work(VISIT_WORK) {
        return Err(VmError::StepLimit);
    }
    let mut index = 0;
    while index < MAX_CHILDREN_WALKED {
        let Some(child) = vm.objects().members(id).and_then(|m| m.get(index)) else {
            break;
        };
        if gui.is_awake(child) {
            sleep_at(vm, gui, child, depth + 1)?;
        }
        index += 1;
    }
    if !gui.is_awake(id) {
        return Ok(());
    }
    // GuiMessageVectorCtrl's `onSleep` detaches before GuiControl's.
    crate::msgvector::on_sleep(gui, id);
    // The first responder cleared and the mouse unlocked before the script's `onSleep`.
    crate::input::on_sleep(vm, gui, id)?;
    crate::custom::sleep(vm, gui, id)?;
    let result = if gui.is_realm(id) {
        Ok(())
    } else {
        vm.call_method(id, b"onSleep", &[]).map(|_| ())
    };
    if let Some(control) = gui.0.borrow_mut().controls.get_mut(&id) {
        control.awake = false;
        // Sleeping clears the "has rendered" flag with the awake one: the script's `onRender` runs once again on the first
        // frame after the next wake (a menu that asks a server for its lists there asks again).
        control.has_rendered = false;
    }
    result
}

/// `GuiControl::addObject` of `child` into `parent` (both controls): a child already in `parent` stays; one in an awake control
/// sleeps as it leaves it (`removeObject`); it then wakes when `parent` is awake.
pub(crate) fn add_child(
    vm: &mut Vm,
    gui: &Gui,
    parent: ObjId,
    child: ObjId,
) -> Result<(), VmError> {
    let old = vm.objects().get(child).and_then(|o| o.group());
    if old == Some(parent) {
        return Ok(());
    }
    if let Some(old) = old
        && gui.is_awake(old)
        && gui.is_awake(child)
    {
        sleep(vm, gui, child)?;
    }
    vm.objects_mut().add(parent, child);
    if gui.is_awake(parent) && !gui.is_awake(child) {
        awaken(vm, gui, child)?;
    }
    // GuiScrollCtrl's `addObject` ends with `computeSizes`.
    if vm.objects().contains(parent) && is_a(vm, parent, "GuiScrollCtrl") {
        crate::scroll::compute_sizes(vm, gui, parent);
    }
    Ok(())
}

/// `GuiControl::removeObject` of `child` from `parent`: it sleeps first when `parent` is awake. False when it was not a member.
pub(crate) fn remove_child(
    vm: &mut Vm,
    gui: &Gui,
    parent: ObjId,
    child: ObjId,
) -> Result<bool, VmError> {
    if !vm.objects().is_member(parent, child) {
        return Ok(false);
    }
    if gui.is_awake(parent) && gui.is_awake(child) {
        sleep(vm, gui, child)?;
    }
    Ok(vm.objects_mut().remove(parent, child))
}

/// A control the host makes itself for a client-script realm: an object of `class` that scripts neither read nor write
/// ([`SimObject::engine_made`]), with no name, no `GuiGroup` membership and no script callbacks (`onAdd`, `onWake`, `onSleep`,
/// `onRemove` and `onRender` never run for it). The control's state is made as a script-made control's is. `None` for an
/// unknown class or when no id is free.
pub(crate) fn make_realm_control(
    vm: &mut Vm,
    gui: &Gui,
    class: &str,
    fields: &[(&[u8], &[u8], &[u8])],
) -> Option<ObjId> {
    let id = vm.add_engine_object(class.as_bytes(), fields)?;
    let mut control = Control {
        selected: -1,
        realm: true,
        ..Control::default()
    };
    control.active = crate::custom::made(vm, gui, id).unwrap_or(false);
    gui.0.borrow_mut().controls.insert(id, control);
    Some(id)
}

/// The behaviour of `GuiControl` and every subclass.
struct ControlClass(Gui);

impl EngineClass for ControlClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let mut control = Control {
            selected: -1,
            file: vm.current_file().map(<[u8]>::to_vec).unwrap_or_default(),
            ..Control::default()
        };
        if classes::has_text_state(vm, id) {
            control.text = Some(classes::initial_text(vm, id));
        }
        // A plugin control's state is made here whatever the other checks say.
        let made = crate::custom::made(vm, &self.0, id).unwrap_or(false);
        control.active = made
            || ACTIVE_CLASSES.iter().any(|c| is_a(vm, id, c))
            || self.0.ext().starts_active(vm, id);
        if is_a(vm, id, "GuiTextEditCtrl") {
            control.edit = Some(Box::new(crate::input::EditState::new()));
        }
        classes::construct(vm, id, &mut control);
        self.0.0.borrow_mut().controls.insert(id, control);
        vm.link_name_namespace(id);
        if let Some(group) = vm.objects().find(b"GuiGroup") {
            vm.objects_mut().add(group, id);
        }
        vm.call_method(id, b"onAdd", &[])?;
        Ok(true)
    }

    fn on_add_object(&self, vm: &mut Vm, group: ObjId, child: ObjId) -> Result<(), VmError> {
        // GuiControl::addObject wakes a child added to an awake parent; GuiScrollCtrl's then recomputes.
        if self.0.is_awake(group) && self.0.is_control(child) && !self.0.is_awake(child) {
            awaken(vm, &self.0, child)?;
        }
        if vm.objects().contains(group) && is_a(vm, group, "GuiScrollCtrl") {
            crate::scroll::compute_sizes(vm, &self.0, group);
        }
        Ok(())
    }

    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        // The script's `onRemove` first (`Con::executef(this, "onRemove")`: silent without one), then the first
        // responder cleared and SimGroup's own.
        if !self.0.is_realm(id) {
            vm.call_method(id, b"onRemove", &[])?;
        }
        let result = if self.0.is_awake(id) {
            sleep(vm, &self.0, id)
        } else {
            Ok(())
        };
        crate::input::forget(&self.0, id);
        self.0.ext().removed(id);
        crate::custom::removed(&self.0, id);
        crate::client_script::forget(&self.0, id);
        let mut state = self.0.0.borrow_mut();
        state.controls.remove(&id);
        if state.canvas == Some(id) {
            state.canvas = None;
        }
        result
    }
}

/// `GuiControlProfile` and `GuiCursor`: they join GuiDataGroup (TGE 1.4 `guiTypes.cc`).
struct DataClass;

impl EngineClass for DataClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        if let Some(group) = vm.objects().find(b"GuiDataGroup") {
            vm.objects_mut().add(group, id);
        }
        Ok(true)
    }
}

/// `GuiControlProfile`: joins GuiDataGroup, and its constructor starts a new profile from `GuiDefaultProfile` when
/// that object exists and is a profile: every static field but `mouseOverSelected` (which the constructor clears) is copied, before
/// the declaration's `: parent` copy and its own fields. The first profile (GuiDefaultProfile itself) keeps the class's values.
struct ProfileClass;

/// The profile a new profile is copied from, and the one field the copy leaves out.
const DEFAULT_PROFILE: &[u8] = b"GuiDefaultProfile";
const NOT_COPIED: &[u8] = b"mouseOverSelected";

impl EngineClass for ProfileClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        DataClass.on_add(vm, id)
    }

    fn on_construct(&self, objects: &Objects, object: &mut SimObject) {
        let classes = &objects.classes;
        let Some(profile) = classes.find(b"GuiControlProfile") else {
            return;
        };
        // The constructor sets none of the colours (TGE leaves them unset): they start as `0 0 0 0`, before the copy from
        // GuiDefaultProfile and the declaration's fields.
        if let Some(def) = classes.get(profile) {
            for (field, slots) in &def.layout.fields {
                if field.ty == FieldType::ColorI {
                    for slot in slots {
                        if let Some(value) = object.slot_mut(*slot) {
                            *value = Value::ColorI([0, 0, 0, 0]);
                        }
                    }
                }
            }
        }
        let Some(source) = objects
            .find(DEFAULT_PROFILE)
            .and_then(|id| objects.get(id))
            .filter(|o| classes.is_a(o.class(), profile))
        else {
            return;
        };
        let Some(def) = classes.get(profile) else {
            return;
        };
        for (field, _) in &def.layout.fields {
            if field.name.eq_ignore_ascii_case(NOT_COPIED) {
                continue;
            }
            let (Some((_, from)), Some((_, to))) = (
                classes.static_field(source.class(), &field.name),
                classes.static_field(object.class(), &field.name),
            ) else {
                continue;
            };
            for (a, b) in from.iter().zip(to.iter()) {
                if let (Some(value), Some(slot)) = (source.slot(*a).cloned(), object.slot_mut(*b)) {
                    *slot = value;
                }
            }
        }
    }
}

fn reg(
    vm: &mut Vm,
    gui: &Gui,
    name: &str,
    min: i32,
    max: i32,
    usage: &str,
    f: impl Fn(&mut Vm, &Gui, ObjId, &NativeCall) -> Native + 'static,
) {
    let gui = gui.clone();
    vm.register_in(
        "GuiControl",
        name,
        min,
        max,
        usage,
        move |vm, c| match this(vm, c) {
            Some(id) if gui.is_control(id) => f(vm, &gui, id, c),
            _ => Native::Void,
        },
    );
}

fn ints(p: (i32, i32)) -> Native {
    Native::Str(format!("{} {}", p.0, p.1).into_bytes())
}

pub(crate) fn register(vm: &mut Vm, gui: &Gui) {
    vm.set_class_behaviour("GuiControl", Rc::new(ControlClass(gui.clone())));
    vm.set_class_behaviour("GuiControlProfile", Rc::new(ProfileClass));
    vm.set_class_behaviour("GuiCursor", Rc::new(DataClass));

    reg(
        vm,
        gui,
        "setValue",
        3,
        3,
        "(string value)",
        |vm, gui, id, c| {
            classes::set_value(vm, gui, id, arg(c, 2));
            Native::Void
        },
    );
    reg(vm, gui, "getValue", 2, 2, "", |vm, gui, id, _| {
        Native::Str(classes::get_value(vm, gui, id))
    });
    reg(
        vm,
        gui,
        "setActive",
        3,
        3,
        "(bool active)",
        // `mActive`; false also clears the first responder.
        |vm, gui, id, c| {
            let on = atob(arg(c, 2));
            gui.with_mut(id, |s| s.active = on);
            if !on {
                crate::input::clear_first_responder(vm, gui, id);
            }
            Native::Void
        },
    );
    reg(vm, gui, "isActive", 2, 2, "", |_, gui, id, _| {
        Native::Bool(crate::input::active(gui, id))
    });
    reg(
        vm,
        gui,
        "setVisible",
        3,
        3,
        "(bool visible)",
        |vm, _, id, c| {
            vm.set_object_field(
                id,
                b"visible",
                b"",
                if atob(arg(c, 2)) { b"1" } else { b"0" },
            );
            Native::Void
        },
    );
    reg(vm, gui, "isVisible", 2, 2, "", |vm, _, id, _| {
        Native::Bool(flag(vm, id, b"visible"))
    });
    reg(vm, gui, "isAwake", 2, 2, "", |_, gui, id, _| {
        Native::Bool(gui.is_awake(id))
    });
    // `makeFirstResponder(bool)` (TGE 1.4): `setFirstResponder()` or `clearFirstResponder()`.
    reg(
        vm,
        gui,
        "makeFirstResponder",
        3,
        3,
        "(bool isFirst)",
        |vm, gui, id, c| {
            if atob(arg(c, 2)) {
                done(crate::input::set_first_responder(vm, gui, id))
            } else {
                crate::input::clear_first_responder(vm, gui, id);
                Native::Void
            }
        },
    );
    reg(
        vm,
        gui,
        "setProfile",
        3,
        3,
        "(GuiControlProfile p)",
        |vm, _, id, c| {
            let name = arg(c, 2).to_vec();
            if vm
                .objects()
                .find(&name)
                .is_some_and(|p| is_a(vm, p, "GuiControlProfile"))
            {
                vm.set_object_field(id, b"profile", b"", &name);
            }
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        "resize",
        6,
        6,
        "(int x, int y, int w, int h)",
        |vm, gui, id, c| {
            let position = (atoi(arg(c, 2)), atoi(arg(c, 3)));
            let extent = (atoi(arg(c, 4)), atoi(arg(c, 5)));
            resize(vm, gui, id, position, extent);
            Native::Void
        },
    );
    reg(vm, gui, "getPosition", 2, 2, "", |vm, _, id, _| {
        ints(point(vm, id, b"position"))
    });
    reg(vm, gui, "getScreenPosition", 2, 2, "", |vm, _, id, _| {
        ints(screen_position(vm, id))
    });
    reg(vm, gui, "getExtent", 2, 2, "()", |vm, _, id, _| {
        ints(point(vm, id, b"extent"))
    });
    reg(vm, gui, "getMinExtent", 2, 2, "()", |vm, _, id, _| {
        ints(point(vm, id, b"minExtent"))
    });
    reg(
        vm,
        gui,
        "setHasRendered",
        3,
        3,
        "(bool val)",
        |_, gui, id, c| {
            if let Some(control) = gui.0.borrow_mut().controls.get_mut(&id) {
                control.has_rendered = atob(arg(c, 2));
            }
            Native::Void
        },
    );
    // `GuiControl::addObject` / `removeObject` behind the SimSet methods (the engine's are virtual): wake and sleep.
    reg(
        vm,
        gui,
        "add",
        3,
        0,
        "(SimObject obj, ...)",
        |vm, gui, id, c| {
            for text in c.argv.iter().skip(2) {
                match vm.objects().find(text) {
                    Some(child) if gui.is_control(child) => {
                        if let Err(e) = add_child(vm, gui, id, child) {
                            return Native::Fail(e);
                        }
                    }
                    // `GuiControl::addObject` refuses anything that is not a control, silently in a release build.
                    Some(_) => {}
                    None => {
                        let set = crate::name_of(vm, id);
                        let object = String::from_utf8_lossy(text);
                        let m = crate::ext::GuiMessage::SetAddMissing {
                            object: &object,
                            set: &set,
                        };
                        vm.print(crate::message_text(gui, &m).as_bytes());
                    }
                }
            }
            Native::Void
        },
    );
    reg(
        vm,
        gui,
        "remove",
        3,
        0,
        "(SimObject obj, ...)",
        |vm, gui, id, c| {
            for text in c.argv.iter().skip(2) {
                let removed = match vm.objects().find(text) {
                    Some(child) => match remove_child(vm, gui, id, child) {
                        Ok(removed) => removed,
                        Err(e) => return Native::Fail(e),
                    },
                    None => false,
                };
                if !removed {
                    let set = crate::name_of(vm, id);
                    let object = String::from_utf8_lossy(text);
                    let m = crate::ext::GuiMessage::SetRemoveMissing {
                        object: &object,
                        set: &set,
                    };
                    vm.print(crate::message_text(gui, &m).as_bytes());
                }
            }
            Native::Void
        },
    );
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
