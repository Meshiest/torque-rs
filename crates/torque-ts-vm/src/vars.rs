//! Script variables. As in TGE, a variable remembers how it was last set: as an integer, a
//! float (stored as 32-bit) or a string, and caches the other numeric views at that moment.
//!
//! Follows Torque3D's MIT-licensed `console/consoleInternal.cpp` (`Dictionary`) (Copyright (c) 2012 GarageGames, LLC);
//! the code here is this crate's own.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::text::{atof, atoi, f64_to_u32, fold, format_g, format_int, until_nul};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Int,
    Float,
    Str,
}

/// The C type of a variable the host registered (`Con::addVariable`, a global of the host's): the value lives in
/// that C variable and every set goes through the type's `setData`, every read through its `getData`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineType {
    /// `atoi` in, `%d` out.
    S32,
    /// `dAtob` in, `1` / `0` out.
    Bool,
    /// `(F32)atof` in, `%g` out.
    F32,
    /// The text itself (TGE's string table keeps it; this keeps the text).
    Str,
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// The name as it was first written (`export` prints it).
    name: Vec<u8>,
    kind: Kind,
    sval: Vec<u8>,
    ival: Cell<u32>,
    fval: Cell<f32>,
    /// The two views are still to be read from `sval` ([`string_views`]): a string store leaves them for the first read that
    /// wants a number (the text does not change in between, so they are the views the store would have made).
    stale: Cell<bool>,
    /// Set for a variable the host registered ([`Dictionary::add_engine`]).
    engine: Option<EngineType>,
}

impl Entry {
    fn new(name: &[u8]) -> Entry {
        Entry {
            name: until_nul(name).to_vec(),
            kind: Kind::Str,
            sval: Vec::new(),
            ival: Cell::new(0),
            fval: Cell::new(0.0),
            stale: Cell::new(false),
            engine: None,
        }
    }

    /// Makes a spare entry what [`Entry::new`] makes, keeping its buffers (a short value's room; a long one's is let go).
    fn reset(&mut self, name: &[u8]) {
        refill(&mut self.name, until_nul(name));
        self.kind = Kind::Str;
        refill(&mut self.sval, b"");
        self.ival.set(0);
        self.fval.set(0.0);
        self.stale.set(false);
        self.engine = None;
    }

    /// The host's C type, for a variable it registered.
    pub fn engine_type(&self) -> Option<EngineType> {
        self.engine
    }

    /// What a registered variable holds after its type's `setData(text)`: the text of its `getData`, with the views
    /// `getIntValue` (`atoi` of it) and `getFloatValue` (`atof` of it) read from that text.
    fn store_engine(&mut self, t: EngineType, text: &[u8]) {
        let text = until_nul(text);
        self.sval = match t {
            EngineType::S32 => atoi(text).to_string().into_bytes(),
            EngineType::Bool => {
                let on = text.eq_ignore_ascii_case(b"true") || atof(text) as f32 != 0.0;
                if on { b"1".to_vec() } else { b"0".to_vec() }
            }
            EngineType::F32 => format_g(f64::from(atof(text) as f32)).into_bytes(),
            EngineType::Str => text.to_vec(),
        };
        self.ival.set(atoi(&self.sval) as u32);
        self.fval.set(atof(&self.sval) as f32);
        self.stale.set(false);
        self.kind = Kind::Str;
    }

    /// The name as it was first written.
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// Whether the value was last set as a string, or the variable is one the host registered (its `getData` text): `export`
    /// quotes those; integers and floats print bare.
    pub fn is_string(&self) -> bool {
        self.kind == Kind::Str
    }

    /// The first `max` bytes of [`get_string`](Self::get_string), without copying the rest of a long value.
    pub fn string_head(&self, max: usize) -> Vec<u8> {
        match self.kind {
            Kind::Str => self.sval.get(..max).unwrap_or(&self.sval).to_vec(),
            _ => self.get_string(),
        }
    }

    /// Reads the views of a string store that has not been read as a number yet.
    fn fresh_views(&self) {
        if self.stale.get() {
            let (i, f) = string_views(&self.sval);
            self.ival.set(i);
            self.fval.set(f);
            self.stale.set(false);
        }
    }

    pub fn get_int(&self) -> u32 {
        self.fresh_views();
        self.ival.get()
    }

    /// The 32-bit float view, widened.
    pub fn get_float(&self) -> f64 {
        self.fresh_views();
        f64::from(self.fval.get())
    }

    pub fn get_string(&self) -> Vec<u8> {
        match self.kind {
            Kind::Str => self.sval.clone(),
            Kind::Float => format_g(f64::from(self.fval.get())).into_bytes(),
            Kind::Int => format_int(self.ival.get()).into_bytes(),
        }
    }

    /// [`get_string`](Self::get_string) handed to `f` without copying a string value.
    pub fn with_string<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        match self.kind {
            Kind::Str => f(&self.sval),
            Kind::Float => f(format_g(f64::from(self.fval.get())).as_bytes()),
            Kind::Int => f(format_int(self.ival.get()).as_bytes()),
        }
    }

    pub fn set_int(&mut self, value: u32) {
        if let Some(t) = self.engine {
            // `setIntValue` of a registered variable: `getData(TypeS32, &value)` handed to its type's `setData`.
            return self.store_engine(t, format_int(value).as_bytes());
        }
        self.fval.set(value as f32);
        self.ival.set(value);
        self.stale.set(false);
        self.sval.clear();
        self.kind = Kind::Int;
    }

    /// Stores the value as a 32-bit float. The integer view is the C float to unsigned
    /// conversion of the narrowed value ([`f64_to_u32`]: truncate, keep the low 32 bits, 0 when out of
    /// the 64-bit range or not a number).
    pub fn set_float(&mut self, value: f64) {
        let narrow = value as f32;
        if let Some(t) = self.engine {
            // `setFloatValue(F32)`: `getData(TypeF32, &value)` (`%g`) handed to the type's `setData`.
            return self.store_engine(t, format_g(f64::from(narrow)).as_bytes());
        }
        self.fval.set(narrow);
        self.ival.set(f64_to_u32(f64::from(narrow)));
        self.stale.set(false);
        self.sval.clear();
        self.kind = Kind::Float;
    }

    pub fn set_string(&mut self, value: &[u8]) {
        if let Some(t) = self.engine {
            return self.store_engine(t, value);
        }
        let value = until_nul(value);
        refill(&mut self.sval, value);
        self.stale.set(true);
        self.kind = Kind::Str;
    }
}

/// Puts `value` in `buffer`, reusing its room unless that is far more than `value` needs (a variable once set to a long string
/// gives the room back when a short one replaces it).
pub fn refill(buffer: &mut Vec<u8>, value: &[u8]) {
    if buffer.capacity() > 1024 && buffer.capacity() / 4 > value.len() {
        *buffer = value.to_vec();
    } else {
        buffer.clear();
        buffer.extend_from_slice(value);
    }
}

/// The integer and float views a variable set to the text `value` has (`LOADVAR_UINT`, `LOADVAR_FLT` read them): the float
/// view is `(F32)atof`, and the integer view is the float one converted as `set_float` does (a string `"4e9"` reads as
/// 4000000000 in `| 0`, `"0x10"` as 16, `"1e-45"` as 0, `"2147483647"` as 2^31), not `atoi`'s. A text of 256 bytes or more
/// reads as 0 in both. For a host that does a script's work natively, so its tests of a local are the script's.
pub fn string_views(value: &[u8]) -> (u32, f32) {
    let value = until_nul(value);
    if value.len() < 256 {
        let fval = atof(value) as f32;
        (f64_to_u32(f64::from(fval)), fval)
    } else {
        (0, 0.0)
    }
}

/// A set of variables. Names are case-insensitive, and a name is taken up to its first NUL byte (as TGE's C strings end
/// there), so `$a\0b` is `$a`: the key and the spelling an entry keeps (what `export` writes) are the same name.
///
/// Each entry sits in a slot that keeps its place while the dictionary holds the entry, so an executor can remember its
/// current variable by slot ([`slot_folded`](Self::slot_folded)) and reach it again ([`at_mut`](Self::at_mut)) without
/// hashing the name, as long as the dictionary's [`stamp`](Self::stamp) is the same: the stamp is unique to this dictionary
/// and changes whenever an entry is removed (adding one moves none).
#[derive(Debug)]
pub struct Dictionary {
    /// Folded name to slot.
    index: crate::namehash::NameMap<Key, usize>,
    slots: Vec<Option<Entry>>,
    /// Slots emptied by a removal, for the next additions.
    free: Vec<usize>,
    stamp: u64,
    /// Entries a [`clear`](Self::clear) let go, with their buffers, for the next additions (a frame's dictionary goes back to
    /// the VM's pool and serves the next call).
    spare: Vec<Entry>,
}

/// The entries a dictionary keeps for reuse after a clear.
const SPARE_ENTRIES: usize = 32;

/// A folded name as the index keeps it: a short one in place, so adding a variable allocates nothing for its key. It
/// hashes and compares as its bytes (`Borrow<[u8]>`), so the index is searched with a plain slice.
#[derive(Debug, Clone)]
enum Key {
    Short { len: u8, bytes: [u8; SHORT_KEY] },
    Long(Box<[u8]>),
}

/// The longest name a [`Key`] holds in place.
const SHORT_KEY: usize = 22;

impl Key {
    fn new(name: &[u8]) -> Key {
        let mut bytes = [0u8; SHORT_KEY];
        match bytes.get_mut(..name.len()) {
            Some(short) => {
                short.copy_from_slice(name);
                Key::Short {
                    len: name.len() as u8,
                    bytes,
                }
            }
            None => Key::Long(name.into()),
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            Key::Short { len, bytes } => &bytes[..usize::from(*len)],
            Key::Long(name) => name,
        }
    }
}

impl std::borrow::Borrow<[u8]> for Key {
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}

impl std::hash::Hash for Key {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for Key {}

/// The source of [`Dictionary::stamp`]s: never the same twice in a process.
static NEXT_STAMP: AtomicU64 = AtomicU64::new(1);

fn new_stamp() -> u64 {
    NEXT_STAMP.fetch_add(1, Ordering::Relaxed)
}

impl Default for Dictionary {
    fn default() -> Dictionary {
        Dictionary {
            index: crate::namehash::NameMap::default(),
            slots: Vec::new(),
            free: Vec::new(),
            stamp: new_stamp(),
            spare: Vec::new(),
        }
    }
}

impl Dictionary {
    pub fn get(&self, name: &[u8]) -> Option<&Entry> {
        crate::text::with_folded(name, |k| self.get_folded(k))
    }

    pub fn get_mut(&mut self, name: &[u8]) -> Option<&mut Entry> {
        let slot = crate::text::with_folded(name, |k| self.slot_folded(k))?;
        self.at_mut(slot)
    }

    /// The entry of an already folded name.
    pub fn get_folded(&self, key: &[u8]) -> Option<&Entry> {
        self.at(self.slot_folded(key)?)
    }

    /// [`get_folded`](Self::get_folded), to change it.
    pub fn get_mut_folded(&mut self, key: &[u8]) -> Option<&mut Entry> {
        let slot = self.slot_folded(key)?;
        self.at_mut(slot)
    }

    /// The slot of the entry of an already folded name.
    #[inline]
    pub fn slot_folded(&self, key: &[u8]) -> Option<usize> {
        self.index.get(until_nul(key)).copied()
    }

    /// The entry in `slot` (see the type's documentation).
    #[inline]
    pub fn at(&self, slot: usize) -> Option<&Entry> {
        self.slots.get(slot)?.as_ref()
    }

    /// [`at`](Self::at), to change it.
    #[inline]
    pub fn at_mut(&mut self, slot: usize) -> Option<&mut Entry> {
        self.slots.get_mut(slot)?.as_mut()
    }

    /// What tells this dictionary, as it is now, apart from every other and from itself before a removal.
    #[inline]
    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    /// [`add`](Self::add) with `key` the folded `name` (no copy of the key when the entry exists).
    pub fn add_folded(&mut self, key: &[u8], name: &[u8]) {
        self.add_slot_folded(key, name);
    }

    /// [`add_folded`](Self::add_folded), giving the entry's slot.
    pub fn add_slot_folded(&mut self, key: &[u8], name: &[u8]) -> usize {
        if let Some(slot) = self.slot_folded(key) {
            return slot;
        }
        let slot = self.new_slot(name);
        self.index.insert(Key::new(until_nul(key)), slot);
        slot
    }

    /// A slot holding a new entry spelled `name`.
    fn new_slot(&mut self, name: &[u8]) -> usize {
        let entry = match self.spare.pop() {
            Some(mut entry) => {
                entry.reset(name);
                entry
            }
            None => Entry::new(name),
        };
        match self.free.pop() {
            Some(slot) => {
                self.slots[slot] = Some(entry);
                slot
            }
            None => {
                self.slots.push(Some(entry));
                self.slots.len() - 1
            }
        }
    }

    /// The entry for `name`, created empty when missing (with `name` as its spelling: the string
    /// table keeps the first spelling of a name, and so does an entry until it is removed).
    pub fn add(&mut self, name: &[u8]) -> &mut Entry {
        let key = fold(until_nul(name));
        let slot = match self.slot_folded(&key) {
            Some(slot) => slot,
            None => {
                let slot = self.new_slot(name);
                self.index.insert(Key::new(&key), slot);
                slot
            }
        };
        self.slots[slot].get_or_insert_with(|| Entry::new(name))
    }

    /// `Con::addVariable(name, type, &global)` before any script runs: the entry (spelled `name`, created if missing) is
    /// bound to a C variable of type `t` holding `initial` (the host's static value), and keeps the type until it is
    /// removed (`deleteVariables` drops the binding with the entry, as TGE's does).
    pub fn add_engine(&mut self, name: &[u8], t: EngineType, initial: &[u8]) {
        let entry = self.add(name);
        entry.engine = Some(t);
        entry.store_engine(t, initial);
    }

    /// Removes a variable; true when it existed.
    pub fn remove(&mut self, name: &[u8]) -> bool {
        let Some(slot) = self.index.remove(fold(until_nul(name)).as_slice()) else {
            return false;
        };
        if let Some(place) = self.slots.get_mut(slot) {
            *place = None;
        }
        self.free.push(slot);
        self.stamp = new_stamp();
        true
    }

    /// Every variable, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.slots.iter().flatten()
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Empties it, keeping its table (a frame's dictionary goes back to the VM's pool).
    pub fn clear(&mut self) {
        self.index.clear();
        let room = SPARE_ENTRIES.saturating_sub(self.spare.len());
        self.spare.extend(self.slots.drain(..).flatten().take(room));
        self.free.clear();
        self.stamp = new_stamp();
    }

    /// The entries its table holds without growing.
    pub fn capacity(&self) -> usize {
        self.index.capacity()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

#[cfg(test)]
mod engine_tests {
    use super::*;

    fn entry(t: EngineType, initial: &[u8]) -> Dictionary {
        let mut d = Dictionary::default();
        d.add_engine(b"$Pref::X", t, initial);
        d
    }

    #[test]
    fn a_registered_variable_reads_as_its_type_prints() {
        let mut d = entry(EngineType::S32, b"0");
        let e = d.get_mut(b"$pref::x").unwrap();
        e.set_string(b"2.7");
        assert_eq!(e.get_string(), b"2");
        e.set_float(2.5);
        assert_eq!(e.get_string(), b"2");
        e.set_float(1e10);
        assert_eq!(e.get_string(), b"1", "%g gives 1e+10, atoi reads 1");
        e.set_int(u32::MAX);
        assert_eq!((e.get_string(), e.get_int()), (b"-1".to_vec(), u32::MAX));
        assert!(e.is_string(), "export quotes it");
        assert_eq!(e.name(), b"$Pref::X", "the host's spelling");

        let mut d = entry(EngineType::Bool, b"1");
        let e = d.get_mut(b"$pref::x").unwrap();
        assert_eq!(e.get_string(), b"1");
        e.set_string(b"TRUE");
        assert_eq!(e.get_string(), b"1");
        e.set_string(b"abc");
        assert_eq!(e.get_string(), b"0");
        e.set_int(7);
        assert_eq!(e.get_string(), b"1");

        let mut d = entry(EngineType::F32, b"5");
        let e = d.get_mut(b"$pref::x").unwrap();
        assert_eq!(e.get_string(), b"5");
        e.set_int(1);
        assert_eq!(e.get_string(), b"1");
        e.set_string(b"0.1");
        assert_eq!(
            (e.get_string(), e.get_float() as f32),
            (b"0.1".to_vec(), 0.1f32)
        );

        let mut d = entry(EngineType::Str, b"");
        let e = d.get_mut(b"$pref::x").unwrap();
        e.set_int(3);
        assert_eq!(e.get_string(), b"3");
        assert!(e.is_string());
    }

    /// SECURITY (resource cap): a variable that held a long string gives its room back when a short one replaces it.
    #[test]
    fn a_short_string_after_a_long_one_gives_the_room_back() {
        let mut d = Dictionary::default();
        let e = d.add(b"%x");
        e.set_string(&vec![b'a'; 1 << 20]);
        e.set_string(b"b");
        assert!(e.sval.capacity() < 1024, "{}", e.sval.capacity());
        assert_eq!(e.get_string(), b"b");
        e.set_string(b"cd");
        assert_eq!((e.get_string(), e.get_int()), (b"cd".to_vec(), 0));
        let mut buffer = vec![0u8; 1 << 20];
        refill(&mut buffer, b"key");
        assert!(buffer.capacity() < 1024 && buffer == b"key");
    }

    /// A slot names its entry while the stamp stays; a removal or a clear changes the stamp, and no two dictionaries share
    /// one, so a slot remembered from another table or from before a removal is never taken for the entry.
    #[test]
    fn slots_hold_their_entries_until_the_stamp_changes() {
        let mut d = Dictionary::default();
        let a = d.add_slot_folded(b"%a", b"%A");
        let stamp = d.stamp();
        d.at_mut(a).unwrap().set_string(b"1");
        let b = d.add_slot_folded(b"%b", b"%b");
        assert_eq!((d.stamp(), d.slot_folded(b"%a")), (stamp, Some(a)));
        assert_eq!(d.add_slot_folded(b"%a", b"%a"), a);
        assert_eq!(d.at(a).unwrap().name(), b"%A");
        assert!(d.remove(b"%A"));
        assert_ne!(d.stamp(), stamp);
        assert!(d.at(a).is_none() && d.get(b"%a").is_none());
        let c = d.add_slot_folded(b"%c", b"%c");
        assert_eq!(c, a, "the emptied slot is used again");
        assert_eq!(d.get_folded(b"%b").map(Entry::name), Some(&b"%b"[..]));
        assert_eq!(d.at(b).map(Entry::name), Some(&b"%b"[..]));
        let before = d.stamp();
        d.clear();
        assert!(d.stamp() != before && d.is_empty() && d.iter().next().is_none());
        assert_ne!(Dictionary::default().stamp(), Dictionary::default().stamp());
    }

    #[test]
    fn a_name_is_the_same_variable_up_to_its_nul() {
        let mut d = Dictionary::default();
        d.add(b"$Pref::Dev\0x").set_string(b"1");
        assert_eq!(
            d.get(b"$pref::dev").map(Entry::get_string).as_deref(),
            Some(&b"1"[..])
        );
        assert_eq!(
            d.get(b"$Pref::Dev").map(Entry::name),
            Some(&b"$Pref::Dev"[..])
        );
        let slot = d.add_slot_folded(b"$pref::dev\0y", b"$Pref::Dev\0y");
        assert_eq!(d.slot_folded(b"$pref::dev"), Some(slot));
        assert_eq!(d.len(), 1);
        assert!(d.remove(b"$PREF::DEV\0z"));
        assert!(d.get(b"$Pref::Dev").is_none());
    }

    /// After a clear, the entries kept for reuse come back as new ones: the new spelling, no value, no host type; a long
    /// name is a key like a short one.
    #[test]
    fn a_cleared_dictionary_gives_fresh_entries() {
        let mut d = Dictionary::default();
        d.add_engine(b"$Old::Kept", EngineType::S32, b"5");
        d.add(b"%x").set_float(2.5);
        let long = [b'%'; 40];
        d.add(&long).set_string(b"long");
        d.clear();
        for name in [&b"%Y"[..], b"%z", &long] {
            let e = d.add(name);
            assert_eq!(e.name(), name);
            assert!(e.is_string() && e.get_string().is_empty() && e.engine_type().is_none());
            assert_eq!((e.get_int(), e.get_float()), (0, 0.0));
        }
        assert!(d.get(b"%y").is_some() && d.get(&long).is_some() && d.get(b"%x").is_none());
    }

    #[test]
    fn removing_a_registered_variable_drops_its_type() {
        let mut d = entry(EngineType::S32, b"4");
        assert!(d.remove(b"$PREF::X"));
        d.add(b"$pref::x").set_string(b"2.7");
        let e = d.get(b"$pref::x").unwrap();
        assert_eq!((e.get_string(), e.engine_type()), (b"2.7".to_vec(), None));
    }
}
