//! The object model: objects with ids, names and fields, the class registry, and the members of sets
//! and groups: TGE's SimObject, SimSet, SimGroup and the id and name tables.
//!
//! This module is plain data and never runs script code: the VM runs the script callbacks (`onAdd`,
//! `onRemove`) and the class hooks around these operations. Objects refer to each other by id only, so
//! there are no reference cycles, and an object is dropped as soon as it is unregistered.
//!
//! Loops whose length depends on what a script has built add their length to a work counter that the
//! VM charges to the step budget (`vm::WORK_PER_STEP`).
//!
//! Follows Torque3D's MIT-licensed `console/simObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (c) 2015
//! Faust Logic, Inc.), `console/simSet.cpp` (Copyright (c) 2012 GarageGames, LLC), `console/simManager.cpp` (Copyright
//! (c) 2012 GarageGames, LLC) and `console/simDictionary.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is
//! this crate's own.

use std::any::Any;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use crate::classes::Validator;
use crate::fields::{self, FieldType, Value};
use crate::memory::{FIELD_COST, MemoryAccount, OBJECT_COST};
use crate::text::{atoi, fold, until_nul, with_folded};
use crate::vars::refill;
use crate::vm::Vm;

/// An object's id as scripts see it (printed signed: RootGroup's is -1).
pub type ObjId = u32;
/// A class's index in the [`ClassRegistry`].
pub type ClassId = u32;

/// RootGroup's id.
pub const ROOT_GROUP_ID: ObjId = u32::MAX;
/// The first datablock id (`DataBlockObjectIdFirst`).
pub const FIRST_DATABLOCK_ID: ObjId = 3;
/// The longest dynamic field name kept, array suffix included (TGE's buffers hold 256 bytes).
pub const MAX_FIELD_NAME: usize = 255;

pub const SIM_OBJECT: ClassId = 0;
pub const SIM_SET: ClassId = 1;
pub const SIM_GROUP: ClassId = 2;
pub const SCRIPT_OBJECT: ClassId = 3;
pub const SCRIPT_GROUP: ClassId = 4;
pub const SIM_DATA_BLOCK: ClassId = 5;

/// How the object tables start: the sets and groups made after RootGroup and the dynamic id counter. The default
/// follows Torque3D's `Sim::init` (MIT-licensed `console/simManager.cpp`, Copyright (c) 2012 GarageGames, LLC) for the sets and
/// groups TGE 1.4 has too, and starts the dynamic ids right after the datablock range ([`Limits`](crate::policy::Limits)).
#[derive(Debug, Clone, Copy)]
pub struct SimConfig {
    /// The sets and groups made after RootGroup, in order (`DataBlockGroup` among them, by that name).
    pub start_objects: &'static [(&'static str, ClassId)],
    /// The dynamic id counter starts here and is incremented before use: the first object gets the next free id.
    pub last_dynamic_id: ObjId,
    /// Past this the counter wraps back to `last_dynamic_id`, skipping ids in use.
    pub max_dynamic_id: ObjId,
}

impl Default for SimConfig {
    fn default() -> SimConfig {
        SimConfig {
            start_objects: &START_OBJECTS,
            last_dynamic_id: crate::policy::Limits::default().datablock_id_limit(),
            max_dynamic_id: ROOT_GROUP_ID - 1,
        }
    }
}

/// The default start-up sets and groups.
const START_OBJECTS: [(&str, ClassId); 12] = [
    ("ActiveActionMapSet", SIM_SET),
    ("GhostAlwaysSet", SIM_SET),
    ("WayPointSet", SIM_SET),
    ("fxReplicatorSet", SIM_SET),
    ("fxFoliageSet", SIM_SET),
    ("ActionMapGroup", SIM_GROUP),
    ("ClientGroup", SIM_GROUP),
    ("GuiGroup", SIM_GROUP),
    ("GuiDataGroup", SIM_GROUP),
    ("TCPGroup", SIM_GROUP),
    ("ClientConnectionGroup", SIM_GROUP),
    ("DataBlockGroup", SIM_GROUP),
];

/// A Rust hook a class runs when one of its objects is registered (`on_add`: returning false fails the
/// registration, as TGE's `onAdd` does) or removed (`on_remove`). This is the seam a host's
/// classes added later (GUI, scene, network) plug into.
pub type ClassHook = Rc<dyn Fn(&mut Vm, ObjId) -> bool>;

/// One static field a class declares (see [`crate::fields`] for the types).
#[derive(Debug, Clone, PartialEq)]
pub struct StaticField {
    pub name: Vec<u8>,
    pub ty: FieldType,
    /// Elements (1 for a plain field).
    pub count: u16,
    /// The byte offset of element 0 in the C++ object: fields of one class chain at the same offset share storage.
    pub offset: Option<u32>,
    pub validator: Option<Validator>,
    /// What the field is for, shown by `dump` beside its range: set by Rust classes that document their fields
    /// (`#[derive(EngineClass)]`); the console's own classes have none.
    pub doc: Option<&'static str>,
}

impl StaticField {
    /// A plain text field (`caseString`, one element): the shape of the fields a host registers by name only.
    pub fn text(name: &str) -> StaticField {
        StaticField {
            name: name.as_bytes().to_vec(),
            ty: FieldType::CaseString,
            count: 1,
            offset: None,
            validator: None,
            doc: None,
        }
    }
}

/// Where the static fields of a class, its ancestors' included, keep their elements in an object.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    /// Each field with the slot of each of its elements, ancestors' fields first.
    pub fields: Vec<(StaticField, Vec<usize>)>,
    by_name: crate::namehash::NameMap<Vec<u8>, usize>,
    /// What a fresh object's slots hold (the type's zero, or the class's default).
    pub initial: Vec<Value>,
}

impl Layout {
    /// The layout of a class: its parent's, then its own fields (an element at an engine offset another
    /// field already holds shares that slot).
    pub fn extend(parent: Option<&Layout>, own: &[StaticField]) -> Layout {
        let mut layout = parent.cloned().unwrap_or_default();
        let mut at_offset: HashMap<u32, usize> = HashMap::new();
        for (field, slots) in &layout.fields {
            if let Some(offset) = field.offset {
                for (k, slot) in slots.iter().enumerate() {
                    at_offset.insert(
                        offset.saturating_add(element_size(field.ty) * k as u32),
                        *slot,
                    );
                }
            }
        }
        for field in own {
            if layout.by_name.contains_key(&fold(&field.name)) {
                continue;
            }
            let mut slots = Vec::with_capacity(usize::from(field.count));
            for k in 0..u32::from(field.count) {
                let key = field
                    .offset
                    .map(|o| o.saturating_add(element_size(field.ty) * k));
                let slot = match key.and_then(|key| at_offset.get(&key)) {
                    Some(slot) => *slot,
                    None => {
                        layout.initial.push(fields::zero(field.ty));
                        let slot = layout.initial.len() - 1;
                        if let Some(key) = key {
                            at_offset.insert(key, slot);
                        }
                        slot
                    }
                };
                slots.push(slot);
            }
            layout
                .by_name
                .insert(fold(&field.name), layout.fields.len());
            layout.fields.push((field.clone(), slots));
        }
        layout
    }

    /// The field called `name` (ignoring case) and its slots.
    pub fn field(&self, name: &[u8]) -> Option<&(StaticField, Vec<usize>)> {
        self.fields
            .get(*with_folded(name, |k| self.by_name.get(k))?)
    }
}

/// The bytes one element of a type takes in the C++ object (for the shared offsets).
fn element_size(ty: FieldType) -> u32 {
    match ty {
        FieldType::U8 | FieldType::Bool => 1,
        FieldType::U16 => 2,
        FieldType::S32Vector | FieldType::F32Vector | FieldType::Point3F => 12,
        FieldType::ColorF => 16,
        FieldType::Point2I | FieldType::Point2F => 8,
        FieldType::MatrixPosition | FieldType::MatrixRotation => 64,
        _ => 4,
    }
}

#[derive(Clone)]
pub struct ClassDef {
    /// The name `getClassName` prints.
    pub name: Vec<u8>,
    pub parent: Option<ClassId>,
    /// The static fields this class adds; inherited ones belong to its ancestors.
    pub fields: Vec<StaticField>,
    /// Every static field of the class and its ancestors, and where an object keeps them.
    pub layout: Rc<Layout>,
    /// Whether `new Class(name, args...)` accepts constructor arguments (SimGroup's do and ignore them;
    /// every other built-in class fails the declaration).
    pub takes_arguments: bool,
    /// False for a console class that is not a SimObject (the network events): `new` refuses it with
    /// `Unable to instantiate non-SimObject class X.`
    pub sim_object: bool,
    pub on_add: Option<ClassHook>,
    pub on_remove: Option<ClassHook>,
    /// The class's Rust behaviour (`Vm::set_class_behaviour`); see [`ClassRegistry::behaviours`].
    pub behaviour: Option<Rc<dyn crate::classes::EngineClass>>,
}

/// The class tree. The six built-in classes have fixed ids ([`SIM_OBJECT`] ...).
pub struct ClassRegistry {
    classes: Vec<ClassDef>,
    by_name: crate::namehash::NameMap<Vec<u8>, ClassId>,
    /// [`static_slot`](Self::static_slot)'s answers by class and the address of a `'static` name (a direct-mapped
    /// table; emptied whenever a class is added or handed out to change).
    slot_memo: Box<[std::cell::Cell<SlotMemo>; SLOT_MEMO_SIZE]>,
    /// How the console types of the classes' fields read back.
    pub field_rules: fields::FieldRules,
}

/// One answer of [`ClassRegistry::static_slot`]: the class, the name's address and length, the first element's slot
/// (`usize::MAX` for a field the class lacks).
#[derive(Clone, Copy)]
struct SlotMemo {
    class: ClassId,
    name: usize,
    len: usize,
    slot: usize,
}

const SLOT_MEMO_SIZE: usize = 1024;
const SLOT_MEMO_EMPTY: SlotMemo = SlotMemo {
    class: ClassId::MAX,
    name: 0,
    len: 0,
    slot: usize::MAX,
};

impl Default for ClassRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ClassRegistry {
    pub fn new() -> ClassRegistry {
        let mut registry = ClassRegistry {
            classes: Vec::new(),
            by_name: Default::default(),
            slot_memo: Box::new(std::array::from_fn(|_| {
                std::cell::Cell::new(SLOT_MEMO_EMPTY)
            })),
            field_rules: fields::FieldRules::default(),
        };
        // `class` and `superClass` are `string` fields (interned).
        let script = || {
            ["class", "superClass"].map(|n| StaticField {
                ty: FieldType::String,
                ..StaticField::text(n)
            })
        };
        registry.insert("SimObject", None, Vec::new(), false, true);
        registry.insert("SimSet", Some(SIM_OBJECT), Vec::new(), false, true);
        registry.insert("SimGroup", Some(SIM_SET), Vec::new(), true, true);
        registry.insert(
            "ScriptObject",
            Some(SIM_OBJECT),
            script().to_vec(),
            false,
            true,
        );
        registry.insert(
            "ScriptGroup",
            Some(SIM_GROUP),
            script().to_vec(),
            true,
            true,
        );
        registry.insert("SimDataBlock", Some(SIM_OBJECT), Vec::new(), false, true);
        registry
    }

    fn insert(
        &mut self,
        name: &str,
        parent: Option<ClassId>,
        fields: Vec<StaticField>,
        takes_arguments: bool,
        inherits_fields: bool,
    ) -> ClassId {
        self.forget_slots();
        let id = ClassId::try_from(self.classes.len()).unwrap_or(ClassId::MAX);
        let layout = Layout::extend(
            parent
                .filter(|_| inherits_fields)
                .and_then(|p| self.get(p))
                .map(|p| p.layout.as_ref()),
            &fields,
        );
        self.classes.push(ClassDef {
            name: name.as_bytes().to_vec(),
            parent,
            fields,
            layout: Rc::new(layout),
            takes_arguments,
            sim_object: true,
            on_add: None,
            on_remove: None,
            behaviour: None,
        });
        self.by_name.insert(fold(name.as_bytes()), id);
        id
    }

    /// Adds a class below `parent` with plain text fields (see [`StaticField::text`]); it inherits the
    /// parent's argument policy. None when the name is taken or the parent is unknown. Use
    /// `Vm::register_class` so the class's namespace is linked to its parent's as well.
    pub fn register(&mut self, name: &str, parent: &str, fields: &[&str]) -> Option<ClassId> {
        let fields = fields.iter().map(|f| StaticField::text(f)).collect();
        self.register_typed(name, parent, fields, true)
    }

    /// Adds a class with typed static fields: its parent's and `fields`, or `fields` only when
    /// `inherits_fields` is false (a class whose fields do not include its parent's).
    pub fn register_typed(
        &mut self,
        name: &str,
        parent: &str,
        fields: Vec<StaticField>,
        inherits_fields: bool,
    ) -> Option<ClassId> {
        if self.by_name.contains_key(&fold(name.as_bytes())) {
            return None;
        }
        let parent = self.find(parent.as_bytes())?;
        let takes_arguments = self.get(parent)?.takes_arguments;
        Some(self.insert(name, Some(parent), fields, takes_arguments, inherits_fields))
    }

    /// Replaces what a fresh object of `class` reads (its defaults; the class table's, applied by
    /// `classes::install`). The slots must be the class's layout's.
    pub fn set_initial(&mut self, class: ClassId, initial: Vec<Value>) {
        if let Some(def) = self.classes.get_mut(class as usize)
            && initial.len() == def.layout.initial.len()
        {
            Rc::make_mut(&mut def.layout).initial = initial;
        }
    }

    /// Case-insensitive (identifiers reach the registry in their first-interned spelling).
    pub fn find(&self, name: &[u8]) -> Option<ClassId> {
        // Folded on the stack (a host may ask for one class name for every object of a scope query).
        with_folded(until_nul(name), |k| self.by_name.get(k)).copied()
    }

    pub fn get(&self, class: ClassId) -> Option<&ClassDef> {
        self.classes.get(class as usize)
    }

    pub fn get_mut(&mut self, class: ClassId) -> Option<&mut ClassDef> {
        self.forget_slots();
        self.classes.get_mut(class as usize)
    }

    fn forget_slots(&mut self) {
        for memo in self.slot_memo.iter() {
            memo.set(SLOT_MEMO_EMPTY);
        }
    }

    /// The first element's slot of the static field `name` of `class` ([`static_field`](Self::static_field)'s),
    /// remembered by the name's address: a `'static` name's bytes never change, so the answer for it holds until the
    /// class does.
    pub fn static_slot(&self, class: ClassId, name: &'static [u8]) -> Option<usize> {
        let address = name.as_ptr() as usize;
        let mix = (address as u64 ^ u64::from(class).wrapping_mul(0x9e37_79b9_7f4a_7c15))
            .wrapping_mul(0xff51_afd7_ed55_8ccd);
        let cell = &self.slot_memo[(mix >> 54) as usize % SLOT_MEMO_SIZE];
        let memo = cell.get();
        if memo.class == class && memo.name == address && memo.len == name.len() {
            return (memo.slot != usize::MAX).then_some(memo.slot);
        }
        let slot = self
            .static_field(class, name)
            .and_then(|(_, slots)| slots.first().copied());
        cell.set(SlotMemo {
            class,
            name: address,
            len: name.len(),
            slot: slot.unwrap_or(usize::MAX),
        });
        slot
    }

    pub fn name(&self, class: ClassId) -> &[u8] {
        self.get(class).map_or(&b""[..], |c| c.name.as_slice())
    }

    pub fn len(&self) -> usize {
        self.classes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    /// The class itself and its ancestors, nearest first. A parent is always registered before its
    /// children, so the chain ends; the bound is a precaution.
    fn chain(&self, class: ClassId) -> impl Iterator<Item = &ClassDef> {
        let mut next = self.get(class);
        std::iter::from_fn(move || {
            let current = next?;
            next = current.parent.and_then(|p| self.get(p));
            Some(current)
        })
        .take(self.classes.len())
    }

    pub fn is_a(&self, class: ClassId, ancestor: ClassId) -> bool {
        let Some(target) = self.get(ancestor) else {
            return false;
        };
        self.chain(class).any(|c| std::ptr::eq(c, target))
    }

    /// The static field `name` of `class` (declared by it or an ancestor) and the slots of its elements.
    pub fn static_field(&self, class: ClassId, name: &[u8]) -> Option<&(StaticField, Vec<usize>)> {
        self.get(class)?.layout.field(name)
    }

    /// The nearest `on_add` hook of the class or an ancestor.
    pub fn on_add_hook(&self, class: ClassId) -> Option<ClassHook> {
        self.chain(class).find_map(|c| c.on_add.clone())
    }

    /// The nearest `on_remove` hook of the class or an ancestor.
    pub fn on_remove_hook(&self, class: ClassId) -> Option<ClassHook> {
        self.chain(class).find_map(|c| c.on_remove.clone())
    }

    /// The behaviours of the class and its ancestors, the root's first: C++ overrides call their parent's
    /// first (`GameBaseData::onAdd` links the namespaces before a subclass's own `onAdd` runs). `on_remove` walks
    /// the list backwards.
    pub fn behaviours(&self, class: ClassId) -> Vec<Rc<dyn crate::classes::EngineClass>> {
        let mut out: Vec<Rc<dyn crate::classes::EngineClass>> = self
            .chain(class)
            .filter_map(|c| c.behaviour.clone())
            .collect();
        out.reverse();
        out
    }
}

/// Dynamic fields: case-insensitive names, kept in name order. TGE iterates them in the order of
/// its string table's pointer hashes, which cannot be reproduced; its `dump` sorts them by name, as this
/// order does.
///
/// Once its object is registered, the fields count their bytes in the `Vm`'s [`MemoryAccount`]
/// ([`attach`](Self::attach)), with what the object keeps besides them: a write the account cannot take is not made.
#[derive(Debug, Default)]
pub struct Fields {
    /// Folded name -> (the spelling the field was created with, the value).
    map: BTreeMap<Vec<u8>, (Vec<u8>, Vec<u8>)>,
    /// What the fields count ([`field_size`] of each).
    held: usize,
    /// What their object counts besides them ([`SimObject::own_size`]), once attached.
    other: usize,
    account: Option<MemoryAccount>,
}

/// What one field counts in the account: its key, its spelling, its value's room and [`FIELD_COST`].
fn field_size(key: &[u8], name: &[u8], value: &Vec<u8>) -> usize {
    FIELD_COST + key.len() + name.len() + value.capacity()
}

impl PartialEq for Fields {
    fn eq(&self, other: &Fields) -> bool {
        self.map == other.map
    }
}

impl Eq for Fields {}

impl Clone for Fields {
    /// A copy on no account (the copy's object attaches it when it is registered).
    fn clone(&self) -> Fields {
        let map = self.map.clone();
        let held = map
            .iter()
            .map(|(key, (name, value))| field_size(key, name, value))
            .sum();
        Fields {
            map,
            held,
            other: 0,
            account: None,
        }
    }
}

impl Drop for Fields {
    fn drop(&mut self) {
        if let Some(account) = &self.account {
            account.settle(self.held + self.other, 0);
        }
    }
}

impl Fields {
    pub fn get(&self, name: &[u8]) -> Option<&[u8]> {
        with_folded(name, |k| self.map.get(k)).map(|(_, v)| v.as_slice())
    }

    /// Counts the fields, and `other` bytes their object keeps besides them, in `account` from now on; false, and nothing
    /// changed, when the account cannot take them (the object should not be registered).
    pub fn attach(&mut self, account: &MemoryAccount, other: usize) -> bool {
        let before = self.held + self.other;
        let attached = self.account.as_ref().is_some_and(|a| a.same(account));
        let counted = if attached { before } else { 0 };
        if !account.fits((self.held + other).saturating_sub(counted)) {
            return false;
        }
        if let Some(old) = self.account.take() {
            old.settle(before, 0);
        }
        self.other = other;
        account.settle(0, self.held + other);
        self.account = Some(account.clone());
        true
    }

    /// Whether the account takes `extra` more bytes (always, before the fields are attached).
    pub(crate) fn fits(&self, extra: usize) -> bool {
        self.account.as_ref().is_none_or(|a| a.fits(extra))
    }

    /// Notes a refused store on the account (none before the fields are attached).
    pub(crate) fn refuse(&self) {
        if let Some(account) = &self.account {
            account.refuse();
        }
    }

    /// Their object keeps `other` bytes besides them now (nothing is counted before the fields are attached).
    pub(crate) fn set_other(&mut self, other: usize) {
        if let Some(account) = &self.account {
            account.settle(self.other, other);
            self.other = other;
        }
    }

    /// The fields held `before` bytes and hold `after` now (`held` and `account` are the fields', apart from the map a
    /// caller may be borrowing).
    fn settle(held: &mut usize, account: Option<&MemoryAccount>, before: usize, after: usize) {
        *held = held.saturating_sub(before).saturating_add(after);
        if let Some(account) = account {
            account.settle(before, after);
        }
    }

    /// Sets a field; the empty string deletes it. A write the account cannot take is not made.
    pub fn set(&mut self, name: &[u8], value: &[u8]) {
        let value = until_nul(value);
        // An existing field is found by a folded copy of the name on the stack and its value buffer is reused; only a
        // new field allocates (a key, a spelling and a value).
        let done = with_folded(name, |key| {
            let Some((spelling, old)) = self.map.get_mut(key) else {
                return false;
            };
            let before = field_size(key, spelling, old);
            let account = self.account.as_ref();
            if value.is_empty() {
                self.map.remove(key);
                Fields::settle(&mut self.held, account, before, 0);
                return true;
            }
            let extra = value.len().saturating_sub(old.capacity());
            if let Some(account) = account
                && extra > 0
                && !account.fits(extra)
            {
                account.refuse();
                return true;
            }
            refill(old, value);
            let after = field_size(key, spelling, old);
            Fields::settle(&mut self.held, account, before, after);
            true
        });
        if done || value.is_empty() {
            return;
        }
        let key = fold(name);
        let value = value.to_vec();
        let size = field_size(&key, name, &value);
        if !self.fits(size) {
            self.refuse();
            return;
        }
        self.map.insert(key, (name.to_vec(), value));
        Fields::settle(&mut self.held, self.account.as_ref(), 0, size);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// (spelling, value), sorted by name ignoring case.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.map.values().map(|(n, v)| (n.as_slice(), v.as_slice()))
    }

    /// The work of cloning these fields (a `: Parent` copy): one unit per field and one per byte it holds (the
    /// folded name the map is keyed by, the spelling and the value). Walks the fields without copying any.
    pub fn copy_cost(&self) -> u64 {
        self.map.iter().fold(0u64, |sum, (key, (name, value))| {
            sum.saturating_add(1)
                .saturating_add(key.len() as u64)
                .saturating_add(name.len() as u64)
                .saturating_add(value.len() as u64)
        })
    }
}

/// The members of a set in TGE's order: appended at the end, removed by moving the last member
/// into the gap (`[A B C D].remove(B)` leaves `A D C`).
#[derive(Debug, Clone, Default)]
pub struct Members {
    list: Vec<ObjId>,
    index: crate::idhash::IdMap<ObjId, usize>,
}

impl Members {
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn get(&self, i: usize) -> Option<ObjId> {
        self.list.get(i).copied()
    }

    pub fn contains(&self, id: ObjId) -> bool {
        self.index.contains_key(&id)
    }

    /// The place of a member in the list (`getObject(n)`'s n).
    pub fn position(&self, id: ObjId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    pub fn as_slice(&self) -> &[ObjId] {
        &self.list
    }

    fn push(&mut self, id: ObjId) -> bool {
        if self.index.contains_key(&id) {
            return false;
        }
        self.index.insert(id, self.list.len());
        self.list.push(id);
        true
    }

    /// Sorts the list by ascending id (TGE's `sortId`).
    fn sort_by_id(&mut self) {
        self.list.sort_unstable();
        for (position, member) in self.list.iter().enumerate() {
            self.index.insert(*member, position);
        }
    }

    fn swap_remove(&mut self, id: ObjId) -> bool {
        let Some(i) = self.index.remove(&id) else {
            return false;
        };
        if i < self.list.len() {
            self.list.swap_remove(i);
            if let Some(&moved) = self.list.get(i) {
                self.index.insert(moved, i);
            }
        }
        true
    }

    /// Moves a member to the front or the back, the others keeping their order (TGE's
    /// `reOrder`). Returns how many entries were rewritten (the work done).
    fn move_to(&mut self, id: ObjId, front: bool) -> usize {
        let Some(&i) = self.index.get(&id) else {
            return 0;
        };
        if i >= self.list.len() {
            return 0;
        }
        self.list.remove(i);
        if front {
            self.list.insert(0, id);
        } else {
            self.list.push(id);
        }
        for (position, member) in self.list.iter().enumerate() {
            self.index.insert(*member, position);
        }
        self.list.len()
    }
}

/// Names to objects. Several objects may share a name; the one most recently entered wins, and an
/// older one is found again when the newer one leaves.
#[derive(Debug, Clone, Default)]
struct NameDict {
    by_name: crate::namehash::NameMap<Vec<u8>, BTreeMap<u64, ObjId>>,
}

impl NameDict {
    fn insert(&mut self, name: &[u8], seq: u64, id: ObjId) {
        self.by_name.entry(fold(name)).or_default().insert(seq, id);
    }

    fn remove(&mut self, name: &[u8], seq: u64) {
        let key = fold(name);
        if let Some(entries) = self.by_name.get_mut(&key) {
            entries.remove(&seq);
            if entries.is_empty() {
                self.by_name.remove(&key);
            }
        }
    }

    fn find(&self, name: &[u8]) -> Option<ObjId> {
        self.by_name.get(&fold(name))?.values().next_back().copied()
    }
}

/// What a set or group holds: its members and, for a group, its own name table (`Group/child`).
#[derive(Debug, Clone, Default)]
struct Container {
    members: Members,
    names: NameDict,
}

/// One object. Created unregistered (id 0, unless given a datablock id), entered in the tables by
/// [`Objects::register`].
pub struct SimObject {
    id: ObjId,
    class: ClassId,
    name: Option<Vec<u8>>,
    /// Folded name of the namespace methods are looked up in: the class name, unless a ScriptObject or
    /// ScriptGroup linked its own.
    namespace: Vec<u8>,
    pub fields: Fields,
    /// The static fields' elements, by the class layout's slots.
    statics: Vec<Value>,
    group: Option<ObjId>,
    /// The plain sets (not the group) this object is a member of.
    sets: BTreeSet<ObjId>,
    members: Option<Box<Container>>,
    name_seq: u64,
    group_seq: u64,
    removing: bool,
    /// Script methods of this object that are running (TGE keeps a list of held objects and a flag):
    /// while it is not 0, `delete` only marks the object.
    locks: u32,
    /// `delete` was asked for while the object was held.
    delete_pending: bool,
    /// State a Rust class keeps for the object (the seam for engine classes added later).
    pub ext: Option<Box<dyn Any>>,
    /// The host made the object itself (`new` in C++, not in script): its `ModStaticFields` and `ModDynamicFields` flags
    /// stay clear (script's `new` sets them), so a script reads every field of it as
    /// "" and its writes are dropped (a popup's list, scroll and background read `profile`, `position`, `extent` as "").
    pub engine_made: bool,
    /// Torque3D's additions to an object, made when one is first set (see [`ObjectExtras`]).
    extras: Option<Box<ObjectExtras>>,
}

/// The most dynamic-field type tags one object keeps (a tag is looked up by a walk of them).
pub const MAX_FIELD_TYPES: usize = 256;

/// What Torque3D's console adds to an object: its internal name (`new Class([name])` with `internal`, found
/// through its sets with `->`, [`Objects::find_internal`]) and the console-type tags of its dynamic fields
/// (`SETCURFIELD_TYPE`). A tag is the type's id; it changes nothing about the field's value. Follows Torque3D's
/// MIT-licensed `simObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (c) 2015 Faust Logic, Inc.) and
/// `simSet.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.
#[derive(Debug, Default, Clone)]
struct ObjectExtras {
    internal_name: Option<Vec<u8>>,
    /// Folded field name (with its array index appended) and type id.
    field_types: Vec<(Vec<u8>, u32)>,
}

impl SimObject {
    pub fn new(class: ClassId, classes: &ClassRegistry) -> SimObject {
        SimObject {
            id: 0,
            class,
            name: None,
            namespace: fold(classes.name(class)),
            fields: Fields::default(),
            statics: classes
                .get(class)
                .map(|c| c.layout.initial.clone())
                .unwrap_or_default(),
            group: None,
            sets: BTreeSet::new(),
            members: classes.is_a(class, SIM_SET).then(Box::default),
            name_seq: 0,
            group_seq: 0,
            removing: false,
            locks: 0,
            delete_pending: false,
            engine_made: false,
            ext: None,
            extras: None,
        }
    }

    /// The internal name (Torque3D's `getInternalName`), if it has one.
    pub fn internal_name(&self) -> Option<&[u8]> {
        self.extras.as_ref()?.internal_name.as_deref()
    }

    /// Sets the internal name (Torque3D's `setInternalName`); an empty name clears it. A name the memory account cannot
    /// take is not set.
    pub fn set_internal_name(&mut self, name: &[u8]) {
        let name = until_nul(name);
        let old = self.internal_name().map_or(0, <[u8]>::len);
        if !self.has_room(name.len().saturating_sub(old)) {
            return;
        }
        let extras = self.extras.get_or_insert_with(Box::default);
        extras.internal_name = (!name.is_empty()).then(|| name.to_vec());
        self.settle_own();
    }

    /// What the object counts in the memory account besides its dynamic fields: [`OBJECT_COST`], its name (kept twice:
    /// here and in the name table), its internal name and the texts of its static fields.
    pub fn own_size(&self) -> usize {
        let name = self.name.as_ref().map_or(0, Vec::len);
        let statics: usize = self.statics.iter().map(Value::held_bytes).sum();
        OBJECT_COST + 2 * name + self.internal_name().map_or(0, <[u8]>::len) + statics
    }

    /// Whether the memory account takes `extra` more bytes for the object; a refusal is noted.
    pub(crate) fn has_room(&self, extra: usize) -> bool {
        if extra == 0 || self.fields.fits(extra) {
            return true;
        }
        self.fields.refuse();
        false
    }

    /// Brings the account up to date with [`own_size`](Self::own_size) (after a write of a static field or a name).
    pub(crate) fn settle_own(&mut self) {
        let size = self.own_size();
        self.fields.set_other(size);
    }

    /// The console-type tag of the dynamic field `name` (its array index appended), if one was set.
    pub fn dynamic_field_type(&self, name: &[u8]) -> Option<u32> {
        let key = fold(name);
        let extras = self.extras.as_ref()?;
        extras
            .field_types
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, t)| *t)
    }

    /// Tags the dynamic field `name` (its array index appended) with the console type `type_id`.
    /// Past [`MAX_FIELD_TYPES`] tags a new one is not kept.
    pub fn set_dynamic_field_type(&mut self, name: &[u8], type_id: u32) {
        let key = fold(name);
        let extras = self.extras.get_or_insert_with(Box::default);
        if let Some(entry) = extras.field_types.iter_mut().find(|(k, _)| *k == key) {
            entry.1 = type_id;
        } else if extras.field_types.len() < MAX_FIELD_TYPES {
            extras.field_types.push((key, type_id));
        }
    }

    pub fn id(&self) -> ObjId {
        self.id
    }

    pub fn class(&self) -> ClassId {
        self.class
    }

    pub fn name(&self) -> Option<&[u8]> {
        self.name.as_deref()
    }

    pub fn namespace(&self) -> &[u8] {
        &self.namespace
    }

    pub fn group(&self) -> Option<ObjId> {
        self.group
    }

    pub fn members(&self) -> Option<&Members> {
        self.members.as_deref().map(|c| &c.members)
    }

    /// The plain sets (not the group; see [`group`](Self::group)) this object is a member of, in
    /// ascending id order.
    pub fn sets(&self) -> impl Iterator<Item = ObjId> + '_ {
        self.sets.iter().copied()
    }

    pub fn is_removing(&self) -> bool {
        self.removing
    }

    /// How many script methods of this object are running (see `Vm::hold`).
    pub fn locks(&self) -> u32 {
        self.locks
    }

    /// A `delete` waits for the last of those methods to return.
    pub fn delete_pending(&self) -> bool {
        self.delete_pending
    }

    /// An explicit id (RootGroup's, a datablock's) for an object that is not registered yet;
    /// [`Objects::register`] refuses one that is taken.
    pub fn set_id(&mut self, id: ObjId) {
        self.id = id;
    }

    /// The name of an object that is not registered yet (`assignName` before `registerObject`).
    pub fn set_initial_name(&mut self, name: &[u8]) {
        let name = until_nul(name);
        self.name = (!name.is_empty()).then(|| name.to_vec());
    }

    /// The namespace methods are looked up in (set by the ScriptObject class linking).
    pub(crate) fn set_namespace(&mut self, namespace: &[u8]) {
        self.namespace = fold(namespace);
    }

    /// A text static field's value (`class`, `superClass`; empty when there is none).
    pub fn static_value(&self, classes: &ClassRegistry, field: &[u8]) -> Vec<u8> {
        let Some((_, slots)) = classes.static_field(self.class, field) else {
            return Vec::new();
        };
        match slots.first().and_then(|s| self.statics.get(*s)) {
            Some(Value::Text(t)) => t.to_vec(),
            _ => Vec::new(),
        }
    }

    /// Whether a text static field was ever assigned, an empty string included. TGE keeps a NULL string-table entry for a field
    /// never assigned and an entry for the empty text of `field = "";`, and a few preloads tell them apart (a sequence list
    /// ends at the first NULL). Script cannot tell them apart; this does by the value being the object's
    /// class's initial one: an object starts with the layout's `Rc`, a write makes a new one, a copy of an unset field
    /// shares it.
    pub fn static_text_is_set(&self, classes: &ClassRegistry, field: &[u8]) -> bool {
        let Some((_, slots)) = classes.static_field(self.class, field) else {
            return false;
        };
        let Some(&slot) = slots.first() else {
            return false;
        };
        let initial = classes
            .get(self.class)
            .and_then(|c| c.layout.initial.get(slot));
        match (self.statics.get(slot), initial) {
            (Some(Value::Text(now)), Some(Value::Text(first))) => !Rc::ptr_eq(now, first),
            (Some(Value::Text(_)), _) => true,
            _ => false,
        }
    }

    /// The element of a static field an index names, as `getDataField` picks it:
    /// `atoi` of the index text, which the VM always passes (empty when there is none, so 0); -1 is
    /// element 0 of a one-element field; anything outside the elements is none (reads ""). The interpreter's
    /// index is never NULL, so a plain read is element 0 of an array field too.
    pub fn read_slot(field: &(StaticField, Vec<usize>), array: &[u8]) -> Option<usize> {
        let index = atoi(array);
        let (spec, slots) = field;
        let index = if index == -1 && spec.count == 1 {
            0
        } else {
            index
        };
        usize::try_from(index)
            .ok()
            .and_then(|i| slots.get(i).copied())
    }

    /// The element a write goes to (`setDataField`: `atoi` of the index, 0 to count-1; -1 writes nothing).
    pub fn write_slot(field: &(StaticField, Vec<usize>), array: &[u8]) -> Option<usize> {
        usize::try_from(atoi(array))
            .ok()
            .and_then(|i| field.1.get(i).copied())
    }

    /// How many static elements the object holds (the work of making or copying it).
    pub fn slot_count(&self) -> usize {
        self.statics.len()
    }

    /// One static element (None for a slot the layout does not have).
    pub fn slot(&self, slot: usize) -> Option<&Value> {
        self.statics.get(slot)
    }

    pub fn slot_mut(&mut self, slot: usize) -> Option<&mut Value> {
        self.statics.get_mut(slot)
    }

    /// `getDataField` for a host that has no `Objects` at hand: a datablock field reads "".
    pub fn data_field(&self, classes: &ClassRegistry, name: &[u8], array: &[u8]) -> Vec<u8> {
        self.data_field_named(classes, name, array, &|_| None)
    }

    /// `setDataField` from a host (not the interpreter): a static field is parsed by its type without the
    /// engine around the object (texts are not interned, `./` is kept, a datablock name finds nothing and
    /// prints nothing, no validator runs); the interpreter's writes go through `Vm::write_field`.
    pub fn set_data_field(
        &mut self,
        classes: &ClassRegistry,
        name: &[u8],
        array: &[u8],
        value: &[u8],
    ) {
        if let Some(field) = classes.static_field(self.class, name) {
            let ty = field.0.ty;
            if let Some(slot) = Self::write_slot(field, array)
                && let Some(stored) = self.statics.get_mut(slot)
            {
                fields::set(ty, stored, value, &mut PlainEnv(classes.field_rules));
                self.settle_own();
            }
            return;
        }
        with_dynamic_name(name, array, |full| self.fields.set(full, value));
    }

    /// `getDataField`: a static field when the class declares `name` (see [`read_slot`](Self::read_slot);
    /// `names` gives the names of the objects a datablock field points at), otherwise the dynamic field
    /// `name` + `array`. Not charged to the step budget (see [`data_field_charged`](Self::data_field_charged)).
    pub fn data_field_named(
        &self,
        classes: &ClassRegistry,
        name: &[u8],
        array: &[u8],
        names: &dyn Fn(ObjId) -> Option<Vec<u8>>,
    ) -> Vec<u8> {
        self.data_field_charged(classes, name, array, names, &mut |_| true)
    }

    /// [`data_field_named`](Self::data_field_named) for a reader that pays for the work (the interpreter's
    /// field reads): a text, a dynamic value or an object's name is charged by its length before it is copied,
    /// a list by the bytes it formats (see `fields::get_charged`). `charge` returns false once the budget is
    /// spent; the read then gives "" (or a part of a list) and the caller's run ends at its next instruction.
    pub fn data_field_charged(
        &self,
        classes: &ClassRegistry,
        name: &[u8],
        array: &[u8],
        names: &dyn Fn(ObjId) -> Option<Vec<u8>>,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Vec<u8> {
        if let Some(field) = classes.static_field(self.class, name) {
            return Self::read_slot(field, array)
                .and_then(|slot| self.statics.get(slot))
                .map(|value| {
                    fields::get_charged(classes.field_rules, field.0.ty, value, names, charge)
                })
                .unwrap_or_default();
        }
        let mut out = Vec::new();
        self.dynamic_field_charged_into(name, array, charge, &mut out);
        out
    }

    /// [`data_field_charged`](Self::data_field_charged) appending the value to `out` (a buffer the caller reuses): a
    /// dynamic field is copied once, into it.
    pub fn data_field_charged_into(
        &self,
        classes: &ClassRegistry,
        name: &[u8],
        array: &[u8],
        names: &dyn Fn(ObjId) -> Option<Vec<u8>>,
        charge: &mut dyn FnMut(u64) -> bool,
        out: &mut Vec<u8>,
    ) {
        if classes.static_field(self.class, name).is_some() {
            out.extend_from_slice(&self.data_field_charged(classes, name, array, names, charge));
            return;
        }
        self.dynamic_field_charged_into(name, array, charge, out);
    }

    fn dynamic_field_charged_into(
        &self,
        name: &[u8],
        array: &[u8],
        charge: &mut dyn FnMut(u64) -> bool,
        out: &mut Vec<u8>,
    ) {
        with_dynamic_name(name, array, |full| {
            if let Some(value) = self.fields.get(full)
                && charge(value.len() as u64)
            {
                out.extend_from_slice(value);
            }
        });
    }

    /// `setDataField` of a DYNAMIC field (the empty string deletes it). False when `name` is a static field
    /// of the class: the interpreter writes those through `Vm::write_field`, which needs the runtime around
    /// the object (the string table, datablock lookups, the console).
    pub fn set_dynamic_field(
        &mut self,
        classes: &ClassRegistry,
        name: &[u8],
        array: &[u8],
        value: &[u8],
    ) -> bool {
        if classes.static_field(self.class, name).is_some() {
            return false;
        }
        with_dynamic_name(name, array, |full| self.fields.set(full, value));
        true
    }

    /// `assignFieldsFrom` (the `: parent` of a declaration): the dynamic fields always; the static ones only between
    /// objects of the same class, and through TEXT (see [`static_texts_of`](Self::static_texts_of) and
    /// [`set_static_texts`](Self::set_static_texts), which the caller runs when the classes are the same). This copies
    /// the dynamic fields and nothing else. The copy is charged before it is made: `charge` gets one unit per static
    /// element (none across classes) and [`Fields::copy_cost`] for the dynamic fields (a deep copy: every byte of every
    /// name and value). False, with nothing copied, when `charge` returns false (the budget is spent). (No validator
    /// runs on the copied statics.)
    pub fn copy_fields_from(
        &mut self,
        parent: &SimObject,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> bool {
        let same_class = self.class == parent.class;
        let slots = if same_class { parent.statics.len() } else { 0 };
        if !charge((slots as u64).saturating_add(parent.fields.copy_cost())) {
            return false;
        }
        self.fields = parent.fields.clone();
        true
    }

    /// The text the copy takes from each static element of this object, in the class's field order (each element: the
    /// type's getter, and if it returns a pointer the setter):
    /// `(slot, type, text)`. A getter that returns NULL gives nothing, so the element is not copied: a string that was
    /// never assigned (a NULL pointer in TGE), a datablock pointer that is null or points at an object with no name
    /// (`names` gives a name), and a `deprecated` field. Everything else goes through its printed form
    /// ([`FieldRules`](crate::fields::FieldRules)): a float arrives rounded to six digits and a rotation as the axis
    /// and angle it prints.
    pub fn static_texts_of(
        &self,
        classes: &ClassRegistry,
        names: &dyn Fn(ObjId) -> Option<Vec<u8>>,
    ) -> Vec<(usize, FieldType, Vec<u8>)> {
        let Some(def) = classes.get(self.class) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (field, slots) in &def.layout.fields {
            if field.ty == FieldType::Deprecated {
                continue;
            }
            for &slot in slots {
                let Some(value) = self.statics.get(slot) else {
                    continue;
                };
                match value {
                    Value::Text(now) => {
                        let unset = def.layout.initial.get(slot).is_some_and(|first| {
                            matches!(first, Value::Text(first) if first.is_empty() && Rc::ptr_eq(now, first))
                        });
                        if unset {
                            continue;
                        }
                    }
                    Value::Object(None) => continue,
                    Value::Object(Some(id)) if names(*id).is_none() => continue,
                    _ => {}
                }
                out.push((
                    slot,
                    field.ty,
                    fields::get_with(classes.field_rules, field.ty, value, names),
                ));
            }
        }
        out
    }

    /// The setter half of the copy: each text of [`static_texts_of`](Self::static_texts_of) parsed by its type into
    /// this object's slot, without the field's validator, in the order given (a position and a rotation share one
    /// matrix: the position first).
    pub fn set_static_texts(
        &mut self,
        texts: &[(usize, FieldType, Vec<u8>)],
        env: &mut dyn fields::FieldEnv,
    ) {
        for (slot, ty, text) in texts {
            if let Some(stored) = self.statics.get_mut(*slot) {
                fields::set(*ty, stored, text, env);
            }
        }
        self.settle_own();
    }
}

/// The runtime around an object, for a host's direct writes: nothing (see `SimObject::set_data_field`).
struct PlainEnv(fields::FieldRules);

impl fields::FieldEnv for PlainEnv {
    fn rules(&self) -> fields::FieldRules {
        self.0
    }
    fn intern(&mut self, text: &[u8]) -> Rc<[u8]> {
        Rc::from(text)
    }
    fn expand_filename(&mut self, text: &[u8]) -> Option<Vec<u8>> {
        Some(text.to_vec())
    }
    fn find_object(&mut self, _text: &[u8], _class: &str) -> Option<ObjId> {
        None
    }
    fn message(&mut self, _level: crate::vm::Level, _text: String) {}
    fn charge(&mut self, _units: u64) -> bool {
        true
    }
}

/// A field spec as a script names a field in one string (`"name[index]"`, what Torque3D's `setFieldValue` takes): the
/// name up to the first `[`, and the index after it up to the first `]` (to the end when there is none); the whole spec
/// is the name when it has no `[`. Each part is cut at its first NUL, as a field write cuts them. Join the parts with
/// [`dynamic_name`] for the field's name.
pub fn split_field_spec(spec: &[u8]) -> (&[u8], &[u8]) {
    let spec = until_nul(spec);
    match spec.iter().position(|&b| b == b'[') {
        None => (spec, &[]),
        Some(open) => {
            let rest = &spec[open + 1..];
            let close = rest.iter().position(|&b| b == b']').unwrap_or(rest.len());
            (&spec[..open], &rest[..close])
        }
    }
}

/// The dynamic field name for `name[array]`, as every field read and write builds it: each part up to its first NUL, the
/// index appended to the name. TGE copies name and index into a 255-byte buffer, so a name with an index is cut at
/// [`MAX_FIELD_NAME`] bytes; a name without one is used as it is. A host that checks field names (a policy on what a
/// script language may write) builds them with this, so it sees the name the write will use.
pub fn dynamic_name(name: &[u8], array: &[u8]) -> Vec<u8> {
    with_dynamic_name(name, array, <[u8]>::to_vec)
}

/// [`dynamic_name`] handed to `f`, built on the stack: a field read or written by a script costs no name allocation.
pub fn with_dynamic_name<R>(name: &[u8], array: &[u8], f: impl FnOnce(&[u8]) -> R) -> R {
    let name = until_nul(name);
    let array = until_nul(array);
    if array.is_empty() {
        return f(name);
    }
    let mut full = [0u8; MAX_FIELD_NAME];
    let name_len = name.len().min(MAX_FIELD_NAME);
    full[..name_len].copy_from_slice(&name[..name_len]);
    let array_len = array.len().min(MAX_FIELD_NAME - name_len);
    full[name_len..name_len + array_len].copy_from_slice(&array[..array_len]);
    f(&full[..name_len + array_len])
}

/// What [`Objects::register`] did. `wrapped`: the dynamic id counter wrapped (the caller may say so).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registered {
    pub id: ObjId,
    pub wrapped: bool,
}

/// Every registered object, the global name table and the id counters.
pub struct Objects {
    pub classes: ClassRegistry,
    /// Boxed: the table's doublings move a pointer an object, not the object (half a million objects made a doubling of the
    /// inline table move about 100 MB inside one frame).
    live: crate::idhash::IdMap<ObjId, Box<SimObject>>,
    names: NameDict,
    next_seq: u64,
    last_dynamic: ObjId,
    config: SimConfig,
    next_datablock: ObjId,
    /// The last id of the datablock range ([`Limits::datablock_id_limit`](crate::policy::Limits::datablock_id_limit)): once the
    /// counter has passed it, datablock registration fails.
    datablock_limit: ObjId,
    /// [`next_datablock_id`](Self::next_datablock_id) passes over ids that are taken (a host's choice).
    pub skip_taken_datablock_ids: bool,
    datablock_group: ObjId,
    work: Cell<u64>,
    /// Ids unregistered since the VM last looked ([`take_unregistered`](Self::take_unregistered)): their
    /// pending events go. One entry per unregistered object; the VM looks at the start of every outermost
    /// run, in every event native and before every event fires.
    unregistered: Vec<ObjId>,
    /// What registered objects and their dynamic fields count in (the `Vm`'s account).
    memory: MemoryAccount,
}

impl Default for Objects {
    fn default() -> Self {
        Self::new()
    }
}

impl Objects {
    /// The tables with RootGroup and the default start-up sets and groups ([`SimConfig::default`]).
    pub fn new() -> Objects {
        Objects::with_config(SimConfig::default())
    }

    /// The tables with RootGroup and `config`'s start-up sets and groups.
    pub fn with_config(config: SimConfig) -> Objects {
        let mut objects = Objects {
            classes: ClassRegistry::new(),
            live: crate::idhash::IdMap::default(),
            names: NameDict::default(),
            next_seq: 0,
            last_dynamic: config.last_dynamic_id,
            config,
            next_datablock: FIRST_DATABLOCK_ID,
            datablock_limit: crate::policy::Limits::default().datablock_id_limit(),
            skip_taken_datablock_ids: false,
            datablock_group: 0,
            work: Cell::new(0),
            unregistered: Vec::new(),
            memory: MemoryAccount::default(),
        };
        let mut root = SimObject::new(SIM_GROUP, &objects.classes);
        root.set_id(ROOT_GROUP_ID);
        root.set_initial_name(b"RootGroup");
        objects.register(root);
        for &(name, class) in config.start_objects {
            let mut object = SimObject::new(class, &objects.classes);
            object.set_initial_name(name.as_bytes());
            if let Some(registered) = objects.register(object) {
                objects.add_to_group(ROOT_GROUP_ID, registered.id);
            }
        }
        objects.datablock_group = objects.names.find(b"DataBlockGroup").unwrap_or(0);
        objects
    }

    pub fn add_work(&self, units: u64) {
        self.work.set(self.work.get().saturating_add(units));
    }

    /// The account registered objects and their dynamic fields count in.
    pub fn memory(&self) -> &MemoryAccount {
        &self.memory
    }

    /// The work counted since the last call (the VM charges it to the step budget).
    pub(crate) fn take_work(&self) -> u64 {
        self.work.replace(0)
    }

    /// The work counted so far, left in place (see [`Vm::ensure_budget`](crate::vm::Vm)).
    pub(crate) fn peek_work(&self) -> u64 {
        self.work.get()
    }

    /// The ids unregistered since the last call (the VM drops their pending events).
    pub(crate) fn take_unregistered(&mut self) -> Vec<ObjId> {
        std::mem::take(&mut self.unregistered)
    }

    fn next_seq(&mut self) -> u64 {
        self.next_seq = self.next_seq.wrapping_add(1);
        self.next_seq
    }

    /// Enters an object in the id and name tables (`registerObject` before `onAdd`).
    /// Id 0 gets the next dynamic id: the counter is incremented first, wraps to the config's start ([`SimConfig`])
    /// past its `max_dynamic_id` and skips ids in use. None (and the object is dropped) when an
    /// explicit id is already taken, or when the [`memory`](Self::memory) account cannot take the object and its fields.
    pub fn register(&mut self, mut object: SimObject) -> Option<Registered> {
        let own = object.own_size();
        if !object.fields.attach(&self.memory, own) {
            self.memory.refuse();
            return None;
        }
        let mut wrapped = false;
        if object.id == 0 {
            let mut id = self.last_dynamic.saturating_add(1);
            // The bound is tested once, before the search for a free id; the
            // skip loop below does not test it again, so a skip may hand out one past it.
            let (base, max) = (self.config.last_dynamic_id, self.config.max_dynamic_id);
            if id > max {
                wrapped = true;
                id = base;
            }
            // With every id taken this gives up after one lap.
            let mut tries = u64::from(max.saturating_sub(base)) + 2;
            while self.live.contains_key(&id) {
                tries -= 1;
                if tries == 0 {
                    return None;
                }
                self.add_work(1);
                id = id.saturating_add(1);
            }
            self.last_dynamic = id;
            object.id = id;
        } else if self.live.contains_key(&object.id) {
            return None;
        }
        let id = object.id;
        object.removing = false;
        object.locks = 0;
        object.delete_pending = false;
        object.group = None;
        if let Some(name) = object.name.clone() {
            object.name_seq = self.next_seq();
            self.names.insert(&name, object.name_seq, id);
        }
        self.live.insert(id, Box::new(object));
        Some(Registered { id, wrapped })
    }

    /// Takes an object out of every table: its group, its sets, the name and id tables; a set's or
    /// group's members are released (no group, not in this set). Runs no callbacks. The object is
    /// returned so the caller decides when it is dropped.
    pub fn unregister(&mut self, id: ObjId) -> Option<SimObject> {
        let mut object = *self.live.remove(&id)?;
        self.unregistered.push(id);
        if let Some(name) = &object.name {
            self.names.remove(name, object.name_seq);
        }
        if let Some(group) = object.group {
            self.leave_group(group, id, object.name.as_deref(), object.group_seq);
        }
        self.add_work(object.sets.len() as u64);
        for set in &object.sets {
            if let Some(container) = self.container_mut(*set) {
                container.members.swap_remove(id);
            }
        }
        if let Some(container) = &object.members {
            self.add_work(container.members.len() as u64);
            for member in container.members.as_slice() {
                if let Some(m) = self.live.get_mut(member) {
                    m.sets.remove(&id);
                    if m.group == Some(id) {
                        m.group = None;
                    }
                }
            }
        }
        // The returned object keeps no links: registering it again starts from a clean state.
        object.sets.clear();
        if let Some(container) = &mut object.members {
            **container = Container::default();
        }
        object.group = None;
        Some(object)
    }

    pub fn get(&self, id: ObjId) -> Option<&SimObject> {
        self.live.get(&id).map(|o| &**o)
    }

    pub fn get_mut(&mut self, id: ObjId) -> Option<&mut SimObject> {
        self.live.get_mut(&id).map(|o| &mut **o)
    }

    pub fn contains(&self, id: ObjId) -> bool {
        self.live.contains_key(&id)
    }

    /// The number of registered objects.
    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Every registered object with its id, in ascending id order (RootGroup, whose id is `u32::MAX`,
    /// comes last). A deterministic walk for layers that must visit all objects (shutdown, ghosting,
    /// "every object of class X"); collecting the ids costs O(n log n), so it is not for hot paths.
    pub fn iter(&self) -> impl Iterator<Item = (ObjId, &SimObject)> + '_ {
        let mut ids: Vec<ObjId> = self.live.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(move |id| self.live.get(&id).map(|object| (id, &**object)))
    }

    fn container(&self, id: ObjId) -> Option<&Container> {
        self.live.get(&id)?.members.as_deref()
    }

    fn container_mut(&mut self, id: ObjId) -> Option<&mut Container> {
        self.live.get_mut(&id)?.members.as_deref_mut()
    }

    pub fn is_set(&self, id: ObjId) -> bool {
        self.container(id).is_some()
    }

    pub fn is_group(&self, id: ObjId) -> bool {
        self.live
            .get(&id)
            .is_some_and(|o| o.members.is_some() && self.classes.is_a(o.class, SIM_GROUP))
    }

    pub fn members(&self, set: ObjId) -> Option<&Members> {
        self.container(set).map(|c| &c.members)
    }

    pub fn is_member(&self, set: ObjId, id: ObjId) -> bool {
        self.members(set).is_some_and(|m| m.contains(id))
    }

    /// `Sim::findObject`: `""` finds nothing; text starting with a digit is `atoi` of the text up to the
    /// first `/`; text starting with `/` starts at RootGroup; otherwise a name. What follows a `/` is
    /// looked up below the object found so far (see [`find_below`](Self::find_below)).
    pub fn find(&self, text: &[u8]) -> Option<ObjId> {
        let text = until_nul(text);
        let (&first, rest) = text.split_first()?;
        if first == b'/' {
            return self.find_below(ROOT_GROUP_ID, rest);
        }
        let slash = text.iter().position(|&c| c == b'/');
        let head = &text[..slash.unwrap_or(text.len())];
        let start = if first.is_ascii_digit() {
            let id = atoi(head) as ObjId;
            self.live.contains_key(&id).then_some(id)?
        } else {
            self.names.find(head)?
        };
        match slash {
            None => Some(start),
            Some(at) => self.find_below(start, &text[at + 1..]),
        }
    }

    /// The member of the set `set` whose internal name is `name` (any case; Torque3D's
    /// `SimSet::findObjectByInternalName`): its members in order, and with `recurse` each member that is a set is
    /// searched right after it is passed over (depth first). A set reached again through another (a set may hold
    /// itself) is not searched twice. Every member looked at is charged as one work unit.
    pub fn find_internal(&self, set: ObjId, name: &[u8], recurse: bool) -> Option<ObjId> {
        let name = until_nul(name);
        let mut seen = std::collections::HashSet::new();
        seen.insert(set);
        // Each entry: a set and the position of the next member to look at.
        let mut stack = vec![(set, 0usize)];
        while let Some((current, at)) = stack.pop() {
            let Some(member) = self.members(current).and_then(|m| m.get(at)) else {
                continue;
            };
            stack.push((current, at + 1));
            self.add_work(1);
            let Some(object) = self.get(member) else {
                continue;
            };
            if object
                .internal_name()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
            {
                return Some(member);
            }
            if recurse && self.is_set(member) && seen.insert(member) {
                stack.push((member, 0));
            }
        }
        None
    }

    /// The object at `path` (`child/grandchild`) below `start`: a group looks each name up in its own
    /// name table (newest first), a plain set searches its members in order, anything else has nothing
    /// below it.
    pub fn find_below(&self, start: ObjId, path: &[u8]) -> Option<ObjId> {
        let mut current = start;
        let mut path = until_nul(path);
        loop {
            let slash = path.iter().position(|&c| c == b'/');
            let segment = &path[..slash.unwrap_or(path.len())];
            let object = self.live.get(&current)?;
            let container = object.members.as_deref()?;
            let next = if self.classes.is_a(object.class, SIM_GROUP) {
                container.names.find(segment)?
            } else {
                self.add_work(container.members.len() as u64);
                container.members.as_slice().iter().copied().find(|m| {
                    self.live
                        .get(m)
                        .and_then(|o| o.name.as_deref())
                        .is_some_and(|n| n.eq_ignore_ascii_case(segment))
                })?
            };
            if !self.live.contains_key(&next) {
                return None;
            }
            match slash {
                None => return Some(next),
                Some(at) => {
                    current = next;
                    path = &path[at + 1..];
                }
            }
        }
    }

    /// `assignName` on a registered object; the empty name clears it. The group's name table follows.
    pub fn set_name(&mut self, id: ObjId, name: &[u8]) {
        let name = until_nul(name);
        let Some(object) = self.live.get(&id) else {
            return;
        };
        let old_len = object.name.as_ref().map_or(0, Vec::len);
        if !object.has_room(2 * name.len().saturating_sub(old_len)) {
            return;
        }
        let (old, old_seq, group, group_seq) = (
            object.name.clone(),
            object.name_seq,
            object.group,
            object.group_seq,
        );
        if let Some(old) = &old {
            self.names.remove(old, old_seq);
            if let Some(container) = group.and_then(|g| self.container_mut(g)) {
                container.names.remove(old, group_seq);
            }
        }
        let new = (!name.is_empty()).then(|| name.to_vec());
        let (seq, new_group_seq) = (self.next_seq(), self.next_seq());
        if let Some(new) = &new {
            self.names.insert(new, seq, id);
            if let Some(container) = group.and_then(|g| self.container_mut(g)) {
                container.names.insert(new, new_group_seq, id);
            }
        }
        if let Some(object) = self.live.get_mut(&id) {
            object.name = new;
            object.name_seq = seq;
            object.group_seq = new_group_seq;
            object.settle_own();
        }
    }

    /// The stored value of the static field `name` of a registered object, its first element: None when the object or the
    /// field does not exist (a dynamic field is text). A host's hot read of a number or a flag, which [`field`](Self::field)
    /// would format as text for the host to parse again.
    pub fn static_value(&self, id: ObjId, name: &[u8]) -> Option<&Value> {
        let object = self.live.get(&id)?;
        let field = self.classes.static_field(object.class(), name)?;
        SimObject::read_slot(field, b"").and_then(|slot| object.slot(slot))
    }

    /// A field of a registered object (None when the object does not exist). Not charged to the step budget: a
    /// host's read; the interpreter's reads go through [`SimObject::data_field_charged`].
    pub fn field(&self, id: ObjId, name: &[u8], array: &[u8]) -> Option<Vec<u8>> {
        let names = |id: ObjId| self.get(id).and_then(|o| o.name().map(<[u8]>::to_vec));
        self.live
            .get(&id)
            .map(|o| o.data_field_named(&self.classes, name, array, &names))
    }

    /// Sets a field of a registered object as a host does (`SimObject::set_data_field`: no string table, no
    /// datablock lookup, no validator); false when the object does not exist. The interpreter's writes go
    /// through `Vm::write_field`.
    pub fn set_field(&mut self, id: ObjId, name: &[u8], array: &[u8], value: &[u8]) -> bool {
        let classes = &self.classes;
        match self.live.get_mut(&id) {
            Some(object) => {
                object.set_data_field(classes, name, array, value);
                true
            }
            None => false,
        }
    }

    fn leave_group(&mut self, group: ObjId, id: ObjId, name: Option<&[u8]>, seq: u64) {
        if let Some(container) = self.container_mut(group) {
            container.members.swap_remove(id);
            if let Some(name) = name {
                container.names.remove(name, seq);
            }
        }
    }

    /// `SimGroup::addObject`: moves `id` into `group` and out of its previous group.
    fn add_to_group(&mut self, group: ObjId, id: ObjId) -> bool {
        // A group may hold itself (TGE has no check); deleting it is then deliberately
        // finite, see `Vm::delete_object`.
        if !self.is_group(group) {
            return false;
        }
        let Some(object) = self.live.get(&id) else {
            return false;
        };
        if object.group == Some(group) {
            return true;
        }
        let (old, name, old_seq) = (object.group, object.name.clone(), object.group_seq);
        if let Some(old) = old {
            self.leave_group(old, id, name.as_deref(), old_seq);
        }
        let seq = self.next_seq();
        if let Some(container) = self.container_mut(group) {
            container.members.push(id);
            if let Some(name) = &name {
                container.names.insert(name, seq, id);
            }
        }
        if let Some(object) = self.live.get_mut(&id) {
            object.group = Some(group);
            object.group_seq = seq;
        }
        true
    }

    fn remove_from_group(&mut self, group: ObjId, id: ObjId) -> bool {
        let Some(object) = self.live.get(&id) else {
            return false;
        };
        if object.group != Some(group) {
            return false;
        }
        let (name, seq) = (object.name.clone(), object.group_seq);
        self.leave_group(group, id, name.as_deref(), seq);
        if let Some(object) = self.live.get_mut(&id) {
            object.group = None;
        }
        true
    }

    /// `set.add(obj)`: a group takes the object out of its previous group; a plain set appends it unless
    /// it is already a member (a set may hold itself). False when `set` is not a set or
    /// `id` does not exist.
    pub fn add(&mut self, set: ObjId, id: ObjId) -> bool {
        if self.is_group(set) {
            return self.add_to_group(set, id);
        }
        if !self.live.contains_key(&id) {
            return false;
        }
        let Some(container) = self.container_mut(set) else {
            return false;
        };
        if container.members.push(id)
            && let Some(object) = self.live.get_mut(&id)
        {
            object.sets.insert(set);
        }
        true
    }

    /// `set.remove(obj)`: false when `id` is not a member.
    pub fn remove(&mut self, set: ObjId, id: ObjId) -> bool {
        if self.is_group(set) {
            return self.remove_from_group(set, id);
        }
        let removed = self
            .container_mut(set)
            .is_some_and(|c| c.members.swap_remove(id));
        if removed && let Some(object) = self.live.get_mut(&id) {
            object.sets.remove(&set);
        }
        removed
    }

    /// Takes `id` out of the member list of `set` whatever its links say (for ids a group cascade has
    /// left behind), and clears its links to `set`.
    pub fn forget_member(&mut self, set: ObjId, id: ObjId) {
        let in_group = self
            .live
            .get(&id)
            .and_then(|o| (o.group == Some(set)).then(|| (o.name.clone(), o.group_seq)));
        if let Some(container) = self.container_mut(set) {
            container.members.swap_remove(id);
            if let Some((Some(name), seq)) = &in_group {
                container.names.remove(name, *seq);
            }
        }
        if let Some(object) = self.live.get_mut(&id) {
            object.sets.remove(&set);
            if object.group == Some(set) {
                object.group = None;
            }
        }
    }

    /// `bringToFront` / `pushToBack`: nothing happens to a non-member.
    pub fn move_member(&mut self, set: ObjId, id: ObjId, to_front: bool) {
        let moved = self
            .container_mut(set)
            .map_or(0, |c| c.members.move_to(id, to_front));
        self.add_work(moved as u64);
    }

    /// `set.clear()`: every member leaves (a group's members end up in no group). Returns how many.
    pub fn clear(&mut self, set: ObjId) -> usize {
        let members: Vec<ObjId> = self
            .members(set)
            .map(|m| m.as_slice().to_vec())
            .unwrap_or_default();
        self.add_work(members.len() as u64);
        for id in &members {
            if !self.remove(set, *id) {
                self.forget_member(set, *id);
            }
        }
        members.len()
    }

    /// Marks an object as being removed (its `onRemove` is about to run). False when it does not exist
    /// or is already being removed.
    pub fn begin_removal(&mut self, id: ObjId) -> bool {
        match self.live.get_mut(&id) {
            Some(object) if !object.removing => {
                object.removing = true;
                true
            }
            _ => false,
        }
    }

    /// A script method of `id` starts. Nothing for an id that does not exist.
    pub(crate) fn lock(&mut self, id: ObjId) {
        if let Some(object) = self.live.get_mut(&id) {
            object.locks = object.locks.saturating_add(1);
        }
    }

    /// A script method of `id` returned. True when it was the last one and a delete
    /// is pending: the caller carries it out now. An object a group cascade removed meanwhile is gone,
    /// and nothing happens.
    pub(crate) fn unlock(&mut self, id: ObjId) -> bool {
        let Some(object) = self.live.get_mut(&id) else {
            return false;
        };
        object.locks = object.locks.saturating_sub(1);
        if object.locks > 0 || !object.delete_pending {
            return false;
        }
        object.delete_pending = false;
        true
    }

    /// `deleteObject` on a held object (TGE's deferred delete):
    /// records the delete. False when the object does not exist or nothing holds it.
    pub(crate) fn defer_delete(&mut self, id: ObjId) -> bool {
        match self.live.get_mut(&id) {
            Some(object) if object.locks > 0 => {
                object.delete_pending = true;
                true
            }
            _ => false,
        }
    }

    /// TGE unlinks each member from its group before removing it during the group's deletion but
    /// leaves it in the group's list until the group itself is gone (`getCount()` stays
    /// constant).
    pub fn unlink_from_group(&mut self, id: ObjId) {
        if let Some(object) = self.live.get_mut(&id) {
            object.group = None;
        }
    }

    /// Sorts a set's member list by ascending id, as `SimGroup::onRemove` does before it walks the list
    /// (the members' `onRemove` callbacks see the sorted list). The list is charged.
    pub fn sort_members(&mut self, set: ObjId) {
        let Some(container) = self.container_mut(set) else {
            return;
        };
        let len = container.members.len();
        container.members.sort_by_id();
        self.add_work(len as u64);
    }

    /// A group's members that are registered and not being removed, by ascending id. The whole list is
    /// scanned (a cascaded member stays in it, as a dead id, until the group is gone), so that is what
    /// is charged.
    pub fn live_members_by_id(&self, set: ObjId) -> Vec<ObjId> {
        let Some(members) = self.members(set) else {
            return Vec::new();
        };
        self.add_work(members.len() as u64);
        let mut ids: Vec<ObjId> = members
            .as_slice()
            .iter()
            .copied()
            .filter(|id| self.live.get(id).is_some_and(|o| !o.removing))
            .collect();
        ids.sort_unstable();
        ids
    }

    /// DataBlockGroup's id, resolved once at start-up. If a script deletes the group, the id stays: the
    /// group would be dangling there; this lets datablocks declared
    /// afterwards register without a group (placed like any `new`: `$instantGroup` or RootGroup, not
    /// counted by [`datablock_count`](Self::datablock_count), and deletable).
    pub fn datablock_group(&self) -> ObjId {
        self.datablock_group
    }

    /// The number of objects in DataBlockGroup (`getDataBlockGroupSize`).
    pub fn datablock_count(&self) -> usize {
        self.members(self.datablock_group).map_or(0, Members::len)
    }

    /// `SimDataBlock::assignId`: the next datablock id. The counter keeps advancing, also past the limit.
    /// With [`skip_taken_datablock_ids`](Self::skip_taken_datablock_ids), an id another object holds is passed over: these
    /// tables keep one object an id, so a host whose own datablocks share the range with ones it was given (a networked
    /// copy's) gets the next free id instead of a refused registration.
    /// Without it (a server) the taken id is handed out and the registration is refused.
    pub fn next_datablock_id(&mut self) -> ObjId {
        let mut id = self.next_datablock;
        while self.skip_taken_datablock_ids
            && self.live.contains_key(&id)
            && id <= self.datablock_limit
        {
            self.add_work(1);
            id = id.saturating_add(1);
        }
        self.next_datablock = id.saturating_add(1);
        id
    }

    /// `SimDataBlock::onAdd`'s check: has the counter passed the last id of the datablock range?
    pub fn datablocks_exhausted(&self) -> bool {
        self.next_datablock > self.datablock_limit
    }

    /// The last id of the datablock range.
    pub fn datablock_id_limit(&self) -> ObjId {
        self.datablock_limit
    }

    /// Whether `id` lies in the datablock range.
    pub fn is_datablock_id(&self, id: ObjId) -> bool {
        (FIRST_DATABLOCK_ID..=self.datablock_limit).contains(&id)
    }

    /// Sets the last id of the datablock range (see [`Limits`](crate::policy::Limits); `Vm::set_limits` sets it).
    pub fn set_datablock_id_limit(&mut self, limit: ObjId) {
        self.datablock_limit = limit;
    }

    /// For hosts restoring a state, and tests.
    pub fn set_next_datablock_id(&mut self, id: ObjId) {
        self.next_datablock = id;
    }

    /// The last dynamic id handed out; the next object gets the one after it. For hosts and tests.
    pub fn set_last_dynamic_id(&mut self, id: ObjId) {
        self.last_dynamic = id;
    }
}

#[cfg(test)]
#[path = "sim_tests.rs"]
mod tests;
