//! The typed op applier: reads the `ui` ops of a realm's flush, checks each against the format and the realm's grants and caps,
//! and builds, changes and removes the realm's controls. It is the enforcement point: whatever the guest sends is hostile.
//! A violation stops the realm, and an op that violates changes nothing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use torque_client_script_format::ui::{
    self, Kind, MAX_CONTAINER_CHILDREN, MAX_CONTAINER_DEPTH, MAX_CONTROL_TEXT, MAX_CONTROLS,
    MAX_DEPTH, MAX_IMAGE_BYTES, MAX_IMAGES, MAX_LIST_TEXT, MAX_RICH_TEXT, MAX_TEXT, Prop, PropType,
    PropValue,
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
use super::layout::RealmLayout;
use super::props::{Given, Lookup, Over, Props, int_of, num_of, rows_of, text_of, vec2_of};
use crate::control::{self, make_realm_control};
use crate::draw::RealmImage;
use crate::{Gui, MAX_FRAME_WORK, MAX_TREE_DEPTH};

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
    /// The bytes of its text props, for its caps and the realm-wide one.
    text: TextUse,
    image: Option<String>,
    /// The containers on the way from its mod root to it, itself counted (a root is 0).
    containers: usize,
    /// A container's props as set (it keeps no [`NodeState`]: its class is the layout plugin's). Empty for the others.
    held: Props,
}

impl Node {
    fn new(obj: ObjId, parent: u32, kind: Option<Kind>, depth: usize, containers: usize) -> Node {
        Node {
            obj,
            parent,
            kind,
            depth,
            children: Vec::new(),
            text: TextUse::default(),
            image: None,
            containers,
            held: Props::default(),
        }
    }

    /// Whether it is a container (`flex` or `grid`).
    fn is_container(&self) -> bool {
        self.kind.is_some_and(|k| k.container().is_some())
    }
}

/// An image path the realm has shown, by how many controls show it now (0: kept, its texture with it), and what the store
/// gave for it.
#[derive(Debug)]
struct ImageUse {
    count: usize,
    image: RealmImage,
    /// While kept: its place among the kept ones.
    kept_at: Option<u64>,
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
    /// The decoded bytes of the images in `images` that controls show.
    image_bytes: usize,
    /// The images no control shows now, kept so that showing one again decodes nothing, by when they were last shown: they
    /// give way, oldest first, to a new image that the realm's [`MAX_IMAGE_BYTES`] (counting what holding each costs) or its
    /// [`MAX_IMAGES`] entries have no room for.
    kept: BTreeMap<u64, String>,
    kept_seq: u64,
    /// What holding every image in `images` costs ([`RealmImage::held_bytes`]).
    held: usize,
    /// Images refused for the decoded cap.
    refused_images: u64,
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TextUse {
    /// `text`, `tooltip` and `placeholder`.
    plain: usize,
    /// A richtext's markup.
    rich: usize,
    /// A list's rows.
    rows: usize,
}

impl TextUse {
    fn of(kind: Kind, props: &dyn Lookup) -> TextUse {
        TextUse {
            plain: text_of(props, Prop::Tooltip).len()
                + text_of(props, Prop::Placeholder).len()
                + text_of(props, Prop::Text).len(),
            rich: if kind == Kind::RichText {
                text_of(props, Prop::RichText).len()
            } else {
                0
            },
            rows: rows_of(props).iter().map(String::len).sum(),
        }
    }

    /// The text of a control of `kind` that has `self` over props `base`, once `given` is set. A rich text's markup and a list's
    /// rows are what the op gives (nothing else sets them). The plain text is counted afresh from the props as they stand over
    /// `base`, not by taking away what `base` held of a prop: a text field's text is also the player's to change by typing, so
    /// what `base` holds of it need not be what `self` counted, and taking that away from `self` could go below nothing. Only the
    /// three small props are read, and only when the op sets one of them.
    fn after(self, kind: Kind, base: &dyn Lookup, given: &[(Prop, PropValue)]) -> TextUse {
        let mut text = self;
        let mut plain = false;
        for (prop, value) in given {
            match prop {
                Prop::Tooltip | Prop::Placeholder | Prop::Text => plain = true,
                Prop::RichText if kind == Kind::RichText => text.rich = ui::text_bytes(value),
                Prop::Rows => text.rows = ui::text_bytes(value),
                _ => {}
            }
        }
        if plain {
            let view = Over {
                top: &Given(given),
                base,
            };
            text.plain = text_of(&view, Prop::Tooltip).len()
                + text_of(&view, Prop::Placeholder).len()
                + text_of(&view, Prop::Text).len();
        }
        text
    }

    fn total(&self) -> usize {
        self.plain + self.rich + self.rows
    }

    fn within_caps(&self) -> bool {
        self.plain <= MAX_CONTROL_TEXT && self.rich <= MAX_RICH_TEXT && self.rows <= MAX_LIST_TEXT
    }
}

/// Whether a control's text keeps its caps.
fn check_text(text: TextUse) -> Result<(), Violation> {
    if !text.within_caps() {
        return Err(Violation("a control's text is past its cap"));
    }
    Ok(())
}

/// The rules between a control's props that one prop alone does not show (the text is [`check_text`]'s, the image
/// [`Realm::image_for`]'s).
fn check_combined(kind: Kind, props: &dyn Lookup) -> Result<(), Violation> {
    if rows_of(props).len() < int_of(props, Prop::Selected) as usize {
        return Err(Violation("a selected row past the rows"));
    }
    let (lo, hi) = vec2_of(props, Prop::Range);
    let value = num_of(props, Prop::SliderValue);
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
            // The realm has not been told the screen yet: its first frame sends it (`ui.screen()` would read 0 until the screen
            // changed otherwise).
            l.screen = (0, 0);
        }
        Some(Realm {
            grants,
            nodes: HashMap::new(),
            next: 1,
            roots: HashMap::new(),
            controls: 0,
            text_total: 0,
            images: HashMap::new(),
            image_bytes: 0,
            kept: BTreeMap::new(),
            kept_seq: 0,
            held: 0,
            refused_images: 0,
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

    /// How many images the realm holds, shown or kept.
    #[cfg(test)]
    pub(crate) fn image_entries(&self) -> usize {
        self.images.len()
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

    /// Applies one flush of the realm's ops at `now_us` (the host's clock), once [`Realm::check_flush`] found it whole: the `ui`
    /// ops are applied; the other records are checked against the grants and given back. No op lays a layout out at once: a
    /// resize of a container of the layout's is left to the frame's layout check, which lays it out once.
    ///
    /// # Errors
    /// The first violation: the flush does not follow the format, an op is of a tier the realm lacks, or a `ui` op breaks a
    /// rule or cap; then nothing of the flush was applied. Past the check, only a control the script engine cannot make,
    /// place or delete stops a flush part way (the ops before it were applied); the host stops the realm either way.
    pub fn apply_flush(
        &mut self,
        vm: &mut Vm,
        gui: &Gui,
        flush: &[u8],
        now_us: u64,
    ) -> Result<Applied, Violation> {
        self.check_flush(gui, flush)?;
        self.apply_checked(vm, gui, flush, now_us)
    }

    /// [`Realm::apply_flush`] without the check of the whole flush first: for a piece of a flush [`Realm::check_flush`] passed
    /// whole, with nothing applied since but its pieces before this one. Every op is still checked as it applies (the check
    /// first only makes a refused flush change nothing); a host that applies a flush in pieces saves checking each again.
    ///
    /// # Errors
    /// As [`Realm::apply_flush`]'s, the ops before the one refused applied.
    pub fn apply_checked(
        &mut self,
        vm: &mut Vm,
        gui: &Gui,
        flush: &[u8],
        now_us: u64,
    ) -> Result<Applied, Violation> {
        gui.set_client_script_clock(now_us);
        // Whatever this flush does, the layers work out again what the tree shows.
        invalidate(gui);
        gui.touch_custom();
        let saved = vm.begin_host_budget(MAX_FRAME_WORK);
        gui.0.borrow_mut().client_script.applying = true;
        let result = self.apply_records(vm, gui, flush);
        let work = vm.end_host_budget(saved);
        let l = &mut gui.0.borrow_mut().client_script;
        l.applying = false;
        l.stats.apply_work += work;
        result
    }

    fn apply_records(
        &mut self,
        vm: &mut Vm,
        gui: &Gui,
        flush: &[u8],
    ) -> Result<Applied, Violation> {
        let mut applied = Applied::default();
        for record in Records::new(flush, MAX_OPS, MAX_FLUSH_BYTES)? {
            let record = record?;
            let tier = Tier::from_id(record.tier).ok_or(Violation("an op of an unknown tier"))?;
            if !self.grants.has(tier) {
                return Err(Violation("an op of a tier the realm does not have"));
            }
            match tier {
                Tier::Ui => {
                    self.apply_ui(vm, gui, &record, &mut applied)?;
                    gui.0.borrow_mut().client_script.stats.applied_records += 1;
                }
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

    /// Checks a flush whole against the realm as it is, as [`Realm::apply_flush`] would apply it, and changes nothing: every
    /// rule and cap of every op, each op seeing what the ones before it did. A flush this passes applies whole (unless the
    /// script engine cannot make a control).
    ///
    /// # Errors
    /// The first violation, as [`Realm::apply_flush`] would give it.
    pub fn check_flush(&self, gui: &Gui, flush: &[u8]) -> Result<(), Violation> {
        let mut dry = Dry::new(self, gui);
        for record in Records::new(flush, MAX_OPS, MAX_FLUSH_BYTES)? {
            let record = record?;
            let tier = Tier::from_id(record.tier).ok_or(Violation("an op of an unknown tier"))?;
            if !self.grants.has(tier) {
                return Err(Violation("an op of a tier the realm does not have"));
            }
            match tier {
                Tier::Ui => dry.op(&record)?,
                Tier::Realm | Tier::Net => {}
                _ => return Err(Violation("an op of a tier format 1 has no ops for")),
            }
        }
        Ok(())
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
            op::ui::DESTROY => self.destroy(vm, gui, &mut r),
            op::ui::PARENT => self.reparent(vm, gui, &mut r),
            op::ui::ORDER => self.order(vm, gui, &mut r),
            op::ui::INTERACTIVE => {
                let on = r.bool()?;
                r.finish()?;
                let l = &mut gui.0.borrow_mut().client_script;
                if on && l.interactive_blocked && !l.activated() {
                    // After an Escape the cursor comes back only for a request that follows the player's own action closely.
                    applied.interactive_refused += 1;
                } else {
                    if on {
                        l.interactive_blocked = false;
                    }
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
        self.nodes.insert(handle, Node::new(obj, 0, None, 0, 0));
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
        let container = kind_def.kind.container();
        let (parent_obj, depth, containers) = {
            let p = self.live(parent)?;
            if is_field(p) {
                return Err(Violation("a control under a text field"));
            }
            if p.is_container() && p.children.len() >= MAX_CONTAINER_CHILDREN {
                return Err(Violation("a container with more children than the cap"));
            }
            (
                p.obj,
                p.depth + 1,
                p.containers + usize::from(container.is_some()),
            )
        };
        if depth > MAX_DEPTH || self.controls >= MAX_CONTROLS {
            return Err(Violation("a control past the depth or count cap"));
        }
        if containers > MAX_CONTAINER_DEPTH {
            return Err(Violation("containers nested past the cap"));
        }
        let mut props = Props::default();
        for (prop, value) in &given {
            props.set(*prop, value.clone());
        }
        let text = TextUse::of(kind_def.kind, &props);
        check_text(text)?;
        check_combined(kind_def.kind, &props)?;
        let mut image = props.text(Prop::Image).to_string();
        self.check_totals(text.total(), &image, 0, None)?;
        let mut shown = self.image_for(&image)?;
        if !self.image_fits(&image, shown.as_ref(), None) {
            (image, shown) = (String::new(), None);
            self.refuse_image(vm, gui);
        }
        let layout = gui.realm_layout();
        let fields = layout_fields(layout, kind_def.kind, &given);
        let (obj, laid) = make_control(vm, gui, kind_def.kind, layout, &fields)
            .ok_or(Violation("a control that could not be made"))?;
        let held = if container.is_some() {
            props.clone()
        } else {
            Props::default()
        };
        // A container of the layout's class keeps no state of the realm's (its class is the layout's, so this finds none); a
        // group made for an engine without a layout, and every other control, does.
        gui.with_custom_state::<NodeState, _>(obj, |s| {
            *s = NodeState::new(handle, container.is_none().then_some(kind_def.kind));
            s.set_props(props, shown.clone());
        });
        place_defaults(vm, gui, obj, &given);
        if control::add_child(vm, gui, parent_obj, obj).is_err() {
            discard(vm, obj);
            return Err(Violation("a control that could not be placed"));
        }
        if laid {
            gui.0.borrow_mut().client_script.laid.insert(obj);
        }
        self.next += 1;
        self.add_counts(text.total(), &image, shown);
        self.controls += 1;
        if let Some(p) = self.nodes.get_mut(&parent) {
            p.children.push(handle);
        }
        let mut node = Node::new(obj, parent, Some(kind_def.kind), depth, containers);
        node.text = text;
        node.image = (!image.is_empty()).then_some(image);
        node.held = held;
        self.nodes.insert(handle, node);
        if containers > 0 {
            self.mark_layouts_from(gui, handle);
        }
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
        let known = self.in_use(image) || old_image == Some(image);
        if !image.is_empty() && !known && self.images.len() - self.kept.len() >= MAX_IMAGES {
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

    /// Whether controls show the image at `path`.
    fn in_use(&self, path: &str) -> bool {
        self.images.get(path).is_some_and(|used| used.count > 0)
    }

    /// Whether a control showing `path` (shown as `image`) in place of `old_image` keeps the images controls show within the
    /// decoded cap (the kept ones give way).
    fn image_fits(&self, path: &str, image: Option<&RealmImage>, old_image: Option<&str>) -> bool {
        let Some(image) = image.filter(|_| !self.in_use(path)) else {
            return true;
        };
        let freed = old_image
            .filter(|old| *old != path)
            .and_then(|old| self.images.get(old))
            .filter(|used| used.count == 1)
            .map_or(0, |used| used.image.decoded_bytes());
        within_image_bytes(self.image_bytes, freed, image.decoded_bytes())
    }

    fn add_counts(&mut self, text: usize, path: &str, image: Option<RealmImage>) {
        self.text_total += text;
        let Some(image) = image else {
            return;
        };
        let bytes = image.decoded_bytes();
        match self.images.get_mut(path) {
            Some(used) => {
                if let Some(at) = used.kept_at.take() {
                    self.kept.remove(&at);
                    self.image_bytes += bytes;
                }
                used.count += 1;
            }
            None => {
                // The kept images give way, oldest first, so what all of them hold stays within the caps.
                let cost = image.held_bytes();
                while (self.held + cost > MAX_IMAGE_BYTES || self.images.len() >= MAX_IMAGES)
                    && let Some((_, oldest)) = self.kept.pop_first()
                {
                    if let Some(gone) = self.images.remove(&oldest) {
                        self.held -= gone.image.held_bytes();
                    }
                }
                self.image_bytes += bytes;
                self.held += cost;
                self.images.insert(
                    path.to_string(),
                    ImageUse {
                        count: 1,
                        image,
                        kept_at: None,
                    },
                );
            }
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
                self.image_bytes -= used.image.decoded_bytes();
                self.kept_seq += 1;
                used.kept_at = Some(self.kept_seq);
                self.kept.insert(self.kept_seq, image.to_string());
            }
        }
    }

    fn set(&mut self, vm: &mut Vm, gui: &Gui, r: &mut Reader<'_>) -> Result<(), Violation> {
        let handle = r.u32()?;
        let (obj, kind, old_text, old_image, in_layout) = {
            let n = self.control(handle)?;
            (n.obj, n.kind, n.text, n.image.clone(), n.containers > 0)
        };
        let kind = kind.ok_or(Violation("a mod root where a control is needed"))?;
        let given = ui::read_props(r, ui::kind_def(kind))?;
        r.finish()?;
        let container = kind.container().is_some();
        // The control's props are read where they are, under what the op gives: nothing of them is copied.
        let old = old_image.as_deref();
        let (text, image, shown, refused) = if container {
            self.check_set(kind, &self.live(handle)?.held, &given, old_text, old)?
        } else {
            gui.with_custom_state::<NodeState, _>(obj, |s| {
                self.check_set(kind, &s.props, &given, old_text, old)
            })
            .ok_or(Violation("a control with no state"))??
        };
        if refused {
            self.refuse_image(vm, gui);
        }
        if container && let Some(n) = self.nodes.get_mut(&handle) {
            for (prop, value) in &given {
                n.held.set(*prop, value.clone());
            }
        }
        // A realm group keeps the props too; a container of the layout's class has no state of the realm's to set.
        gui.with_custom_state::<NodeState, _>(obj, |s| s.merge_props(&given, shown.clone()));
        apply_layout(vm, gui, obj, &given);
        write_fields(vm, obj, &layout_fields(gui.realm_layout(), kind, &given));
        if in_layout {
            self.mark_layouts_from(gui, handle);
        }
        self.drop_counts(old_text.total(), old);
        self.add_counts(text.total(), &image, shown);
        if let Some(n) = self.nodes.get_mut(&handle) {
            n.text = text;
            n.image = (!image.is_empty()).then_some(image);
        }
        Ok(())
    }

    /// The checks of a set of `given` on a control of `kind` whose props are `base`, its text `old_text` and its image
    /// `old_image`: its text, image path and image afterwards, and whether the image was refused for the decoded cap (then
    /// it shows none).
    fn check_set(
        &self,
        kind: Kind,
        base: &dyn Lookup,
        given: &[(Prop, PropValue)],
        old_text: TextUse,
        old_image: Option<&str>,
    ) -> Result<(TextUse, String, Option<RealmImage>, bool), Violation> {
        let view = Over {
            top: &Given(given),
            base,
        };
        let text = old_text.after(kind, base, given);
        check_text(text)?;
        check_combined(kind, &view)?;
        let image = text_of(&view, Prop::Image).to_string();
        self.check_totals(text.total(), &image, old_text.total(), old_image)?;
        let shown = self.image_for(&image)?;
        if !self.image_fits(&image, shown.as_ref(), old_image) {
            return Ok((text, String::new(), None, true));
        }
        Ok((text, image, shown, false))
    }

    /// An image past the decoded cap: it is not shown (content past a cap is no protocol violation), counted, and the first
    /// one of a realm is noted on the console.
    fn refuse_image(&mut self, vm: &mut Vm, gui: &Gui) {
        self.refused_images += 1;
        gui.0.borrow_mut().client_script.stats.refused_images += 1;
        if self.refused_images == 1 {
            vm.warn(format!(
                "server scripts: an image past the {} MiB of pictures a server may show is not shown",
                MAX_IMAGE_BYTES >> 20
            ));
        }
    }

    /// Something at `handle` changed what a layout reads: the containers on the way up from it (itself counted) are checked
    /// again for the next frames ([`super::layers::mark_layouts`]), and no others.
    fn mark_layouts_from(&self, gui: &Gui, handle: u32) {
        let mut containers = Vec::new();
        let mut at = handle;
        while let Some(n) = self.nodes.get(&at)
            && n.kind.is_some()
        {
            if n.is_container() {
                containers.push(n.obj);
            }
            at = n.parent;
        }
        super::layers::mark_layouts(gui, containers);
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

    fn destroy(&mut self, vm: &mut Vm, gui: &Gui, r: &mut Reader<'_>) -> Result<(), Violation> {
        let handle = r.u32()?;
        r.finish()?;
        let (parent, in_layout) = {
            let n = self.control(handle)?;
            (n.parent, n.containers > 0)
        };
        self.remove_subtree(vm, handle)?;
        if in_layout {
            self.mark_layouts_from(gui, parent);
        }
        Ok(())
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
            self.drop_counts(node.text.total(), node.image.as_deref());
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
        let (parent_obj, parent_depth, parent_containers) = {
            let p = self.live(new_parent)?;
            if is_field(p) {
                return Err(Violation("a control moved under a text field"));
            }
            if p.is_container()
                && new_parent != old_parent
                && p.children.len() >= MAX_CONTAINER_CHILDREN
            {
                return Err(Violation("a container with more children than the cap"));
            }
            (p.obj, p.depth, p.containers)
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
        let old_containers = self.live(old_parent)?.containers;
        let nested = below
            .iter()
            .filter_map(|h| self.nodes.get(h))
            .map(|n| n.containers - old_containers)
            .max()
            .unwrap_or(0);
        if parent_containers + nested > MAX_CONTAINER_DEPTH {
            return Err(Violation("containers moved past the nesting cap"));
        }
        control::add_child(vm, gui, parent_obj, obj)
            .map_err(|_| Violation("a control that could not be moved"))?;
        if let Some(p) = self.nodes.get_mut(&old_parent) {
            p.children.retain(|c| *c != handle);
        }
        if let Some(p) = self.nodes.get_mut(&new_parent) {
            p.children.push(handle);
        }
        for &h in &below {
            if let Some(n) = self.nodes.get_mut(&h) {
                n.depth = n.depth - old_depth + new_depth;
                n.containers = n.containers - old_containers + parent_containers;
            }
        }
        if let Some(n) = self.nodes.get_mut(&handle) {
            n.parent = new_parent;
        }
        if old_containers > 0 || parent_containers > 0 {
            // The containers it left, the ones it joined, and the ones it carries (their depth below a layout's top changed).
            self.mark_layouts_from(gui, old_parent);
            for h in below {
                self.mark_layouts_from(gui, h);
            }
        }
        Ok(())
    }

    fn order(&mut self, vm: &mut Vm, gui: &Gui, r: &mut Reader<'_>) -> Result<(), Violation> {
        let (handle, front) = (r.u32()?, r.bool()?);
        r.finish()?;
        let (obj, parent, in_layout) = {
            let n = self.control(handle)?;
            (n.obj, n.parent, n.containers > 0)
        };
        let parent_obj = self.live(parent)?.obj;
        // The member list starts with the control drawn first.
        vm.objects_mut().move_member(parent_obj, obj, !front);
        if in_layout {
            self.mark_layouts_from(gui, parent);
        }
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
        self.images.clear();
        self.kept.clear();
        self.held = 0;
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
            || self.image_bytes > MAX_IMAGE_BYTES
        {
            return Err("a cap is broken".into());
        }
        let decoded: usize = self
            .images
            .values()
            .filter(|u| u.count > 0)
            .map(|u| u.image.decoded_bytes())
            .sum();
        let held: usize = self.images.values().map(|u| u.image.held_bytes()).sum();
        let kept = self.images.values().filter(|u| u.count == 0).count();
        let placed = self.kept.iter().all(|(at, path)| {
            self.images
                .get(path)
                .is_some_and(|u| u.count == 0 && u.kept_at == Some(*at))
        });
        if held != self.held || kept != self.kept.len() || !placed {
            return Err(format!(
                "{kept} kept images holding {held} bytes but {} counted holding {}",
                self.kept.len(),
                self.held
            ));
        }
        if decoded != self.image_bytes {
            return Err(format!(
                "{decoded} bytes of images but {} counted",
                self.image_bytes
            ));
        }
        let counted = self.nodes.values().filter(|n| n.kind.is_some()).count();
        if counted != self.controls {
            return Err(format!("{counted} controls but {} counted", self.controls));
        }
        for (handle, node) in &self.nodes {
            if node.depth > MAX_DEPTH || node.depth > MAX_TREE_DEPTH {
                return Err(format!("handle {handle} is {} deep", node.depth));
            }
            if node.containers > MAX_CONTAINER_DEPTH {
                return Err(format!(
                    "handle {handle} is in {} containers",
                    node.containers
                ));
            }
            if node.is_container() && node.children.len() > MAX_CONTAINER_CHILDREN {
                return Err(format!(
                    "container {handle} has {} children",
                    node.children.len()
                ));
            }
            let above = self.nodes.get(&node.parent).map_or(0, |p| p.containers);
            if node.containers != above + usize::from(node.is_container()) {
                return Err(format!("handle {handle} counts its containers wrong"));
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

/// Whether images of `bytes` decoded, less `freed`, plus `added` stay within the realm's decoded cap.
fn within_image_bytes(bytes: usize, freed: usize, added: usize) -> bool {
    bytes - freed + added <= MAX_IMAGE_BYTES
}

/// A node as the check of a flush has it once an op of the flush changed it.
#[derive(Clone, Debug)]
struct Shadow {
    parent: u32,
    kind: Option<Kind>,
    depth: usize,
    children: Vec<u32>,
    text: TextUse,
    image: Option<String>,
    containers: usize,
    /// The props the flush set on it, over the realm's (all of them for a node the flush made).
    set: Props,
    /// The flush made it: the realm has no props of it.
    made: bool,
}

impl Shadow {
    fn of(node: &Node) -> Shadow {
        Shadow {
            parent: node.parent,
            kind: node.kind,
            depth: node.depth,
            children: node.children.clone(),
            text: node.text,
            image: node.image.clone(),
            containers: node.containers,
            set: Props::default(),
            made: false,
        }
    }

    fn new(parent: u32, kind: Option<Kind>, depth: usize, containers: usize) -> Shadow {
        Shadow {
            parent,
            kind,
            depth,
            children: Vec::new(),
            text: TextUse::default(),
            image: None,
            containers,
            set: Props::default(),
            made: true,
        }
    }
}

/// A live node as the check sees it: the realm's own, or the flush's change of it.
#[derive(Clone, Copy)]
enum Seen<'a> {
    Realm(&'a Node),
    Flush(&'a Shadow),
}

impl<'a> Seen<'a> {
    fn parent(self) -> u32 {
        match self {
            Seen::Realm(n) => n.parent,
            Seen::Flush(n) => n.parent,
        }
    }

    fn kind(self) -> Option<Kind> {
        match self {
            Seen::Realm(n) => n.kind,
            Seen::Flush(n) => n.kind,
        }
    }

    fn depth(self) -> usize {
        match self {
            Seen::Realm(n) => n.depth,
            Seen::Flush(n) => n.depth,
        }
    }

    fn containers(self) -> usize {
        match self {
            Seen::Realm(n) => n.containers,
            Seen::Flush(n) => n.containers,
        }
    }

    fn children(self) -> &'a [u32] {
        match self {
            Seen::Realm(n) => &n.children,
            Seen::Flush(n) => &n.children,
        }
    }

    fn text(self) -> TextUse {
        match self {
            Seen::Realm(n) => n.text,
            Seen::Flush(n) => n.text,
        }
    }

    fn image(self) -> Option<&'a str> {
        match self {
            Seen::Realm(n) => n.image.as_deref(),
            Seen::Flush(n) => n.image.as_deref(),
        }
    }

    fn is_container(self) -> bool {
        self.kind().is_some_and(|k| k.container().is_some())
    }

    fn is_field(self) -> bool {
        self.kind() == Some(Kind::TextField)
    }
}

/// An image path whose use a flush changed: how many controls show it now, and what it takes decoded.
#[derive(Clone, Copy, Debug)]
struct ImageCount {
    count: usize,
    bytes: usize,
}

/// A flush applied to the realm's books alone, without objects: [`Realm::check_flush`]. Each op is checked as
/// [`Realm::apply_flush`] checks it, in the same order, against the realm as the ops before it left it; what the flush changed
/// is kept beside the realm's books, which stay as they are.
struct Dry<'r> {
    realm: &'r Realm,
    gui: &'r Gui,
    /// The nodes the flush changed or made; `None` once removed.
    nodes: HashMap<u32, Option<Shadow>>,
    next: u32,
    /// The mod roots the flush made.
    roots: HashSet<String>,
    controls: usize,
    text_total: usize,
    images: HashMap<String, ImageCount>,
    /// The distinct images in use, and their decoded bytes.
    distinct: usize,
    image_bytes: usize,
}

impl<'r> Dry<'r> {
    fn new(realm: &'r Realm, gui: &'r Gui) -> Dry<'r> {
        Dry {
            realm,
            gui,
            nodes: HashMap::new(),
            next: realm.next,
            roots: HashSet::new(),
            controls: realm.controls,
            text_total: realm.text_total,
            images: HashMap::new(),
            distinct: realm.images.len() - realm.kept.len(),
            image_bytes: realm.image_bytes,
        }
    }

    fn op(&mut self, record: &Record<'_>) -> Result<(), Violation> {
        let mut r = record.reader();
        match record.kind {
            op::ui::ROOT => self.root(&mut r),
            op::ui::CREATE => self.create(&mut r),
            op::ui::SET => self.set(&mut r),
            op::ui::DESTROY => self.destroy(&mut r),
            op::ui::PARENT => self.reparent(&mut r),
            op::ui::ORDER => {
                let (handle, _front) = (r.u32()?, r.bool()?);
                r.finish()?;
                let parent = self.control(handle)?.parent();
                self.live(parent).map(|_| ())
            }
            op::ui::INTERACTIVE => {
                r.bool()?;
                r.finish()?;
                Ok(())
            }
            op::ui::FOCUS => {
                let handle = r.u32()?;
                r.finish()?;
                if self.control(handle)?.kind() != Some(Kind::TextField) {
                    return Err(Violation("focus on a control that is not a text field"));
                }
                Ok(())
            }
            _ => Err(Violation("an unknown ui op")),
        }
    }

    fn seen(&self, handle: u32) -> Option<Seen<'_>> {
        match self.nodes.get(&handle) {
            Some(Some(shadow)) => Some(Seen::Flush(shadow)),
            Some(None) => None,
            None => self.realm.nodes.get(&handle).map(Seen::Realm),
        }
    }

    fn live(&self, handle: u32) -> Result<Seen<'_>, Violation> {
        self.seen(handle)
            .ok_or(Violation("a handle that is not live"))
    }

    fn control(&self, handle: u32) -> Result<Seen<'_>, Violation> {
        let node = self.live(handle)?;
        if node.kind().is_none() {
            return Err(Violation("a mod root where a control is needed"));
        }
        Ok(node)
    }

    /// The flush's own copy of a live node, made from the realm's on first change.
    fn shadow(&mut self, handle: u32) -> Option<&mut Shadow> {
        if !self.nodes.contains_key(&handle) {
            let node = self.realm.nodes.get(&handle)?;
            self.nodes.insert(handle, Some(Shadow::of(node)));
        }
        self.nodes.get_mut(&handle)?.as_mut()
    }

    fn root(&mut self, r: &mut Reader<'_>) -> Result<(), Violation> {
        let (handle, mod_id) = (r.u32()?, r.text()?);
        r.finish()?;
        abi::check_mod_id(mod_id).map_err(Violation)?;
        let taken = self.realm.roots.contains_key(mod_id) || self.roots.contains(mod_id);
        if handle != self.next || taken || self.realm.roots.len() + self.roots.len() >= MAX_ROOTS {
            return Err(Violation(
                "a mod root that is not the next, or a second one",
            ));
        }
        if self.gui.0.borrow().client_script.server.is_none() {
            return Err(Violation("no server layer"));
        }
        self.next += 1;
        self.roots.insert(mod_id.to_string());
        self.nodes.insert(handle, Some(Shadow::new(0, None, 0, 0)));
        Ok(())
    }

    fn create(&mut self, r: &mut Reader<'_>) -> Result<(), Violation> {
        let (handle, parent, kind_id) = (r.u32()?, r.u32()?, r.u8()?);
        let kind_def = ui::kind_from_id(kind_id).ok_or(Violation("an unknown kind"))?;
        let given = ui::read_props(r, kind_def)?;
        r.finish()?;
        if handle != self.next {
            return Err(Violation("a handle that is not the next number"));
        }
        let container = kind_def.kind.container();
        let (depth, containers) = {
            let p = self.live(parent)?;
            if p.is_field() {
                return Err(Violation("a control under a text field"));
            }
            if p.is_container() && p.children().len() >= MAX_CONTAINER_CHILDREN {
                return Err(Violation("a container with more children than the cap"));
            }
            (
                p.depth() + 1,
                p.containers() + usize::from(container.is_some()),
            )
        };
        if depth > MAX_DEPTH || self.controls >= MAX_CONTROLS {
            return Err(Violation("a control past the depth or count cap"));
        }
        if containers > MAX_CONTAINER_DEPTH {
            return Err(Violation("containers nested past the cap"));
        }
        let mut props = Props::default();
        for (prop, value) in &given {
            props.set(*prop, value.clone());
        }
        let text = TextUse::of(kind_def.kind, &props);
        check_text(text)?;
        check_combined(kind_def.kind, &props)?;
        let mut image = props.text(Prop::Image).to_string();
        self.check_totals(text.total(), &image, 0, None)?;
        let mut bytes = self.image_for(&image)?;
        if !self.image_fits(&image, bytes, None) {
            (image, bytes) = (String::new(), 0);
        }
        self.next += 1;
        self.add_counts(text.total(), &image, bytes);
        self.controls += 1;
        if let Some(p) = self.shadow(parent) {
            p.children.push(handle);
        }
        let mut node = Shadow::new(parent, Some(kind_def.kind), depth, containers);
        node.text = text;
        node.image = (!image.is_empty()).then_some(image);
        node.set = props;
        self.nodes.insert(handle, Some(node));
        Ok(())
    }

    fn set(&mut self, r: &mut Reader<'_>) -> Result<(), Violation> {
        let handle = r.u32()?;
        let (kind, old_text, old_image) = {
            let n = self.control(handle)?;
            (n.kind(), n.text(), n.image().map(str::to_string))
        };
        let kind = kind.ok_or(Violation("a mod root where a control is needed"))?;
        let given = ui::read_props(r, ui::kind_def(kind))?;
        r.finish()?;
        let old = old_image.as_deref();
        let (text, image, bytes) = self.with_props(handle, kind, |base| {
            self.check_set(kind, base, &given, old_text, old)
        })??;
        self.drop_counts(old_text.total(), old);
        self.add_counts(text.total(), &image, bytes);
        if let Some(n) = self.shadow(handle) {
            n.text = text;
            n.image = (!image.is_empty()).then_some(image);
            for (prop, value) in given {
                n.set.set(prop, value);
            }
        }
        Ok(())
    }

    /// [`Realm::check_set`] against the books: the image as its decoded bytes.
    fn check_set(
        &self,
        kind: Kind,
        base: &dyn Lookup,
        given: &[(Prop, PropValue)],
        old_text: TextUse,
        old_image: Option<&str>,
    ) -> Result<(TextUse, String, usize), Violation> {
        let view = Over {
            top: &Given(given),
            base,
        };
        let text = old_text.after(kind, base, given);
        check_text(text)?;
        check_combined(kind, &view)?;
        let image = text_of(&view, Prop::Image).to_string();
        self.check_totals(text.total(), &image, old_text.total(), old_image)?;
        let bytes = self.image_for(&image)?;
        if !self.image_fits(&image, bytes, old_image) {
            return Ok((text, String::new(), 0));
        }
        Ok((text, image, bytes))
    }

    /// `f` of the props of the live control `handle`, read where they are: what the flush set, over what the realm keeps (a
    /// container's in its node, the others' in their state).
    fn with_props<R>(
        &self,
        handle: u32,
        kind: Kind,
        f: impl FnOnce(&dyn Lookup) -> R,
    ) -> Result<R, Violation> {
        let none = Given(&[]);
        let (top, made): (&dyn Lookup, bool) = match self.nodes.get(&handle) {
            Some(Some(shadow)) => (&shadow.set, shadow.made),
            _ => (&none, false),
        };
        if made {
            return Ok(f(top));
        }
        let node = self
            .realm
            .nodes
            .get(&handle)
            .ok_or(Violation("a handle that is not live"))?;
        if kind.container().is_some() {
            return Ok(f(&Over {
                top,
                base: &node.held,
            }));
        }
        self.gui
            .with_custom_state::<NodeState, _>(node.obj, |s| {
                f(&Over {
                    top,
                    base: &s.props,
                })
            })
            .ok_or(Violation("a control with no state"))
    }

    fn subtree(&self, handle: u32) -> Vec<u32> {
        let mut out = vec![handle];
        let mut at = 0;
        while let Some(&h) = out.get(at) {
            at += 1;
            if let Some(n) = self.seen(h) {
                out.extend(n.children());
            }
        }
        out
    }

    fn destroy(&mut self, r: &mut Reader<'_>) -> Result<(), Violation> {
        let handle = r.u32()?;
        r.finish()?;
        let parent = self.control(handle)?.parent();
        for h in self.subtree(handle) {
            let Some(node) = self.seen(h) else {
                continue;
            };
            let (text, image, control) = (
                node.text().total(),
                node.image().map(str::to_string),
                node.kind().is_some(),
            );
            self.drop_counts(text, image.as_deref());
            if control {
                self.controls -= 1;
            }
            self.nodes.insert(h, None);
        }
        if let Some(p) = self.shadow(parent) {
            p.children.retain(|c| *c != handle);
        }
        Ok(())
    }

    fn reparent(&mut self, r: &mut Reader<'_>) -> Result<(), Violation> {
        let (handle, new_parent) = (r.u32()?, r.u32()?);
        r.finish()?;
        let (old_parent, old_depth) = {
            let n = self.control(handle)?;
            (n.parent(), n.depth())
        };
        let (parent_depth, parent_containers) = {
            let p = self.live(new_parent)?;
            if p.is_field() {
                return Err(Violation("a control moved under a text field"));
            }
            if p.is_container()
                && new_parent != old_parent
                && p.children().len() >= MAX_CONTAINER_CHILDREN
            {
                return Err(Violation("a container with more children than the cap"));
            }
            (p.depth(), p.containers())
        };
        let below = self.subtree(handle);
        if below.contains(&new_parent) {
            return Err(Violation("a control moved under itself"));
        }
        let height = below
            .iter()
            .filter_map(|h| self.seen(*h))
            .map(|n| n.depth() - old_depth)
            .max()
            .unwrap_or(0);
        let new_depth = parent_depth + 1;
        if new_depth + height > MAX_DEPTH {
            return Err(Violation("a control moved past the depth cap"));
        }
        let old_containers = self.live(old_parent)?.containers();
        let nested = below
            .iter()
            .filter_map(|h| self.seen(*h))
            .map(|n| n.containers() - old_containers)
            .max()
            .unwrap_or(0);
        if parent_containers + nested > MAX_CONTAINER_DEPTH {
            return Err(Violation("containers moved past the nesting cap"));
        }
        if let Some(p) = self.shadow(old_parent) {
            p.children.retain(|c| *c != handle);
        }
        if let Some(p) = self.shadow(new_parent) {
            p.children.push(handle);
        }
        for &h in &below {
            if let Some(n) = self.shadow(h) {
                n.depth = n.depth - old_depth + new_depth;
                n.containers = n.containers - old_containers + parent_containers;
            }
        }
        if let Some(n) = self.shadow(handle) {
            n.parent = new_parent;
        }
        Ok(())
    }

    /// How many controls show `path` now, and what it takes decoded.
    fn image_use(&self, path: &str) -> Option<ImageCount> {
        if let Some(found) = self.images.get(path) {
            return Some(*found).filter(|c| c.count > 0);
        }
        self.realm
            .images
            .get(path)
            .filter(|u| u.count > 0)
            .map(|used| ImageCount {
                count: used.count,
                bytes: used.image.decoded_bytes(),
            })
    }

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
        let known = self.image_use(image).is_some() || old_image == Some(image);
        if !image.is_empty() && !known && self.distinct >= MAX_IMAGES {
            return Err(Violation("more distinct images than the cap"));
        }
        Ok(())
    }

    /// The decoded bytes of the image at `path` (0 for none), as [`Realm::image_for`] would find it.
    fn image_for(&self, path: &str) -> Result<usize, Violation> {
        if path.is_empty() {
            return Ok(0);
        }
        if let Some(used) = self.image_use(path) {
            return Ok(used.bytes);
        }
        (self.realm.content)(path)
            .and_then(|bytes| RealmImage::header(path, &bytes))
            .map(|(_, size)| RealmImage::decoded_bytes_of(size))
            .ok_or(Violation("an image that is not this server's content"))
    }

    fn image_fits(&self, path: &str, bytes: usize, old_image: Option<&str>) -> bool {
        if path.is_empty() || self.image_use(path).is_some() {
            return true;
        }
        let freed = old_image
            .filter(|old| *old != path)
            .and_then(|old| self.image_use(old))
            .filter(|used| used.count == 1)
            .map_or(0, |used| used.bytes);
        within_image_bytes(self.image_bytes, freed, bytes)
    }

    fn add_counts(&mut self, text: usize, path: &str, bytes: usize) {
        self.text_total += text;
        if path.is_empty() {
            return;
        }
        let count = self.image_use(path).map_or(0, |used| used.count);
        if count == 0 {
            self.distinct += 1;
            self.image_bytes += bytes;
        }
        self.images.insert(
            path.to_string(),
            ImageCount {
                count: count + 1,
                bytes,
            },
        );
    }

    fn drop_counts(&mut self, text: usize, image: Option<&str>) {
        self.text_total = self.text_total.saturating_sub(text);
        let Some(path) = image else {
            return;
        };
        let Some(used) = self.image_use(path) else {
            return;
        };
        if used.count == 1 {
            self.distinct -= 1;
            self.image_bytes -= used.bytes;
        }
        self.images.insert(
            path.to_string(),
            ImageCount {
                count: used.count - 1,
                bytes: used.bytes,
            },
        );
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

/// The console fields a layout class reads, for the layout props among `given` of a control of `kind`: the words of CSS the
/// props make, under the names the layout gave them. None without a layout.
fn layout_fields(
    layout: Option<&RealmLayout>,
    kind: Kind,
    given: &[(Prop, PropValue)],
) -> Vec<(&'static str, String)> {
    let Some(layout) = layout else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (prop, value) in given {
        if let Some(name) = layout.field(*prop) {
            out.push((name, ui::field_text(ui::prop_def(*prop), value)));
        }
        // A container inside a container is laid out at the size it is given only when the layout is told it: else it follows
        // its content.
        if kind.container().is_some()
            && let (Prop::Size, PropValue::Vec2(w, h)) = (prop, value)
        {
            let (wide, tall) = layout.size_fields();
            out.push((wide, ui::number_text(w.round())));
            out.push((tall, ui::number_text(h.round())));
        }
    }
    out
}

/// Writes console fields to a control.
fn write_fields(vm: &mut Vm, obj: ObjId, fields: &[(&str, String)]) {
    for (name, text) in fields {
        vm.set_object_field(obj, name.as_bytes(), b"", text.as_bytes());
    }
}

/// Makes the object of a control of `kind` with `fields` set from the start: a container of the layout's class (then true:
/// the layout's `update` is the realm's to run), else the realm's class for the kind (a container with no layout is a group).
fn make_control(
    vm: &mut Vm,
    gui: &Gui,
    kind: Kind,
    layout: Option<&RealmLayout>,
    fields: &[(&str, String)],
) -> Option<(ObjId, bool)> {
    let raw: Vec<(&[u8], &[u8], &[u8])> = fields
        .iter()
        .map(|(name, text)| (name.as_bytes(), &b""[..], text.as_bytes()))
        .collect();
    if let (Some(container), Some(layout)) = (kind.container(), layout)
        && let Some(obj) = make_realm_control(vm, gui, layout.class(container), &raw)
    {
        return Some((obj, true));
    }
    make_realm_control(vm, gui, class_name(Some(kind)), &raw).map(|obj| (obj, false))
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
