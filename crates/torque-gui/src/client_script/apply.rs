//! The typed op applier: reads the `ui` ops of a realm's flush, checks each against the format and the realm's grants and caps,
//! and builds, changes and removes the realm's controls. It is the enforcement point: whatever the guest sends is hostile.
//! A violation stops the realm, and an op that violates changes nothing.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use torque_client_script_format::ui::{
    self, Kind, MAX_CONTROL_TEXT, MAX_CONTROLS, MAX_DEPTH, MAX_IMAGES, MAX_LIST_TEXT,
    MAX_RICH_TEXT, MAX_TEXT, Prop, PropType, PropValue,
};
use torque_client_script_format::{
    Grants, MAX_FLUSH_BYTES, MAX_OPS, Malformed, Reader, Record, Records, Tier, abi, op,
};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::chip;
use super::controls::{NodeState, class_name};
use super::layers::{
    LayerState, RealmView, Release, effectively_visible, invalidate, release_holds,
    server_takes_input,
};
use super::props::Props;
use crate::control::{self, make_realm_control};
use crate::draw::RealmImage;
use crate::{Gui, MAX_TREE_DEPTH};

/// The most mod roots a realm has.
pub const MAX_ROOTS: usize = 64;

/// Why a realm is stopped: an op a checked guest never sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Violation(pub &'static str);

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "protocol violation: {}", self.0)
    }
}

impl std::error::Error for Violation {}

impl From<Malformed> for Violation {
    fn from(m: Malformed) -> Violation {
        Violation(m.0)
    }
}

/// A record of another tier, checked against the grants and passed on for the host to handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forwarded {
    /// The record's tier.
    pub tier: Tier,
    /// The record's kind within the tier.
    pub kind: u8,
    /// The record's body.
    pub body: Vec<u8>,
}

/// What one flush did besides the `ui` ops.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// The `realm` and `net` records, in order.
    pub forwarded: Vec<Forwarded>,
    /// `ui.focus` ops that were refused for lack of a player's action (they do not stop the realm).
    pub focus_refused: u32,
    /// `ui.interactive(true)` ops that were refused because the player or the game took the holds and the player has not
    /// acted on the realm's controls since (they do not stop the realm).
    pub interactive_refused: u32,
}

/// The realm's server's store: the bytes of a content path the server owns, None for any other. The format has checked the
/// path's form; the store looks it up as it is (no case folding, no extension search, never the game's files). What it gives
/// is what is drawn. A new realm refuses every path until the host sets one ([`Realm::set_content_check`]).
pub type ContentCheck = Box<dyn Fn(&str) -> Option<Arc<[u8]>>>;

/// One control or mod root the realm made.
#[derive(Debug)]
struct Node {
    obj: ObjId,
    /// The parent's handle; 0 for a mod root.
    parent: u32,
    kind: Option<Kind>,
    /// Levels below its mod root (a root is 0).
    depth: usize,
    children: Vec<u32>,
    /// The bytes of its text props, for the realm-wide cap.
    text: usize,
    image: Option<String>,
}

/// An image path in use, by how many controls, and what the store gave for it.
#[derive(Debug)]
struct ImageUse {
    count: usize,
    image: RealmImage,
}

/// The props that place a control; the others are kept in its state.
const LAYOUT: [Prop; 5] = [
    Prop::Pos,
    Prop::Size,
    Prop::HSize,
    Prop::VSize,
    Prop::Visible,
];

/// A realm's controls on one GUI, and the grants that decide which ops it may send.
pub struct Realm {
    grants: Grants,
    nodes: HashMap<u32, Node>,
    next: u32,
    roots: HashMap<String, u32>,
    /// Controls, not counting mod roots.
    controls: usize,
    text_total: usize,
    images: HashMap<String, ImageUse>,
    content: ContentCheck,
}

impl fmt::Debug for Realm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Realm")
            .field("grants", &self.grants)
            .field("controls", &self.controls)
            .finish_non_exhaustive()
    }
}

/// The bytes of a control's text props, by the cap each has.
struct TextUse {
    /// `text`, `tooltip` and `placeholder`.
    plain: usize,
    /// A richtext's markup.
    rich: usize,
    /// A list's rows.
    rows: usize,
}

impl TextUse {
    fn of(kind: Kind, props: &Props) -> TextUse {
        TextUse {
            plain: props.text(Prop::Tooltip).len()
                + props.text(Prop::Placeholder).len()
                + props.text(Prop::Text).len(),
            rich: if kind == Kind::RichText {
                props.text(Prop::RichText).len()
            } else {
                0
            },
            rows: props.rows().iter().map(String::len).sum(),
        }
    }

    fn total(&self) -> usize {
        self.plain + self.rich + self.rows
    }

    fn within_caps(&self) -> bool {
        self.plain <= MAX_CONTROL_TEXT && self.rich <= MAX_RICH_TEXT && self.rows <= MAX_LIST_TEXT
    }
}

/// The rules between a control's props that one prop alone does not show (the image is [`Realm::image_for`]'s).
fn check_combined(kind: Kind, props: &Props) -> Result<(), Violation> {
    if !TextUse::of(kind, props).within_caps() {
        return Err(Violation("a control's text is past its cap"));
    }
    if props.rows().len() < props.int(Prop::Selected) as usize {
        return Err(Violation("a selected row past the rows"));
    }
    let (lo, hi) = props.vec2(Prop::Range);
    let value = props.num(Prop::SliderValue);
    if kind == Kind::Slider && !(lo < hi && (lo..=hi).contains(&value)) {
        return Err(Violation("a slider's range or value out of order"));
    }
    Ok(())
}

/// The name of enum prop `prop`'s value `index`, for a console field.
fn enum_name(prop: Prop, index: u8) -> &'static str {
    match ui::prop_def(prop).ty {
        PropType::Enum(names) => names.get(usize::from(index)).copied().unwrap_or(""),
        _ => "",
    }
}

/// Deletes an object a failed step left behind. A delete fails only when the script engine's step budget is spent, and then
/// there is nothing more to do for the object.
fn discard(vm: &mut Vm, id: ObjId) {
    let _ = vm.delete_object_quiet(id);
}

fn flag_text(on: bool) -> &'static [u8] {
    if on { b"1" } else { b"0" }
}

impl Realm {
    /// Starts the realm on `gui`: the server layer's container and the chip, for a server shown as `view`. `None` when a realm
    /// is on this GUI already or the controls cannot be made.
    pub fn new(vm: &mut Vm, gui: &Gui, grants: Grants, view: RealmView) -> Option<Realm> {
        if gui.0.borrow().client_script.server.is_some() {
            return None;
        }
        let size = gui.0.borrow().canvas_size;
        let server = make_realm_control(vm, gui, class_name(None), &[])?;
        let Some(chip) = make_realm_control(vm, gui, chip::CLASS, &[]) else {
            discard(vm, server);
            return None;
        };
        gui.with_custom_state::<NodeState, _>(server, |s| *s = NodeState::new(0, None));
        control::set_bounds(vm, server, (0, 0), size);
        if control::awaken(vm, gui, server).is_err() {
            discard(vm, server);
            discard(vm, chip);
            return None;
        }
        {
            let mut s = gui.0.borrow_mut();
            let l = &mut s.client_script;
            l.server = Some(server);
            l.chip = Some(chip);
            l.view = Some(view);
            l.screen = size;
        }
        Some(Realm {
            grants,
            nodes: HashMap::new(),
            next: 1,
            roots: HashMap::new(),
            controls: 0,
            text_total: 0,
            images: HashMap::new(),
            content: Box::new(|_| None),
        })
    }

    /// Sets the server's store, which gives the bytes of the content paths the server owns.
    pub fn set_content_check(&mut self, check: ContentCheck) {
        self.content = check;
    }

    /// The tiers the realm may still use.
    pub fn grants(&self) -> Grants {
        self.grants
    }

    /// The number the next handle will have.
    #[cfg(test)]
    pub(crate) fn next_handle(&self) -> u32 {
        self.next
    }

    /// How many controls the realm has (mod roots not counted).
    pub fn control_count(&self) -> usize {
        self.controls
    }

    /// The object of a live handle.
    pub fn object(&self, handle: u32) -> Option<ObjId> {
        self.nodes.get(&handle).map(|n| n.obj)
    }

    /// Every live handle, mod roots included.
    pub fn handles(&self) -> Vec<u32> {
        let mut handles: Vec<u32> = self.nodes.keys().copied().collect();
        handles.sort_unstable();
        handles
    }

    /// Applies one flush of the realm's ops at `now_us` (the host's clock). The `ui` ops are applied; the other records are
    /// checked against the grants and given back.
    ///
    /// # Errors
    /// The first violation: the flush does not follow the format, an op is of a tier the realm lacks, or a `ui` op breaks a
    /// rule or cap. The ops before it were applied; the host stops the realm.
    pub fn apply_flush(
        &mut self,
        vm: &mut Vm,
        gui: &Gui,
        flush: &[u8],
        now_us: u64,
    ) -> Result<Applied, Violation> {
        gui.set_client_script_clock(now_us);
        let mut applied = Applied::default();
        // Whatever this flush does, the layers work out again what the tree shows.
        invalidate(gui);
        for record in Records::new(flush, MAX_OPS, MAX_FLUSH_BYTES)? {
            let record = record?;
            let tier = Tier::from_id(record.tier).ok_or(Violation("an op of an unknown tier"))?;
            if !self.grants.has(tier) {
                return Err(Violation("an op of a tier the realm does not have"));
            }
            match tier {
                Tier::Ui => self.apply_ui(vm, gui, &record, &mut applied)?,
                Tier::Realm | Tier::Net => applied.forwarded.push(Forwarded {
                    tier,
                    kind: record.kind,
                    body: record.body.to_vec(),
                }),
                _ => return Err(Violation("an op of a tier format 1 has no ops for")),
            }
        }
        Ok(applied)
    }

    fn apply_ui(
        &mut self,
        vm: &mut Vm,
        gui: &Gui,
        record: &Record<'_>,
        applied: &mut Applied,
    ) -> Result<(), Violation> {
        let mut r = record.reader();
        match record.kind {
            op::ui::ROOT => self.root(vm, gui, &mut r),
            op::ui::CREATE => self.create(vm, gui, &mut r),
            op::ui::SET => self.set(vm, gui, &mut r),
            op::ui::DESTROY => self.destroy(vm, &mut r),
            op::ui::PARENT => self.reparent(vm, gui, &mut r),
            op::ui::ORDER => self.order(vm, &mut r),
            op::ui::INTERACTIVE => {
                let on = r.bool()?;
                r.finish()?;
                let l = &mut gui.0.borrow_mut().client_script;
                if on && l.interactive_blocked {
                    applied.interactive_refused += 1;
                } else {
                    l.interactive = on;
                }
                Ok(())
            }
            op::ui::FOCUS => self.focus(vm, gui, &mut r, applied),
            _ => Err(Violation("an unknown ui op")),
        }
    }

    fn live(&self, handle: u32) -> Result<&Node, Violation> {
        self.nodes
            .get(&handle)
            .ok_or(Violation("a handle that is not live"))
    }

    /// A live control (not a mod root).
    fn control(&self, handle: u32) -> Result<&Node, Violation> {
        let node = self.live(handle)?;
        if node.kind.is_none() {
            return Err(Violation("a mod root where a control is needed"));
        }
        Ok(node)
    }

    fn root(&mut self, vm: &mut Vm, gui: &Gui, r: &mut Reader<'_>) -> Result<(), Violation> {
        let (handle, mod_id) = (r.u32()?, r.text()?);
        r.finish()?;
        abi::check_mod_id(mod_id).map_err(Violation)?;
        if handle != self.next || self.roots.contains_key(mod_id) || self.roots.len() >= MAX_ROOTS {
            return Err(Violation(
                "a mod root that is not the next, or a second one",
            ));
        }
        let server = gui
            .0
            .borrow()
            .client_script
            .server
            .ok_or(Violation("no server layer"))?;
        let size = gui.0.borrow().canvas_size;
        let obj = make_realm_control(vm, gui, class_name(None), &[])
            .ok_or(Violation("a control that could not be made"))?;
        gui.with_custom_state::<NodeState, _>(obj, |s| *s = NodeState::new(handle, None));
        vm.set_object_field(obj, b"horizSizing", b"", b"width");
        vm.set_object_field(obj, b"vertSizing", b"", b"height");
        control::set_bounds(vm, obj, (0, 0), size);
        if control::add_child(vm, gui, server, obj).is_err() {
            discard(vm, obj);
            return Err(Violation("a control that could not be placed"));
        }
        self.next += 1;
        self.roots.insert(mod_id.to_string(), handle);
        self.nodes.insert(
            handle,
            Node {
                obj,
                parent: 0,
                kind: None,
                depth: 0,
                children: Vec::new(),
                text: 0,
                image: None,
            },
        );
        Ok(())
    }

    fn create(&mut self, vm: &mut Vm, gui: &Gui, r: &mut Reader<'_>) -> Result<(), Violation> {
        let (handle, parent, kind_id) = (r.u32()?, r.u32()?, r.u8()?);
        let kind_def = ui::kind_from_id(kind_id).ok_or(Violation("an unknown kind"))?;
        let given = ui::read_props(r, kind_def)?;
        r.finish()?;
        if handle != self.next {
            return Err(Violation("a handle that is not the next number"));
        }
        let (parent_obj, depth) = {
            let p = self.live(parent)?;
            if is_field(p) {
                return Err(Violation("a control under a text field"));
            }
            (p.obj, p.depth + 1)
        };
        if depth > MAX_DEPTH || self.controls >= MAX_CONTROLS {
            return Err(Violation("a control past the depth or count cap"));
        }
        let mut props = Props::default();
        for (prop, value) in &given {
            props.set(*prop, value.clone());
        }
        check_combined(kind_def.kind, &props)?;
        let text = TextUse::of(kind_def.kind, &props).total();
        let image = props.text(Prop::Image).to_string();
        self.check_totals(text, &image, 0, None)?;
        let shown = self.image_for(&image)?;
        let obj = make_realm_control(vm, gui, class_name(Some(kind_def.kind)), &[])
            .ok_or(Violation("a control that could not be made"))?;
        gui.with_custom_state::<NodeState, _>(obj, |s| {
            *s = NodeState::new(handle, Some(kind_def.kind));
            s.set_props(props, shown.clone());
        });
        place_defaults(vm, gui, obj, &given);
        if control::add_child(vm, gui, parent_obj, obj).is_err() {
            discard(vm, obj);
            return Err(Violation("a control that could not be placed"));
        }
        self.next += 1;
        self.add_counts(text, &image, shown);
        self.controls += 1;
        if let Some(p) = self.nodes.get_mut(&parent) {
            p.children.push(handle);
        }
        self.nodes.insert(
            handle,
            Node {
                obj,
                parent,
                kind: Some(kind_def.kind),
                depth,
                children: Vec::new(),
                text,
                image: (!image.is_empty()).then_some(image),
            },
        );
        Ok(())
    }

    /// Whether a control's text of `text` bytes and image `image` fit the realm's totals, when it replaces `old_text` bytes
    /// and `old_image`.
    fn check_totals(
        &self,
        text: usize,
        image: &str,
        old_text: usize,
        old_image: Option<&str>,
    ) -> Result<(), Violation> {
        if self.text_total - old_text + text > MAX_TEXT {
            return Err(Violation("the realm's text is past its cap"));
        }
        let known = self.images.contains_key(image) || old_image == Some(image);
        if !image.is_empty() && !known && self.images.len() >= MAX_IMAGES {
            return Err(Violation("more distinct images than the cap"));
        }
        Ok(())
    }

    /// What the store gives for the image path `path` (none for no image): the image already in use under it, else the
    /// store's bytes under a new id.
    fn image_for(&self, path: &str) -> Result<Option<RealmImage>, Violation> {
        if path.is_empty() {
            return Ok(None);
        }
        if let Some(used) = self.images.get(path) {
            return Ok(Some(used.image.clone()));
        }
        (self.content)(path)
            .and_then(|bytes| RealmImage::new(path, bytes))
            .map(Some)
            .ok_or(Violation("an image that is not this server's content"))
    }

    fn add_counts(&mut self, text: usize, path: &str, image: Option<RealmImage>) {
        self.text_total += text;
        if let Some(image) = image {
            self.images
                .entry(path.to_string())
                .or_insert(ImageUse { count: 0, image })
                .count += 1;
        }
    }

    fn drop_counts(&mut self, text: usize, image: Option<&str>) {
        self.text_total = self.text_total.saturating_sub(text);
        let Some(image) = image else {
            return;
        };
        if let Some(used) = self.images.get_mut(image) {
            used.count -= 1;
            if used.count == 0 {
                self.images.remove(image);
            }
        }
    }

    fn set(&mut self, vm: &mut Vm, gui: &Gui, r: &mut Reader<'_>) -> Result<(), Violation> {
        let handle = r.u32()?;
        let (obj, kind, old_text, old_image) = {
            let n = self.control(handle)?;
            (n.obj, n.kind, n.text, n.image.clone())
        };
        let kind = kind.ok_or(Violation("a mod root where a control is needed"))?;
        let given = ui::read_props(r, ui::kind_def(kind))?;
        r.finish()?;
        let mut props = gui
            .with_custom_state::<NodeState, _>(obj, |s| s.props.clone())
            .ok_or(Violation("a control with no state"))?;
        for (prop, value) in &given {
            props.set(*prop, value.clone());
        }
        check_combined(kind, &props)?;
        let text = TextUse::of(kind, &props).total();
        let image = props.text(Prop::Image).to_string();
        self.check_totals(text, &image, old_text, old_image.as_deref())?;
        let shown = self.image_for(&image)?;
        gui.with_custom_state::<NodeState, _>(obj, |s| s.set_props(props, shown.clone()));
        apply_layout(vm, gui, obj, &given);
        self.drop_counts(old_text, old_image.as_deref());
        self.add_counts(text, &image, shown);
        if let Some(n) = self.nodes.get_mut(&handle) {
            n.text = text;
            n.image = (!image.is_empty()).then_some(image);
        }
        Ok(())
    }

    /// The handles of `handle` and everything under it, parents first.
    fn subtree(&self, handle: u32) -> Vec<u32> {
        let mut out = vec![handle];
        let mut at = 0;
        while let Some(&h) = out.get(at) {
            at += 1;
            if let Some(n) = self.nodes.get(&h) {
                out.extend(&n.children);
            }
        }
        out
    }

    fn destroy(&mut self, vm: &mut Vm, r: &mut Reader<'_>) -> Result<(), Violation> {
        let handle = r.u32()?;
        r.finish()?;
        self.control(handle)?;
        self.remove_subtree(vm, handle)
    }

    /// Deletes the control `handle` and everything under it.
    fn remove_subtree(&mut self, vm: &mut Vm, handle: u32) -> Result<(), Violation> {
        let (obj, parent) = {
            let n = self.live(handle)?;
            (n.obj, n.parent)
        };
        vm.delete_object_quiet(obj)
            .map_err(|_| Violation("a control that could not be deleted"))?;
        for h in self.subtree(handle) {
            let Some(node) = self.nodes.remove(&h) else {
                continue;
            };
            self.drop_counts(node.text, node.image.as_deref());
            if node.kind.is_some() {
                self.controls -= 1;
            }
        }
        if let Some(p) = self.nodes.get_mut(&parent) {
            p.children.retain(|c| *c != handle);
        }
        Ok(())
    }

    fn reparent(&mut self, vm: &mut Vm, gui: &Gui, r: &mut Reader<'_>) -> Result<(), Violation> {
        let (handle, new_parent) = (r.u32()?, r.u32()?);
        r.finish()?;
        let (obj, old_parent, old_depth) = {
            let n = self.control(handle)?;
            (n.obj, n.parent, n.depth)
        };
        let (parent_obj, parent_depth) = {
            let p = self.live(new_parent)?;
            if is_field(p) {
                return Err(Violation("a control moved under a text field"));
            }
            (p.obj, p.depth)
        };
        let below = self.subtree(handle);
        if below.contains(&new_parent) {
            return Err(Violation("a control moved under itself"));
        }
        let height = below
            .iter()
            .filter_map(|h| self.nodes.get(h))
            .map(|n| n.depth - old_depth)
            .max()
            .unwrap_or(0);
        let new_depth = parent_depth + 1;
        if new_depth + height > MAX_DEPTH {
            return Err(Violation("a control moved past the depth cap"));
        }
        control::add_child(vm, gui, parent_obj, obj)
            .map_err(|_| Violation("a control that could not be moved"))?;
        if let Some(p) = self.nodes.get_mut(&old_parent) {
            p.children.retain(|c| *c != handle);
        }
        if let Some(p) = self.nodes.get_mut(&new_parent) {
            p.children.push(handle);
        }
        for h in below {
            if let Some(n) = self.nodes.get_mut(&h) {
                n.depth = n.depth - old_depth + new_depth;
            }
        }
        if let Some(n) = self.nodes.get_mut(&handle) {
            n.parent = new_parent;
        }
        Ok(())
    }

    fn order(&mut self, vm: &mut Vm, r: &mut Reader<'_>) -> Result<(), Violation> {
        let (handle, front) = (r.u32()?, r.bool()?);
        r.finish()?;
        let (obj, parent) = {
            let n = self.control(handle)?;
            (n.obj, self.live(n.parent)?.obj)
        };
        // The member list starts with the control drawn first.
        vm.objects_mut().move_member(parent, obj, !front);
        Ok(())
    }

    fn focus(
        &mut self,
        vm: &mut Vm,
        gui: &Gui,
        r: &mut Reader<'_>,
        applied: &mut Applied,
    ) -> Result<(), Violation> {
        let handle = r.u32()?;
        r.finish()?;
        let node = self.control(handle)?;
        if node.kind != Some(Kind::TextField) {
            return Err(Violation("focus on a control that is not a text field"));
        }
        let obj = node.obj;
        let allowed = gui.0.borrow().client_script.activated()
            && server_takes_input(vm, gui)
            && effectively_visible(vm, gui, obj, None);
        if !allowed {
            applied.focus_refused += 1;
            return Ok(());
        }
        gui.0.borrow_mut().client_script.focus = Some(obj);
        gui.with_custom_state::<NodeState, _>(obj, |s| {
            s.caret = s.props.text(Prop::Text).chars().count();
        });
        Ok(())
    }

    /// Takes `tier` away: for `ui`, every control goes in this call and the cursor and focus are given back.
    ///
    /// # Errors
    /// When a control cannot be deleted.
    pub fn revoke(&mut self, vm: &mut Vm, gui: &Gui, tier: Tier) -> Result<(), Violation> {
        self.grants = self.grants.without(tier);
        if let Some(view) = gui.0.borrow_mut().client_script.view.as_mut()
            && let Some(t) = view.tiers.iter_mut().find(|t| t.tier == tier)
        {
            t.on = false;
        }
        if tier != Tier::Ui {
            return Ok(());
        }
        let mut roots: Vec<u32> = self.roots.values().copied().collect();
        roots.sort_unstable();
        for root in roots {
            self.remove_subtree(vm, root)?;
        }
        self.roots.clear();
        invalidate(gui);
        release_holds(gui, Release::Quiet);
        Ok(())
    }

    /// Ends the realm: every object and every layer state it made is gone, and the cursor is as it was.
    pub fn teardown(self, vm: &mut Vm, gui: &Gui) {
        release_holds(gui, Release::Quiet);
        let (server, chip) = {
            let l = &gui.0.borrow().client_script;
            (l.server, l.chip)
        };
        for id in server.into_iter().chain(chip) {
            discard(vm, id);
        }
        let mut s = gui.0.borrow_mut();
        let l = &mut s.client_script;
        *l = LayerState {
            screens: std::mem::take(&mut l.screens),
            faces: std::mem::take(&mut l.faces),
            ..LayerState::default()
        };
    }

    /// Checks what must hold after any op, applied or refused: the caps, the depth, the tree against the handles, and that no
    /// control has a code field, a name or a script namespace of its own. Describes the first thing wrong.
    ///
    /// # Errors
    /// The first broken invariant.
    pub fn check(&self, vm: &Vm, gui: &Gui) -> Result<(), String> {
        if self.controls > MAX_CONTROLS
            || self.text_total > MAX_TEXT
            || self.images.len() > MAX_IMAGES
        {
            return Err("a cap is broken".into());
        }
        let counted = self.nodes.values().filter(|n| n.kind.is_some()).count();
        if counted != self.controls {
            return Err(format!("{counted} controls but {} counted", self.controls));
        }
        for (handle, node) in &self.nodes {
            if node.depth > MAX_DEPTH || node.depth > MAX_TREE_DEPTH {
                return Err(format!("handle {handle} is {} deep", node.depth));
            }
            if !vm.objects().contains(node.obj) || !gui.is_realm(node.obj) {
                return Err(format!("handle {handle} has no realm control"));
            }
            if let Some(bad) = code_field(vm, node.obj) {
                return Err(format!("handle {handle} has {bad}"));
            }
            let group = vm.objects().get(node.obj).and_then(|o| o.group());
            let parent = self.nodes.get(&node.parent).map(|p| p.obj);
            if node.parent != 0 && group != parent {
                return Err(format!("handle {handle} is in the wrong group"));
            }
        }
        Ok(())
    }
}

/// Whether the node is a text field: a leaf, so nothing of the realm's can be drawn over it with it ([`super::focus_on_top`]).
fn is_field(node: &Node) -> bool {
    node.kind == Some(Kind::TextField)
}

/// Which code field, name or namespace the control has, if any.
pub(crate) fn code_field(vm: &Vm, obj: ObjId) -> Option<&'static str> {
    const FIELDS: [&str; 8] = [
        "command",
        "altCommand",
        "variable",
        "accelerator",
        "class",
        "superClass",
        "closeCommand",
        "tooltipProfile",
    ];
    let object = vm.objects().get(obj)?;
    if object.name().is_some() {
        return Some("a console name");
    }
    FIELDS.into_iter().find(|name| {
        vm.objects()
            .field(obj, name.as_bytes(), b"")
            .is_some_and(|v| !v.is_empty())
    })
}

/// Places a new control: the format's defaults for the layout props, then what the op gave.
fn place_defaults(vm: &mut Vm, gui: &Gui, obj: ObjId, given: &[(Prop, PropValue)]) {
    let mut all: Vec<(Prop, PropValue)> = vec![
        (Prop::Pos, ui::prop_def(Prop::Pos).default.value()),
        (Prop::Size, ui::prop_def(Prop::Size).default.value()),
    ];
    all.extend(given.iter().cloned());
    apply_layout(vm, gui, obj, &all);
}

/// Writes the layout props among `props` to the control's fields; the others are not its business.
fn apply_layout(vm: &mut Vm, gui: &Gui, obj: ObjId, props: &[(Prop, PropValue)]) {
    let (mut pos, mut ext) = control::bounds(vm, obj);
    let mut moved = false;
    for (prop, value) in props.iter().filter(|(p, _)| LAYOUT.contains(p)) {
        match (prop, value) {
            (Prop::Pos, PropValue::Vec2(x, y)) => {
                pos = (x.round() as i32, y.round() as i32);
                moved = true;
            }
            (Prop::Size, PropValue::Vec2(x, y)) => {
                ext = (x.round() as i32, y.round() as i32);
                moved = true;
            }
            (Prop::HSize, PropValue::Enum(i)) => {
                vm.set_object_field(
                    obj,
                    b"horizSizing",
                    b"",
                    enum_name(Prop::HSize, *i).as_bytes(),
                );
            }
            (Prop::VSize, PropValue::Enum(i)) => {
                vm.set_object_field(
                    obj,
                    b"vertSizing",
                    b"",
                    enum_name(Prop::VSize, *i).as_bytes(),
                );
            }
            (Prop::Visible, PropValue::Bool(on)) => {
                vm.set_object_field(obj, b"visible", b"", flag_text(*on));
            }
            _ => {}
        }
    }
    if moved {
        control::resize(vm, gui, obj, pos, ext);
    }
}
