//! The `ui` tier: controls the realm makes, as handles over its mirror, and the events of its controls.

use std::collections::{BTreeMap, HashMap};

use mlua::{
    AnyUserData, Error, FromLuaMulti, Function, Lua, LuaString, MultiValue, Table, UserData,
    UserDataMethods, Value,
};

use super::format::ui::{
    self as fui, Event, Kind, KindDef, Prop, PropDef, PropType, PropValue, Track,
};
use super::format::{Malformed, NodeBudget, Reader, Tier, Value as FormatValue, Writer, event, op};
use super::{
    CallbackId, MAX_NAME, State, add_handler, function, granted, guarded, invoke_kept, keep,
    name_of, release, short_text, state,
};
use crate::realm::table_slots;

/// The most slots a props table may have when it is read.
const MAX_PROPS_SLOTS: u64 = 256;

/// A control's props as set, in id order (the order the format writes them in).
type Props = BTreeMap<Prop, PropValue>;

/// One control (or a mod's root) in the mirror.
struct Node {
    /// `None` for a root.
    kind: Option<&'static KindDef>,
    parent: u32,
    children: Vec<u32>,
    props: Props,
    /// The bytes of its text props but a richtext's `text` and a list's `rows`, which have their own caps.
    own_text: usize,
    /// The bytes of all its text props.
    text: usize,
    depth: usize,
    /// The containers on the way from its root to it, itself counted (a root is 0).
    containers: usize,
    image: Option<String>,
    handlers: HashMap<Event, CallbackId>,
}

impl Node {
    fn new(kind: Option<&'static KindDef>, parent: u32, depth: usize, containers: usize) -> Node {
        Node {
            kind,
            parent,
            children: Vec::new(),
            props: Props::new(),
            own_text: 0,
            text: 0,
            depth,
            containers,
            image: None,
            handlers: HashMap::new(),
        }
    }

    /// Whether it is a container (`flex` or `grid`).
    fn is_container(&self) -> bool {
        self.kind.is_some_and(|k| k.kind.container().is_some())
    }

    /// The prop's value: as set, or its default.
    fn prop(&self, prop: Prop) -> PropValue {
        self.props
            .get(&prop)
            .cloned()
            .unwrap_or_else(|| fui::prop_def(prop).default.value())
    }
}

/// The mirror of the realm's controls.
#[derive(Default)]
pub(crate) struct UiState {
    nodes: HashMap<u32, Node>,
    roots: HashMap<String, u32>,
    /// The last handle given.
    next: u32,
    controls: usize,
    text: usize,
    images: HashMap<String, usize>,
    screen: (f64, f64, f64),
    screen_handlers: Vec<CallbackId>,
    closed_handlers: Vec<CallbackId>,
    /// When the last activating event of the realm's own controls arrived.
    activation: Option<u64>,
}

impl UiState {
    /// Forgets every control, for a revocation: the kept functions to let go. Handles keep counting up.
    pub(crate) fn revoke(&mut self) -> Vec<CallbackId> {
        let mut released: Vec<CallbackId> = self
            .nodes
            .values()
            .flat_map(|n| n.handlers.values().copied())
            .collect();
        released.append(&mut self.screen_handlers);
        released.append(&mut self.closed_handlers);
        *self = UiState {
            next: self.next,
            ..UiState::default()
        };
        released
    }

    fn live(&self, id: u32) -> mlua::Result<&Node> {
        self.nodes
            .get(&id)
            .ok_or_else(|| Error::runtime("ui: this control was destroyed"))
    }

    /// The live control `id`, not a root: its kind.
    fn control(&self, id: u32, what: &str) -> mlua::Result<&'static KindDef> {
        self.live(id)?.kind.ok_or_else(|| {
            Error::runtime(format!("{what}: a mod's root is never changed this way"))
        })
    }

    /// The number the next control gets.
    fn next_handle(&self) -> mlua::Result<u32> {
        self.next
            .checked_add(1)
            .ok_or_else(|| Error::runtime("ui: this realm has made all the controls it can"))
    }

    /// The control and every control under it, the control first.
    fn subtree(&self, id: u32) -> Vec<u32> {
        let mut out = vec![id];
        let mut at = 0;
        while let Some(&next) = out.get(at) {
            if let Some(node) = self.nodes.get(&next) {
                out.extend_from_slice(&node.children);
            }
            at += 1;
        }
        out
    }

    /// The image a node shows once `props` are set on it, keeping the realm's image counts.
    fn swap_image(&mut self, old: Option<String>, props: &Props) -> Option<String> {
        let Some(PropValue::Text(path)) = props.get(&Prop::Image) else {
            return old;
        };
        if let Some(old) = &old {
            self.drop_image(old);
        }
        if path.is_empty() {
            return None;
        }
        *self.images.entry(path.clone()).or_insert(0) += 1;
        Some(path.clone())
    }

    /// One control less shows `path`.
    fn drop_image(&mut self, path: &str) {
        if let Some(count) = self.images.get_mut(path) {
            *count -= 1;
            if *count == 0 {
                self.images.remove(path);
            }
        }
    }

    /// Sets `props` on the live control `id`, whose text counts become `counts` ([`check_control`]'s).
    fn apply(&mut self, id: u32, props: Props, (own_text, text): (usize, usize)) {
        let Some(node) = self.nodes.get_mut(&id) else {
            return;
        };
        let old = node.image.take();
        self.text = self.text - node.text + text;
        node.own_text = own_text;
        node.text = text;
        let image = self.swap_image(old, &props);
        if let Some(node) = self.nodes.get_mut(&id) {
            node.image = image;
            node.props.extend(props);
        }
    }
}

/// A control of the realm, as scripts hold it.
pub(crate) struct Control {
    id: u32,
    serial: u64,
}

/// The handle `value` holds, checked to be this realm's.
fn handle_of(st: &State, value: &AnyUserData, what: &str) -> mlua::Result<u32> {
    let control = value
        .borrow::<Control>()
        .map_err(|_| Error::runtime(format!("{what} takes a control")))?;
    if !st.is_own(control.serial) {
        return Err(Error::runtime(format!(
            "{what} takes a control of this realm"
        )));
    }
    Ok(control.id)
}

/// A value's number, if it is one.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(i) => Some(*i as f64),
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

/// A value's text, if it is a UTF-8 string of at most `max` bytes (checked before it is read).
fn text(value: &Value, max: usize) -> Option<String> {
    match value {
        Value::String(s) => short_text(s, max),
        _ => None,
    }
}

/// The most bytes a prop's text may have.
fn text_cap(ty: PropType) -> usize {
    match ty {
        PropType::Text { max } | PropType::Rich { max } => max,
        PropType::Image => fui::MAX_PATH,
        PropType::Rows => fui::MAX_ROW_TEXT,
        PropType::Bool
        | PropType::Number { .. }
        | PropType::Int { .. }
        | PropType::Vec2 { .. }
        | PropType::Sides { .. }
        | PropType::Tracks { .. }
        | PropType::Color
        | PropType::Enum(_) => MAX_NAME,
    }
}

/// `value` as `def`'s prop, read without metamethods, its type and range checked.
fn to_prop(def: &PropDef, value: &Value) -> Result<PropValue, String> {
    let wrong = || format!("`{}` takes {}", def.name, describe(def.ty));
    let cap = text_cap(def.ty);
    let converted = match def.ty {
        PropType::Bool => match value {
            Value::Boolean(b) => PropValue::Bool(*b),
            _ => return Err(wrong()),
        },
        PropType::Number { .. } => PropValue::Number(number(value).ok_or_else(wrong)?),
        PropType::Int { .. } => {
            let n = number(value).ok_or_else(wrong)?;
            if n.fract() != 0.0 || !(0.0..=f64::from(u32::MAX)).contains(&n) {
                return Err(format!("`{}` is out of its range", def.name));
            }
            PropValue::Int(n as u32)
        }
        PropType::Vec2 { .. } => {
            let (x, y) = vec2(value).ok_or_else(wrong)?;
            PropValue::Vec2(x, y)
        }
        PropType::Color => PropValue::Color(color(value).ok_or_else(wrong)?),
        PropType::Text { .. } | PropType::Rich { .. } | PropType::Image => {
            PropValue::Text(text(value, cap).ok_or_else(wrong)?)
        }
        PropType::Enum(names) => {
            let name = text(value, cap).ok_or_else(wrong)?;
            let at = names.iter().position(|n| *n == name).ok_or_else(wrong)?;
            PropValue::Enum(at as u8)
        }
        PropType::Rows => PropValue::Rows(rows(value, cap).ok_or_else(wrong)?),
        PropType::Sides { .. } => PropValue::Sides(sides(value).ok_or_else(wrong)?),
        PropType::Tracks { max } => PropValue::Tracks(tracks(value, max).ok_or_else(wrong)?),
    };
    fui::check_value(def, &converted).map_err(|why| format!("`{}`: {why}", def.name))?;
    Ok(converted)
}

/// A pair: `{x, y}` or `{x = .., y = ..}`.
fn vec2(value: &Value) -> Option<(f64, f64)> {
    let Value::Table(t) = value else {
        return None;
    };
    let get = |i: i32, name: &str| -> Option<f64> {
        let v = t.raw_get::<Value>(i).ok()?;
        let v = if v.is_nil() {
            t.raw_get::<Value>(name).ok()?
        } else {
            v
        };
        number(&v)
    };
    Some((get(1, "x")?, get(2, "y")?))
}

/// Four sides: one number for all, `{vertical, horizontal}` or `{top, right, bottom, left}`.
fn sides(value: &Value) -> Option<[f64; 4]> {
    if let Some(n) = number(value) {
        return Some([n; 4]);
    }
    let Value::Table(t) = value else {
        return None;
    };
    let get = |i: i32| number(&t.raw_get::<Value>(i).ok()?);
    match t.raw_len() {
        2 => {
            let (vertical, horizontal) = (get(1)?, get(2)?);
            Some([vertical, horizontal, vertical, horizontal])
        }
        4 => Some([get(1)?, get(2)?, get(3)?, get(4)?]),
        _ => None,
    }
}

/// Grid tracks: an array of at most `max` of a number (a length), `"auto"` or `"1fr"` (a share).
fn tracks(value: &Value, max: usize) -> Option<Vec<Track>> {
    let Value::Table(t) = value else {
        return None;
    };
    let len = t.raw_len();
    if len > max {
        return None;
    }
    (1..=len)
        .map(|i| match t.raw_get::<Value>(i).ok()? {
            Value::String(s) => {
                let s = short_text(&s, 32)?;
                if s == "auto" {
                    return Some(Track::Auto);
                }
                s.strip_suffix("fr")?.parse().ok().map(Track::Fr)
            }
            other => number(&other).map(Track::Px),
        })
        .collect()
}

/// A list's rows: an array of strings, each at most `cap` bytes.
fn rows(value: &Value, cap: usize) -> Option<Vec<String>> {
    let Value::Table(t) = value else {
        return None;
    };
    let len = t.raw_len();
    if len > fui::MAX_ROWS {
        return None;
    }
    (1..=len)
        .map(|i| text(&t.raw_get::<Value>(i).ok()?, cap))
        .collect()
}

/// A color: `{r, g, b, a?}` of 0-1, or `"#rrggbb"` / `"#rrggbbaa"`.
fn color(value: &Value) -> Option<[u8; 4]> {
    match value {
        Value::Table(t) => {
            let mut out = [255u8; 4];
            for (i, slot) in out.iter_mut().enumerate() {
                let v = t.raw_get::<Value>(i + 1).ok()?;
                if v.is_nil() && i == 3 {
                    break;
                }
                let n = number(&v)?;
                if !(0.0..=1.0).contains(&n) {
                    return None;
                }
                *slot = (n * 255.0).round() as u8;
            }
            Some(out)
        }
        Value::String(s) => {
            let s = short_text(s, 9)?;
            let hex = s.strip_prefix('#')?;
            if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
            let alpha = if hex.len() == 8 { byte(6)? } else { 255 };
            Some([byte(0)?, byte(2)?, byte(4)?, alpha])
        }
        _ => None,
    }
}

fn describe(ty: PropType) -> &'static str {
    match ty {
        PropType::Bool => "true or false",
        PropType::Number { .. } | PropType::Int { .. } => "a number",
        PropType::Vec2 { .. } => "{x, y}",
        PropType::Color => "a color ({r, g, b, a} of 0-1, or \"#rrggbb\")",
        PropType::Text { .. } | PropType::Rich { .. } => "text",
        PropType::Image => "a content path (mods/<mod>/<file>.png)",
        PropType::Enum(_) => "one of its names",
        PropType::Rows => "a list of strings",
        PropType::Sides { .. } => "a number, {vertical, horizontal} or {top, right, bottom, left}",
        PropType::Tracks { .. } => "a list of lengths, \"auto\" and shares like \"1fr\"",
    }
}

/// A props table as `kind`'s props.
fn read_props(kind: &KindDef, props: Option<Table>) -> mlua::Result<Props> {
    let mut out = Props::new();
    let Some(props) = props else {
        return Ok(out);
    };
    // Its slots are counted before it is walked: a table with few props and a huge capacity costs nothing unmetered.
    if table_slots(&props) > MAX_PROPS_SLOTS {
        return Err(Error::runtime(
            "ui: a props table holds a few props, not this many slots",
        ));
    }
    for pair in props.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let Some(name) = text(&key, MAX_NAME) else {
            return Err(Error::runtime("ui: a prop's name is a string"));
        };
        let def = kind
            .prop_named(&name)
            .ok_or_else(|| Error::runtime(format!("ui: a {} has no prop `{name}`", kind.name)))?;
        let value = to_prop(def, &value).map_err(|e| Error::runtime(format!("ui: {e}")))?;
        out.insert(def.prop, value);
    }
    Ok(out)
}

/// The rules that tie a control's props together and the realm's caps, for `props` set on a control of `kind` that
/// has `old` (none for a new one): the control's text byte counts afterwards (its own, and all).
fn check_control(
    ui: &UiState,
    kind: &KindDef,
    old: Option<&Node>,
    props: &Props,
) -> mlua::Result<(usize, usize)> {
    let get = |prop: Prop| -> PropValue {
        props
            .get(&prop)
            .cloned()
            .or_else(|| old.and_then(|n| n.props.get(&prop).cloned()))
            .unwrap_or_else(|| fui::prop_def(prop).default.value())
    };
    if kind.has(Prop::Selected)
        && let (PropValue::Int(selected), PropValue::Rows(rows)) =
            (get(Prop::Selected), get(Prop::Rows))
        && selected as usize > rows.len()
    {
        return Err(Error::runtime("ui: `selected` is past the list's rows"));
    }
    if kind.has(Prop::Range)
        && let (PropValue::Vec2(lo, hi), PropValue::Number(v)) =
            (get(Prop::Range), get(Prop::SliderValue))
    {
        if lo >= hi {
            return Err(Error::runtime(
                "ui: a slider's `range` goes from low to high",
            ));
        }
        if v < lo || v > hi {
            return Err(Error::runtime(
                "ui: a slider's `value` is outside its range",
            ));
        }
    }
    let (mut own_text, mut all_text) = (0, 0);
    for prop in kind.props() {
        let bytes = fui::text_bytes(&get(prop));
        all_text += bytes;
        // Rich text and a list's rows have caps of their own (their types').
        if !matches!(prop, Prop::RichText | Prop::Rows) {
            own_text += bytes;
        }
    }
    if own_text > fui::MAX_CONTROL_TEXT {
        return Err(Error::runtime("ui: a control holds at most 4 KiB of text"));
    }
    if ui.text - old.map_or(0, |n| n.text) + all_text > fui::MAX_TEXT {
        return Err(Error::runtime(
            "ui: the realm's controls hold at most 1 MiB of text",
        ));
    }
    if let Some(PropValue::Text(path)) = props.get(&Prop::Image)
        && !path.is_empty()
        && !ui.images.contains_key(path)
        && ui.images.len() >= fui::MAX_IMAGES
    {
        return Err(Error::runtime(
            "ui: the realm shows at most 256 distinct images",
        ));
    }
    Ok((own_text, all_text))
}

/// Writes a props field.
fn write_props(w: &mut Writer, props: &Props) {
    fui::write_props(w, props.iter().map(|(&p, v)| (p, v)));
}

/// The mod's root, made (and its op written) at its first use.
fn root_of(st: &mut State, mod_id: &str) -> mlua::Result<u32> {
    if let Some(&id) = st.ui.roots.get(mod_id) {
        return Ok(id);
    }
    let id = st.ui.next_handle()?;
    st.emit(Tier::Ui, op::ui::ROOT, |w| {
        w.u32(id);
        w.text(mod_id);
    })?;
    st.ui.next = id;
    st.ui.roots.insert(mod_id.to_string(), id);
    st.ui.nodes.insert(id, Node::new(None, 0, 0, 0));
    Ok(id)
}

/// Whether the tier was revoked (its functions then answer `false`).
fn revoked(lua: &Lua) -> mlua::Result<bool> {
    Ok(!granted(lua, Tier::Ui)?)
}

/// A new handle for `id`, as a Lua value.
fn handle(lua: &Lua, id: u32) -> mlua::Result<Value> {
    let serial = state(lua)?.serial();
    lua.create_userdata(Control { id, serial })
        .map(Value::UserData)
}

/// The `ui` table of `mod_id`.
pub(crate) fn table(lua: &Lua, mod_id: &str) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    let owner = mod_id.to_string();
    t.raw_set(
        "root",
        function(lua, move |lua, ()| {
            if revoked(lua)? {
                return Ok(Value::Boolean(false));
            }
            let id = root_of(&mut *state(lua)?, &owner)?;
            handle(lua, id)
        })?,
    )?;
    let owner = mod_id.to_string();
    t.raw_set(
        "new",
        function(
            lua,
            move |lua, args: (LuaString, Option<Table>, Option<Value>)| {
                new_control(lua, &owner, args)
            },
        )?,
    )?;
    t.raw_set(
        "interactive",
        function(lua, |lua, on: bool| {
            if revoked(lua)? {
                return Ok(false);
            }
            state(lua)?.emit(Tier::Ui, op::ui::INTERACTIVE, |w| w.bool(on))?;
            Ok(true)
        })?,
    )?;
    t.raw_set("focus", function(lua, focus)?)?;
    t.raw_set(
        "screen",
        function(lua, |lua, ()| Ok(state(lua)?.ui.screen))?,
    )?;
    t.raw_set(
        "on",
        function(lua, |lua, (name, f): (LuaString, Function)| {
            if revoked(lua)? {
                return Ok(false);
            }
            match name.as_bytes().as_ref() {
                b"screen" => add_handler(lua, f, |st| &mut st.ui.screen_handlers)?,
                b"closed" => add_handler(lua, f, |st| &mut st.ui.closed_handlers)?,
                _ => {
                    return Err(Error::runtime(
                        "ui.on: the events are \"screen\" and \"closed\" (a control's own are h:on)",
                    ));
                }
            }
            Ok(true)
        })?,
    )?;
    t.set_readonly(true);
    Ok(t)
}

/// `ui.new(kind, props?, parent?)`: a control under `parent` (the mod's root when none).
fn new_control(
    lua: &Lua,
    owner: &str,
    (kind, props, parent): (LuaString, Option<Table>, Option<Value>),
) -> mlua::Result<Value> {
    if revoked(lua)? {
        return Ok(Value::Boolean(false));
    }
    let name = name_of(&kind);
    let kind = fui::kind_named(&name)
        .ok_or_else(|| Error::runtime(format!("ui.new: there is no kind `{name}`")))?;
    let props = read_props(kind, props)?;
    let parent = match parent {
        None | Some(Value::Nil) => None,
        Some(Value::UserData(u)) => Some(u),
        Some(_) => return Err(Error::runtime("ui.new: the parent is a control")),
    };
    let id = {
        let mut st = state(lua)?;
        let parent = match parent {
            Some(u) => {
                let id = handle_of(&st, &u, "ui.new")?;
                st.ui.live(id)?;
                id
            }
            None => root_of(&mut st, owner)?,
        };
        create(&mut st, kind, parent, props)?
    };
    handle(lua, id)
}

/// `ui.focus(field)`: only within [`fui::ACTIVATION_US`] of an activating event; `false` otherwise.
fn focus(lua: &Lua, field: Value) -> mlua::Result<bool> {
    if revoked(lua)? {
        return Ok(false);
    }
    let Value::UserData(u) = field else {
        return Err(Error::runtime("ui.focus takes a textfield"));
    };
    let mut st = state(lua)?;
    let id = handle_of(&st, &u, "ui.focus")?;
    if st.ui.live(id)?.kind.map(|k| k.kind) != Some(Kind::TextField) {
        return Err(Error::runtime("ui.focus takes a textfield"));
    }
    let now = st.now;
    let active = st
        .ui
        .activation
        .is_some_and(|at| now.saturating_sub(at) <= fui::ACTIVATION_US);
    if !active {
        return Ok(false);
    }
    st.emit(Tier::Ui, op::ui::FOCUS, |w| w.u32(id))?;
    Ok(true)
}

/// Makes a control of `kind` under `parent` with `props`: its op written, its node in the mirror.
fn create(st: &mut State, kind: &'static KindDef, parent: u32, props: Props) -> mlua::Result<u32> {
    if st.ui.controls >= fui::MAX_CONTROLS {
        return Err(Error::runtime("ui: at most 1024 controls are live at once"));
    }
    let parent_node = st.ui.live(parent)?;
    let depth = parent_node.depth + 1;
    if depth > fui::MAX_DEPTH {
        return Err(Error::runtime("ui: controls nest at most 32 deep"));
    }
    if parent_node.is_container() && parent_node.children.len() >= fui::MAX_CONTAINER_CHILDREN {
        return Err(Error::runtime("ui: a container holds at most 256 children"));
    }
    let containers = parent_node.containers + usize::from(kind.kind.container().is_some());
    if containers > fui::MAX_CONTAINER_DEPTH {
        return Err(Error::runtime("ui: containers nest at most 8 deep"));
    }
    let counts = check_control(&st.ui, kind, None, &props)?;
    let id = st.ui.next_handle()?;
    st.emit(Tier::Ui, op::ui::CREATE, |w| {
        w.u32(id);
        w.u32(parent);
        w.u8(kind.kind as u8);
        write_props(w, &props);
    })?;
    let ui = &mut st.ui;
    ui.next = id;
    ui.controls += 1;
    if let Some(p) = ui.nodes.get_mut(&parent) {
        p.children.push(id);
    }
    ui.nodes
        .insert(id, Node::new(Some(kind), parent, depth, containers));
    ui.apply(id, props, counts);
    Ok(id)
}

/// A method's control, live and not a root, with its kind: `None` when the tier was revoked.
fn target(lua: &Lua, this: &Control, what: &str) -> mlua::Result<Option<(u32, &'static KindDef)>> {
    let st = state(lua)?;
    if !st.granted(Tier::Ui) {
        return Ok(None);
    }
    if !st.is_own(this.serial) {
        return Err(Error::runtime(format!(
            "{what}: a control of another realm"
        )));
    }
    Ok(Some((this.id, st.ui.control(this.id, what)?)))
}

/// `h:set(props)`.
fn set(lua: &Lua, this: &Control, props: Table) -> mlua::Result<bool> {
    let Some((id, kind)) = target(lua, this, "set")? else {
        return Ok(false);
    };
    let props = read_props(kind, Some(props))?;
    let mut st = state(lua)?;
    let counts = check_control(&st.ui, kind, Some(st.ui.live(id)?), &props)?;
    st.emit(Tier::Ui, op::ui::SET, |w| {
        w.u32(id);
        write_props(w, &props);
    })?;
    st.ui.apply(id, props, counts);
    Ok(true)
}

/// `h:get(name)`: from the mirror.
fn get(lua: &Lua, this: &Control, name: LuaString) -> mlua::Result<MultiValue> {
    let Some((id, kind)) = target(lua, this, "get")? else {
        return Ok(MultiValue::from_vec(vec![Value::Boolean(false)]));
    };
    let name = name_of(&name);
    let def = kind
        .prop_named(&name)
        .ok_or_else(|| Error::runtime(format!("get: a {} has no prop `{name}`", kind.name)))?;
    let value = state(lua)?.ui.live(id)?.prop(def.prop);
    prop_to_lua(lua, def.ty, value)
}

/// `h:destroy()`: the control and everything under it.
fn destroy(lua: &Lua, this: &Control) -> mlua::Result<bool> {
    let Some((id, _)) = target(lua, this, "destroy")? else {
        return Ok(false);
    };
    let released = {
        let mut st = state(lua)?;
        st.emit(Tier::Ui, op::ui::DESTROY, |w| w.u32(id))?;
        let ui = &mut st.ui;
        let parent = ui.live(id)?.parent;
        if let Some(p) = ui.nodes.get_mut(&parent) {
            p.children.retain(|&c| c != id);
        }
        let mut released = Vec::new();
        for gone in ui.subtree(id) {
            let Some(node) = ui.nodes.remove(&gone) else {
                continue;
            };
            ui.controls -= 1;
            ui.text -= node.text;
            if let Some(image) = &node.image {
                ui.drop_image(image);
            }
            released.extend(node.handlers.into_values());
        }
        released
    };
    for id in released {
        release(lua, id);
    }
    Ok(true)
}

/// `h:parent(p)`: never under itself, within the depth cap.
fn reparent(lua: &Lua, this: &Control, parent: AnyUserData) -> mlua::Result<bool> {
    let Some((id, _)) = target(lua, this, "parent")? else {
        return Ok(false);
    };
    let mut st = state(lua)?;
    let to = handle_of(&st, &parent, "parent")?;
    let (to_depth, to_containers) = {
        let target = st.ui.live(to)?;
        (target.depth, target.containers)
    };
    let all = st.ui.subtree(id);
    if all.contains(&to) {
        return Err(Error::runtime("parent: a control never goes under itself"));
    }
    let here = st.ui.live(id)?.depth;
    let deepest = all
        .iter()
        .filter_map(|n| st.ui.nodes.get(n).map(|n| n.depth))
        .max()
        .unwrap_or(here);
    let shift = to_depth + 1;
    if deepest - here + shift > fui::MAX_DEPTH {
        return Err(Error::runtime("parent: controls nest at most 32 deep"));
    }
    let old = st.ui.live(id)?.parent;
    let old_containers = st.ui.live(old)?.containers;
    let nested = all
        .iter()
        .filter_map(|n| st.ui.nodes.get(n))
        .map(|n| n.containers - old_containers)
        .max()
        .unwrap_or(0);
    if to_containers + nested > fui::MAX_CONTAINER_DEPTH {
        return Err(Error::runtime("parent: containers nest at most 8 deep"));
    }
    let target = st.ui.live(to)?;
    if target.is_container() && to != old && target.children.len() >= fui::MAX_CONTAINER_CHILDREN {
        return Err(Error::runtime(
            "parent: a container holds at most 256 children",
        ));
    }
    st.emit(Tier::Ui, op::ui::PARENT, |w| {
        w.u32(id);
        w.u32(to);
    })?;
    let ui = &mut st.ui;
    let old = ui.live(id)?.parent;
    if let Some(p) = ui.nodes.get_mut(&old) {
        p.children.retain(|&c| c != id);
    }
    if let Some(p) = ui.nodes.get_mut(&to) {
        p.children.push(id);
    }
    for n in all {
        if let Some(node) = ui.nodes.get_mut(&n) {
            node.depth = node.depth - here + shift;
            node.containers = node.containers - old_containers + to_containers;
        }
    }
    if let Some(node) = ui.nodes.get_mut(&id) {
        node.parent = to;
    }
    Ok(true)
}

/// Where `h:front()` and `h:back()` put a control among its siblings.
#[derive(Clone, Copy)]
enum Order {
    Front,
    Back,
}

/// `h:front()` / `h:back()`.
fn order(lua: &Lua, this: &Control, order: Order) -> mlua::Result<bool> {
    let what = match order {
        Order::Front => "front",
        Order::Back => "back",
    };
    let Some((id, _)) = target(lua, this, what)? else {
        return Ok(false);
    };
    let mut st = state(lua)?;
    st.emit(Tier::Ui, op::ui::ORDER, |w| {
        w.u32(id);
        w.bool(matches!(order, Order::Front));
    })?;
    let parent = st.ui.live(id)?.parent;
    if let Some(p) = st.ui.nodes.get_mut(&parent) {
        p.children.retain(|&c| c != id);
        match order {
            Order::Front => p.children.push(id),
            Order::Back => p.children.insert(0, id),
        }
    }
    Ok(true)
}

/// `h:on(event, fn)`: one function per event, the last one given.
fn on(lua: &Lua, this: &Control, (name, f): (LuaString, Function)) -> mlua::Result<bool> {
    let Some((id, kind)) = target(lua, this, "on")? else {
        return Ok(false);
    };
    let name = name_of(&name);
    let event = *kind
        .events
        .iter()
        .find(|e| e.name() == name)
        .ok_or_else(|| Error::runtime(format!("on: a {} has no event `{name}`", kind.name)))?;
    let callback = keep(lua, f)?;
    let old = state(lua)?
        .ui
        .nodes
        .get_mut(&id)
        .and_then(|n| n.handlers.insert(event, callback));
    if let Some(old) = old {
        release(lua, old);
    }
    Ok(true)
}

/// A control's method: its `self` and arguments converted inside it, a refusal charged ([`guarded`]).
fn method<A: FromLuaMulti, R>(
    lua: &Lua,
    args: MultiValue,
    f: impl FnOnce(&Lua, &Control, A) -> mlua::Result<R>,
) -> mlua::Result<R> {
    guarded(lua, args, |lua, (this, rest): (AnyUserData, A)| {
        let this = this
            .borrow::<Control>()
            .map_err(|_| Error::runtime("a control's method is called on a control"))?;
        f(lua, &this, rest)
    })
}

impl UserData for Control {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, this, ()| {
            Ok(format!("control {}", this.id))
        });
        methods.add_meta_method(mlua::MetaMethod::Eq, |_, this, other: AnyUserData| {
            Ok(other
                .borrow::<Control>()
                .is_ok_and(|o| o.id == this.id && o.serial == this.serial))
        });
        methods.add_function("set", |lua, args| method(lua, args, set));
        methods.add_function("get", |lua, args| method(lua, args, get));
        methods.add_function("destroy", |lua, args| {
            method(lua, args, |lua, this, ()| destroy(lua, this))
        });
        methods.add_function("parent", |lua, args| method(lua, args, reparent));
        methods.add_function("front", |lua, args| {
            method(lua, args, |lua, this, ()| order(lua, this, Order::Front))
        });
        methods.add_function("back", |lua, args| {
            method(lua, args, |lua, this, ()| order(lua, this, Order::Back))
        });
        methods.add_function("on", |lua, args| method(lua, args, on));
    }
}

/// A prop's value as Lua sees it: a vec2 as two numbers, a color as `{r, g, b, a}` of 0-1, an enum as its name.
fn prop_to_lua(lua: &Lua, ty: PropType, value: PropValue) -> mlua::Result<MultiValue> {
    let v = match value {
        PropValue::Bool(b) => Value::Boolean(b),
        PropValue::Number(n) => Value::Number(n),
        PropValue::Int(n) => Value::Number(f64::from(n)),
        PropValue::Vec2(x, y) => {
            return Ok(MultiValue::from_vec(vec![
                Value::Number(x),
                Value::Number(y),
            ]));
        }
        PropValue::Color(c) => {
            let t = lua.create_table()?;
            for (i, b) in c.iter().enumerate() {
                t.raw_set(i + 1, f64::from(*b) / 255.0)?;
            }
            Value::Table(t)
        }
        PropValue::Text(s) => Value::String(lua.create_string(&s)?),
        PropValue::Enum(i) => {
            let names = match ty {
                PropType::Enum(names) => names,
                _ => &[],
            };
            Value::String(lua.create_string(names.get(usize::from(i)).copied().unwrap_or(""))?)
        }
        PropValue::Rows(rows) => {
            let t = lua.create_table()?;
            for (i, row) in rows.iter().enumerate() {
                t.raw_set(i + 1, row.as_str())?;
            }
            Value::Table(t)
        }
        PropValue::Sides(sides) => {
            let t = lua.create_table()?;
            for (i, side) in sides.iter().enumerate() {
                t.raw_set(i + 1, *side)?;
            }
            Value::Table(t)
        }
        PropValue::Tracks(tracks) => {
            let t = lua.create_table()?;
            for (i, track) in tracks.iter().enumerate() {
                match track {
                    Track::Auto => t.raw_set(i + 1, "auto")?,
                    Track::Px(v) => t.raw_set(i + 1, *v)?,
                    Track::Fr(v) => t.raw_set(i + 1, format!("{v}fr"))?,
                }
            }
            Value::Table(t)
        }
    };
    Ok(MultiValue::from_vec(vec![v]))
}

/// What a control's callback gets.
pub(crate) enum Args {
    None,
    Bool(bool),
    Value(FormatValue),
    Text(String),
    Row(u32),
    Scroll(f64, f64),
}

/// A `ui` event as decoded.
pub(crate) enum Arrived {
    Control {
        handle: u32,
        event: Event,
        args: Args,
    },
    Screen(f64, f64, f64),
    Closed,
}

/// A `ui` event the mirror took: its callbacks wait in the queue.
pub(crate) type UiEvent = Arrived;

/// Reads a `ui` event's body.
pub(crate) fn decode(
    kind: u8,
    r: &mut Reader<'_>,
    budget: &mut NodeBudget,
) -> Result<Arrived, Malformed> {
    match kind {
        event::ui::SCREEN => {
            let (w, h, s) = (r.finite()?, r.finite()?, r.finite()?);
            if w < 0.0 || h < 0.0 || s <= 0.0 {
                return Err(Malformed("a screen of negative size or scale"));
            }
            return Ok(Arrived::Screen(w, h, s));
        }
        event::ui::CLOSED => return Ok(Arrived::Closed),
        _ => {}
    }
    let event = Event::from_id(kind).ok_or(Malformed("an unknown ui event"))?;
    let handle = r.u32()?;
    let args = match event {
        Event::Click | Event::Press => Args::None,
        Event::Hover => Args::Bool(r.bool()?),
        Event::Change => Args::Value(super::read_value(r, budget)?),
        Event::Submit => Args::Text(r.text()?.to_string()),
        Event::Select => Args::Row(r.u32()?),
        Event::Scroll => Args::Scroll(r.finite()?, r.finite()?),
    };
    Ok(Arrived::Control {
        handle,
        event,
        args,
    })
}

/// An event arrives: the mirror takes what it changes, and its callbacks are queued; `None` when it is dropped (an
/// ungranted or revoked tier, an unknown or destroyed control, an event or value its control does not have).
pub(crate) fn arrive(lua: &Lua, arrived: Arrived) -> Option<UiEvent> {
    let mut st = state(lua).ok()?;
    if !st.granted(Tier::Ui) {
        return None;
    }
    let now = st.now;
    let ui = &mut st.ui;
    match arrived {
        Arrived::Screen(w, h, s) => ui.screen = (w, h, s),
        Arrived::Closed => {}
        Arrived::Control {
            handle,
            event,
            args,
        } => {
            let kind = ui.nodes.get(&handle)?.kind?;
            if !kind.events.contains(&event) {
                return None;
            }
            let args = take_args(ui, handle, kind, args)?;
            if event.activates() {
                ui.activation = Some(now);
            }
            return Some(Arrived::Control {
                handle,
                event,
                args,
            });
        }
    }
    Some(arrived)
}

/// Checks an event's value against its control and puts what it changes into the mirror: the callback's arguments,
/// or `None` to drop the event.
fn take_args(ui: &mut UiState, handle: u32, kind: &'static KindDef, args: Args) -> Option<Args> {
    let node = ui.nodes.get(&handle)?;
    let (prop, value, args) = match (kind.kind, args) {
        (_, args @ (Args::None | Args::Bool(_) | Args::Scroll(..))) => return Some(args),
        (Kind::TextField, Args::Text(text)) => {
            fui::check_text(&text, fui::MAX_CONTROL_TEXT).ok()?;
            return Some(Args::Text(text));
        }
        (Kind::Toggle, Args::Value(FormatValue::Bool(on))) => {
            (Prop::On, PropValue::Bool(on), Args::Bool(on))
        }
        (Kind::Slider, Args::Value(FormatValue::Number(v))) => {
            let PropValue::Vec2(lo, hi) = node.prop(Prop::Range) else {
                return None;
            };
            if !(v.is_finite() && v >= lo && v <= hi) {
                return None;
            }
            (
                Prop::SliderValue,
                PropValue::Number(v),
                Args::Value(FormatValue::Number(v)),
            )
        }
        (Kind::TextField, Args::Value(FormatValue::String(bytes))) => {
            let text = String::from_utf8(bytes).ok()?;
            let PropValue::Int(max) = node.prop(Prop::MaxLength) else {
                return None;
            };
            if text.chars().count() > max as usize {
                return None;
            }
            (Prop::Text, PropValue::Text(text.clone()), Args::Text(text))
        }
        (Kind::List, Args::Row(row)) => {
            let PropValue::Rows(rows) = node.prop(Prop::Rows) else {
                return None;
            };
            if row as usize > rows.len() {
                return None;
            }
            (Prop::Selected, PropValue::Int(row), Args::Row(row))
        }
        _ => return None,
    };
    fui::check_value(fui::prop_def(prop), &value).ok()?;
    let props = Props::from([(prop, value)]);
    let counts = check_control(ui, kind, Some(node), &props).ok()?;
    ui.apply(handle, props, counts);
    Some(args)
}

/// Runs a queued `ui` event's callbacks.
pub(crate) fn dispatch(lua: &Lua, e: UiEvent) {
    let entry = op::realm::ENTRY_EVENT;
    match e {
        Arrived::Screen(w, h, s) => {
            let handlers = state(lua)
                .map(|st| st.ui.screen_handlers.clone())
                .unwrap_or_default();
            for id in handlers {
                invoke_kept(lua, entry, id, (w, h, s));
            }
        }
        Arrived::Closed => {
            let handlers = state(lua)
                .map(|st| st.ui.closed_handlers.clone())
                .unwrap_or_default();
            for id in handlers {
                invoke_kept(lua, entry, id, ());
            }
        }
        Arrived::Control {
            handle,
            event,
            args,
        } => {
            let Some(id) = state(lua)
                .ok()
                .and_then(|st| st.ui.nodes.get(&handle)?.handlers.get(&event).copied())
            else {
                return;
            };
            match args {
                Args::None => invoke_kept(lua, entry, id, ()),
                Args::Bool(b) => invoke_kept(lua, entry, id, b),
                Args::Text(t) => {
                    if let Ok(s) = lua.create_string(&t) {
                        invoke_kept(lua, entry, id, s);
                    }
                }
                Args::Row(0) => invoke_kept(lua, entry, id, Value::Nil),
                Args::Row(row) => invoke_kept(lua, entry, id, f64::from(row)),
                Args::Scroll(x, y) => invoke_kept(lua, entry, id, (x, y)),
                Args::Value(v) => {
                    if let Ok(v) = super::net::value_to_lua(lua, &v) {
                        invoke_kept(lua, entry, id, v);
                    }
                }
            }
        }
    }
}
