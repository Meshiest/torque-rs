//! Writing typed static fields: the interpreter's field writes and the class table's defaults go
//! through `Vm::write_field`, which parses the text with the field's type (`fields::set`) against the runtime
//! around the object (the string table, datablock lookups, `./` names, the console), stores it, then runs the
//! field's range validator.
//!
//! Follows Torque3D's MIT-licensed `console/consoleObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (c) 2015
//! Faust Logic, Inc.) and `console/typeValidators.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this
//! crate's own.

use std::rc::Rc;

use super::objops::NewObject;
use super::{Level, Vm};
use crate::classes::Validator;
use crate::fields::{self, FieldEnv, Value};
use crate::sim::{ClassId, ObjId, SimObject};
use crate::text::{fold, format_g, to_f64};

/// The most bytes of text the string table keeps (keys and values); past it a text is not remembered (its
/// own spelling is used), like `files::MAX_FILE_NAME_BYTES`.
pub const MAX_STRING_TABLE_BYTES: usize = 16 << 20;

/// The host's observer of static-field writes (see `Vm::set_static_write_observer`).
#[derive(Default, Clone)]
pub(crate) struct StaticObserver(pub(crate) Option<super::StaticWriteFn>);

impl std::fmt::Debug for StaticObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "StaticObserver(set)"
        } else {
            "StaticObserver(none)"
        })
    }
}

/// `StringTable`: the first spelling of every text a `string` or `filename` field (or a class name) stored,
/// ignoring case.
#[derive(Debug)]
pub struct StringTable {
    map: crate::namehash::NameMap<Vec<u8>, Rc<[u8]>>,
    bytes: usize,
    cap: usize,
}

impl Default for StringTable {
    fn default() -> Self {
        Self {
            map: Default::default(),
            bytes: 0,
            cap: MAX_STRING_TABLE_BYTES,
        }
    }
}

impl StringTable {
    /// The first spelling of `text` the table holds (ignoring case), remembering `text` as that spelling when
    /// it is the first and the table has room.
    pub fn intern(&mut self, text: &[u8]) -> Rc<[u8]> {
        let key = fold(text);
        if let Some(found) = self.map.get(&key) {
            return Rc::clone(found);
        }
        let value: Rc<[u8]> = Rc::from(text);
        let cost = text.len().saturating_mul(2);
        if self.bytes.saturating_add(cost) <= self.cap {
            self.bytes += cost;
            self.map.insert(key, Rc::clone(&value));
        }
        value
    }

    #[cfg(test)]
    fn with_cap(cap: usize) -> Self {
        Self {
            cap,
            ..Self::default()
        }
    }
}

pub(super) struct VmEnv<'a>(pub(super) &'a mut Vm);

impl FieldEnv for VmEnv<'_> {
    fn rules(&self) -> crate::fields::FieldRules {
        self.0.objects.classes.field_rules
    }
    fn intern(&mut self, text: &[u8]) -> Rc<[u8]> {
        self.0.strings.intern(text)
    }

    fn expand_filename(&mut self, text: &[u8]) -> Option<Vec<u8>> {
        match self.0.expand_script_filename(text) {
            Ok(path) => Some(path),
            Err(message) => {
                self.0.error(message);
                None
            }
        }
    }

    fn find_object(&mut self, text: &[u8], class: &str) -> Option<ObjId> {
        let objects = &self.0.objects;
        let id = objects.find(text)?;
        let wanted = objects.classes.find(class.as_bytes())?;
        objects
            .classes
            .is_a(objects.get(id)?.class(), wanted)
            .then_some(id)
    }

    fn message(&mut self, level: Level, text: String) {
        self.0.log_bytes(level, text.as_bytes());
    }

    fn charge(&mut self, units: u64) -> bool {
        self.0.charge_work(units)
    }
}

/// How much of an object's name a warning quotes: the log keeps `MAX_LOG_LINE` bytes of a line, and a byte of the
/// name never makes less than a byte of text, so no more than that of a name can show. A multi-byte character the
/// cut splits becomes a replacement character at the end of the copy, which is past what the log keeps (the
/// line starts with the class name, so the name starts at byte 4 or later and its copy ends at least 4 bytes
/// beyond the 4095th byte of the line).
const SHOWN_NAME_BYTES: usize = super::MAX_LOG_LINE;

/// An object's name as a warning quotes it.
fn shown_name(name: &[u8]) -> String {
    super::lossy(name.get(..SHOWN_NAME_BYTES).unwrap_or(name))
}

impl Vm {
    /// The object a write goes to: a registered one, or (None) the one this frame is building. The interpreter
    /// never asks for a frame's object when it is a registered one (`write_cur_field` turns a re-declared
    /// datablock into `Some(id)`), but the type allows it: that case answers with the registered object, as
    /// `Some(id)` would (a unit test pins it), and never panics.
    fn target(&mut self, id: Option<ObjId>) -> Option<&mut SimObject> {
        match id {
            Some(id) => self.objects.get_mut(id),
            None => match self.creating.last_mut()?.object.as_mut()? {
                NewObject::Pending(object) => Some(object),
                NewObject::Live(id) => {
                    let id = *id;
                    self.objects.get_mut(id)
                }
            },
        }
    }

    /// `setDataField` (`id` None: the object this frame is building). A static field's text is parsed with
    /// its type into the element the index names (nothing when the index is outside the elements), then the
    /// field's validator runs on element 0 (also after a skipped write); any other name is a
    /// dynamic field.
    #[inline(never)]
    pub(crate) fn write_field(
        &mut self,
        id: Option<ObjId>,
        name: &[u8],
        array: &[u8],
        value: &[u8],
    ) {
        self.write_field_as(id, name, array, value, true, true);
    }

    /// [`write_field`](Self::write_field) for a class's own assignments, which store a member directly and never reach
    /// `onStaticModified` (`GameBaseData::onAdd` giving `className` its class name): the observer is not told.
    #[inline(never)]
    pub(crate) fn write_field_quiet(
        &mut self,
        id: Option<ObjId>,
        name: &[u8],
        array: &[u8],
        value: &[u8],
    ) {
        self.write_field_as(id, name, array, value, false, true);
    }

    /// A class's own write of a member (a client datablock's `unpackData`): parsed as
    /// [`set_object_field`](Self::set_object_field) parses it, but no range validator runs (the class writes the
    /// member, not the field: `invalid value for ...` is a script's).
    pub fn set_object_member(&mut self, id: ObjId, name: &[u8], array: &[u8], value: &[u8]) {
        if self.objects.contains(id) {
            self.write_field_as(Some(id), name, array, value, false, false);
        }
    }

    /// A host's engine-side assignment of a field of a registered object (a GUI control's bounds after a resize, the
    /// profile `onWake` finds for it): parsed as the interpreter parses it (texts interned, a profile or datablock
    /// found by name, the validator run), the static-write observer not told, as `write_field_quiet`. Nothing happens
    /// for an object that does not exist.
    pub fn set_object_field(&mut self, id: ObjId, name: &[u8], array: &[u8], value: &[u8]) {
        if self.objects.contains(id) {
            self.write_field_quiet(Some(id), name, array, value);
        }
    }

    fn write_field_as(
        &mut self,
        id: Option<ObjId>,
        name: &[u8],
        array: &[u8],
        value: &[u8],
        observed: bool,
        validated: bool,
    ) {
        let Some(class) = self.target(id).map(|o| o.class()) else {
            return;
        };
        let Some(field) = self.objects.classes.static_field(class, name) else {
            self.write_dynamic(id, name, array, value);
            return;
        };
        let (spec, first) = (field.0.clone(), field.1.first().copied());
        if let Some(slot) = SimObject::write_slot(field, array) {
            let mut current = self
                .target(id)
                .and_then(|o| o.slot(slot).cloned())
                .unwrap_or_else(|| fields::zero(spec.ty));
            fields::set(spec.ty, &mut current, value, &mut VmEnv(self));
            if let Some(stored) = self.target(id).and_then(|o| o.slot_mut(slot)) {
                *stored = current;
            }
        }
        if let (true, Some(validator), Some(first)) = (validated, spec.validator, first) {
            self.validate(id, &spec.name, first, validator);
        }
        if observed && spec.ty != fields::FieldType::Deprecated {
            self.notify_static_write(id, class);
        }
    }

    /// The host's observer of static writes (`set_static_write_observer`), for the object a write went to (None while
    /// it is still being built).
    fn notify_static_write(&mut self, id: Option<ObjId>, class: ClassId) {
        let Some(observer) = self.quirks.static_observer.0.clone() else {
            return;
        };
        let registered = match id {
            Some(id) => Some(id),
            None => match self.creating.last().and_then(|c| c.object.as_ref()) {
                Some(NewObject::Live(id)) => Some(*id),
                _ => None,
            },
        };
        observer(self, registered, class);
    }

    /// Makes the class table's defaults (`name` or `name[i]`, text) what a fresh object of `class` reads:
    /// each text is parsed by its field's type as a write would be (no validator), into the class's initial
    /// slots. A host call (`classes::install`), not charged: the table is bounded.
    ///
    /// The class's slots start from the type's zero: a `dump()` lists every non-empty value, so what it
    /// leaves out reads empty even where the parent's defaults (which the layout's initial values were
    /// cloned from) say otherwise; a class table where a class leaves out a value its parent lists reads empty there.
    ///
    /// With `inherit`, the slots start from the class's layout (the parent's defaults) instead of the type's zero, so
    /// the list only needs the values the class changes.
    pub(crate) fn apply_defaults(
        &mut self,
        class: ClassId,
        defaults: &[(&str, &str)],
        inherit: bool,
    ) {
        let Some(layout) = self
            .objects
            .classes
            .get(class)
            .map(|c| Rc::clone(&c.layout))
        else {
            return;
        };
        let mut values = layout.initial.clone();
        if !inherit {
            for (field, slots) in &layout.fields {
                for slot in slots {
                    if let Some(value) = values.get_mut(*slot) {
                        *value = fields::zero(field.ty);
                    }
                }
            }
        }
        for (name, text) in defaults {
            let (base, index) = match name.split_once('[') {
                Some((base, rest)) => (base, rest.trim_end_matches(']')),
                None => (*name, ""),
            };
            let Some(field) = layout.field(base.as_bytes()) else {
                continue;
            };
            let Some(slot) = SimObject::write_slot(field, index.as_bytes()) else {
                continue;
            };
            if let Some(value) = values.get_mut(slot) {
                fields::set(field.0.ty, value, text.as_bytes(), &mut VmEnv(self));
            }
        }
        self.objects.classes.set_initial(class, values);
    }

    /// A dynamic field of the target (the empty string deletes it).
    fn write_dynamic(&mut self, id: Option<ObjId>, name: &[u8], array: &[u8], value: &[u8]) {
        let pending = match id {
            Some(id) => Err(id),
            None => match self.creating.last_mut().and_then(|c| c.object.as_mut()) {
                Some(NewObject::Pending(object)) => Ok(object),
                Some(NewObject::Live(id)) => Err(*id),
                None => return,
            },
        };
        match pending {
            Ok(object) => {
                object.set_dynamic_field(&self.objects.classes, name, array, value);
            }
            Err(id) => {
                self.objects.set_field(id, name, array, value);
            }
        }
    }

    /// A field's range validator (`FRangeValidator`, `IRangeValidator`): clamps, after printing
    /// `<Class> - <name or unnamed>(<id>) - invalid value for <field>: <why>`.
    fn validate(&mut self, id: Option<ObjId>, field: &[u8], slot: usize, validator: Validator) {
        let Some(object) = self.target(id) else {
            return;
        };
        let (object_id, class) = (object.id(), object.class());
        let Some(value) = object.slot_mut(slot) else {
            return;
        };
        let why = match (validator, value) {
            (Validator::FloatRange { min, max }, Value::Float(v)) => {
                if *v < min || *v > max {
                    *v = if *v < min { min } else { max };
                    Some(format!(
                        "Must be between {} and {}",
                        format_g(to_f64(min)),
                        format_g(to_f64(max))
                    ))
                } else {
                    None
                }
            }
            (Validator::ScaledIntRange { factor, min, max }, Value::Int(v)) => {
                *v = v.checked_div(factor).unwrap_or(*v);
                if *v < min || *v > max {
                    // Not `clamp`: a table with min above max must not panic.
                    *v = if *v < min { min } else { max };
                    Some(format!("Scaled value must be between {min} and {max}"))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(why) = why {
            // The name is only needed for the warning, and only as far as the log shows it; what is copied is
            // charged (the write itself costs nothing in range).
            let shown = self.target(id).and_then(|o| o.name().map(shown_name));
            let shown = shown.unwrap_or_else(|| "unnamed".to_string());
            self.objects.add_work(shown.len() as u64);
            let class = super::lossy(self.objects.classes.name(class));
            self.warn(format!(
                "{class} - {shown}({object_id}) - invalid value for {}: {why}",
                super::lossy(field)
            ));
        }
    }
}

#[cfg(test)]
#[path = "statics_tests.rs"]
mod tests;
