//! Tagged strings: the `NetStringTable`, following Torque3D's MIT-licensed `sim/netStringTable.cpp` (Copyright (c) 2012
//! GarageGames, LLC).
//! A tagged string is shown to scripts as the byte 0x01 followed by its id in decimal (`"\x0112"`);
//! the table maps ids to strings with two reference counts, one for engine users (a tagged literal that ran, a network
//! handle) and one for scripts (`addTaggedString`).
//!
//! As TGE's: ids start at 1 and freed ids are reused last-in first-out; the table starts with 16 slots and
//! doubles; strings match case-sensitively; a script removal of an id whose counts are both 0 is ignored. TGE
//! does not check ids against the table: an id at or past the size is treated as absent here, and so is a
//! negative one. A game can change both checks ([`TagRules`]).
//!
//! A limit TGE does not have: its table doubles without one until the address space runs out. This one stops
//! at [`MAX_SLOTS`] slots, so that a script cannot make the table take all the memory: a new string then gets
//! no id ([`TagTable::add_string`] returns `None`) and the caller fails the run with `VmError::TagTableFull`.
//! Strings that are already in the table, and ids that were freed, still work at the limit. Ordinary scripts
//! hold some dozens of tags.
//!
//! The slot count does not bound memory (a slot may hold a string of any length), so the table also caps
//! the bytes of tag text it holds, [`MAX_TAG_BYTES`], with the same outcome (`add_string` returns
//! `None`, the run fails with `TagTableFull`). The count includes the text a freed slot keeps (TGE
//! keeps it too, and a wrapped script count can bring it back to life), and each string is held once: the
//! slot and the lookup map share it.

use std::collections::HashMap;
use std::rc::Rc;

/// The free-list end marker.
const INVALID: u32 = u32::MAX;
/// Slots the table starts with.
const INITIAL_SIZE: u32 = 16;
/// Slots the table grows to at most (a power of two, as the doubling reaches it exactly): about a
/// million tags. The last slot is never handed out, as taking it would need a doubling.
pub const MAX_SLOTS: u32 = 1 << 20;
/// Bytes of tag text the table holds at most, freed slots' leftovers included (ordinary scripts hold
/// a few kilobytes). Like the event queue's [`MAX_PENDING_BYTES`](crate::events::MAX_PENDING_BYTES): a
/// script pays only a step per 64 bytes it writes, so the count of slots is not a bound on memory.
pub const MAX_TAG_BYTES: usize = 64 << 20;

/// The table's checks, where a game's differ. The defaults are Torque3D's.
#[derive(Debug, Clone, Copy)]
pub struct TagRules {
    /// A script removal of an id whose two counts are both 0 is ignored (TGE); false: the script count wraps to
    /// `u32::MAX`, and the string comes back to life.
    pub ignore_dead_removal: bool,
    /// Lookups, script references and removals of an id above the table's size print
    /// [`range_message`](Self::range_message) (and do nothing); an id equal to the size passes the check and is absent.
    /// False (TGE has no check): such an id is absent, silently.
    pub range_errors: bool,
    /// The range check's line: the table's function (`lookupString`, `incStringRefScript`, `removeString`), the id
    /// as a signed number, the table's size.
    pub range_message: fn(&str, i32, u32) -> String,
}

impl Default for TagRules {
    fn default() -> TagRules {
        TagRules {
            ignore_dead_removal: true,
            range_errors: false,
            range_message: |function, id, size| {
                format!("NetStringTable::{function}: tag id {id} is past the table ({size} slots)")
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Slot {
    string: Rc<[u8]>,
    refs: u32,
    script_refs: u32,
    next: u32,
}

/// The tagged string table.
#[derive(Debug, Clone)]
pub struct TagTable {
    slots: Vec<Slot>,
    first_free: u32,
    /// Live strings (in the hash chains) and their ids; the bytes are shared with the slot.
    by_string: HashMap<Rc<[u8]>, u32>,
    /// The bytes of every slot's string (see [`MAX_TAG_BYTES`]).
    held_bytes: usize,
    /// What `held_bytes` may reach: [`MAX_TAG_BYTES`] unless a host lowers it.
    byte_cap: usize,
    rules: TagRules,
}

impl Default for TagTable {
    fn default() -> Self {
        Self::new()
    }
}

impl TagTable {
    pub fn new() -> TagTable {
        let mut table = TagTable {
            slots: Vec::new(),
            first_free: 1,
            by_string: HashMap::new(),
            held_bytes: 0,
            byte_cap: MAX_TAG_BYTES,
            rules: TagRules::default(),
        };
        table.grow_to(INITIAL_SIZE);
        table
    }

    /// Sets the table's checks.
    pub fn set_rules(&mut self, rules: TagRules) {
        self.rules = rules;
    }

    /// The range check's message for `id` when the rules report it.
    fn out_of_range(&self, function: &str, id: u32) -> Option<String> {
        (self.rules.range_errors && id > self.size())
            .then(|| (self.rules.range_message)(function, id as i32, self.size()))
    }

    /// Sets the cap on the tag text held (default [`MAX_TAG_BYTES`]); text already held stays.
    pub fn set_byte_cap(&mut self, cap: usize) {
        self.byte_cap = cap;
    }

    /// The bytes of tag text held, freed slots' leftovers included (what [`MAX_TAG_BYTES`] bounds).
    pub fn held_bytes(&self) -> usize {
        self.held_bytes
    }

    /// The number of slots (the `size` the range messages print).
    pub fn size(&self) -> u32 {
        u32::try_from(self.slots.len()).unwrap_or(u32::MAX)
    }

    /// Appends free slots up to `size`, chained in order, the last one ending the list.
    fn grow_to(&mut self, size: u32) {
        let from = self.size();
        for i in from..size {
            self.slots.push(Slot {
                next: i.saturating_add(1),
                ..Slot::default()
            });
        }
        if let Some(last) = self.slots.last_mut() {
            last.next = INVALID;
        }
    }

    /// `NetStringTable::addString`: the id of `string`, entered when new; one more engine reference.
    /// `None` when the string is new and the table is at [`MAX_SLOTS`] with no free slot left, or would
    /// hold more than [`MAX_TAG_BYTES`] of text (nothing changes then).
    pub fn add_string(&mut self, string: &[u8]) -> Option<u32> {
        if let Some(&id) = self.by_string.get(string) {
            if let Some(slot) = self.slots.get_mut(id as usize) {
                slot.refs = slot.refs.wrapping_add(1);
            }
            return Some(id);
        }
        let id = self.first_free;
        let reused = self.slots.get(id as usize).map_or(0, |s| s.string.len());
        let held = self.held_bytes.saturating_sub(reused);
        if held.saturating_add(string.len()) > self.byte_cap {
            return None;
        }
        let next = self.slots.get(id as usize).map_or(INVALID, |s| s.next);
        if next == INVALID {
            // The table doubles as soon as the last free slot is taken.
            let size = self.size();
            if size >= MAX_SLOTS {
                return None;
            }
            self.first_free = size;
            self.grow_to(size.saturating_mul(2).clamp(INITIAL_SIZE, MAX_SLOTS));
        } else {
            self.first_free = next;
        }
        let shared: Rc<[u8]> = Rc::from(string);
        if let Some(slot) = self.slots.get_mut(id as usize) {
            slot.refs = slot.refs.wrapping_add(1);
            slot.string = Rc::clone(&shared);
        }
        self.held_bytes = held.saturating_add(string.len());
        self.by_string.insert(shared, id);
        Some(id)
    }

    /// The id of a live string, if the table holds it (no reference is taken).
    pub fn id_of(&self, string: &[u8]) -> Option<u32> {
        self.by_string.get(string).copied()
    }

    /// The engine and script reference counts of an id (`None` for an id outside the table).
    pub fn counts(&self, id: u32) -> Option<(u32, u32)> {
        let slot = self.slots.get(id as usize)?;
        Some((slot.refs, slot.script_refs))
    }

    /// `NetStringTable::lookupString`: the string of a live id. Messages for ids out of range are the
    /// caller's (see [`lookup_checked`](Self::lookup_checked)).
    pub fn lookup(&self, id: u32) -> Option<&[u8]> {
        let slot = self.slots.get(id as usize)?;
        if slot.refs == 0 && slot.script_refs == 0 {
            return None;
        }
        Some(&slot.string)
    }

    /// `lookupString` with the range check of the rules: `Err(message)` for an id it reports.
    pub fn lookup_checked(&self, id: u32) -> Result<Option<&[u8]>, String> {
        if let Some(message) = self.out_of_range("lookupString", id) {
            return Err(message);
        }
        Ok(self.lookup(id))
    }

    /// `incStringRefScript` with the range check of the rules.
    pub fn add_script_ref(&mut self, id: u32) -> Result<(), String> {
        if let Some(message) = self.out_of_range("incStringRefScript", id) {
            return Err(message);
        }
        if let Some(slot) = self.slots.get_mut(id as usize) {
            slot.script_refs = slot.script_refs.wrapping_add(1);
        }
        Ok(())
    }

    /// `NetStringTable::removeString`, with the checks of the rules. Returns the error message when there
    /// is one.
    pub fn remove_string(&mut self, id: u32, script: bool) -> Option<String> {
        if let Some(message) = self.out_of_range("removeString", id) {
            return Some(message);
        }
        let guard = self.rules.ignore_dead_removal;
        let slot = self.slots.get_mut(id as usize)?;
        if script {
            if guard && slot.script_refs == 0 && slot.refs == 0 {
                return None;
            }
            if slot.script_refs == 0 && slot.refs != 0 {
                return Some(format!(
                    "removeTaggedString failed!  Ref count is already 0 for string: {}",
                    String::from_utf8_lossy(&slot.string)
                ));
            }
            slot.script_refs = slot.script_refs.wrapping_sub(1);
            if slot.script_refs != 0 || slot.refs != 0 {
                return None;
            }
        } else {
            slot.refs = slot.refs.wrapping_sub(1);
            if slot.refs != 0 || slot.script_refs != 0 {
                return None;
            }
        }
        // Both counts are 0: off the hash chain (if this id is the one there) and onto the free list.
        // The string stays in the slot, as in TGE.
        if self.by_string.get(&slot.string) == Some(&id) {
            self.by_string.remove(&slot.string);
        }
        slot.next = self.first_free;
        self.first_free = id;
        None
    }
}

/// `"\x01"` followed by the id in decimal, cut to `digits` characters (the size of the buffer it is written to,
/// less its NUL: 8 for `addTaggedString`).
pub fn tag_text(id: u32, digits: usize) -> Vec<u8> {
    let mut text = vec![1u8];
    let mut number = id.to_string().into_bytes();
    number.truncate(digits);
    text.extend_from_slice(&number);
    text
}
