//! NetString: the per-connection string table that lets a remote command name a tag by 5 bits (`ConnectionStringTable`,
//! `checkString`, `NetStringEvent` pack / unpack / process).
//!
//! **Sender** ([`SendStrings`]): 32 entries, each holding the global tag it stands for and an "acked" flag, and an LRU list that
//! starts in index order, so the first 32 distinct tags take slots 0, 1, .., 31 and then the least recently used slot is re-used.
//! A hit moves the entry to the most-recently-used end; a **miss re-keys the LRU head at once** (flag cleared) and returns the
//! [`NetStringEvent`] the caller must post to the ordered queue before whatever uses the slot. Delivery of that event sets the
//! flag, but only if the slot still holds the same tag ([`SendStrings::delivered`]).
//!
//! **Receiver** ([`RemoteStrings`]): 32 handles, written only by a processed NetString event. Each handle holds one reference on a
//! tag of the host's global table (the host adds it when it processes the event and releases the displaced one that
//! [`RemoteStrings::set`] returns); nothing is released when the connection is removed, only when it is destroyed
//! ([`RemoteStrings::release_all`], one Sim millisecond later). The crate holds no VM: tag ids are `u32`s the host chooses.
//!
//! **Hostile input.** A slot is 5 bits, so it cannot be out of range; a string is at most 255 bytes (the string coder's own cap);
//! the sender's entries and the receiver's handles are arrays of 32.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `sim/connectionStringTable.cpp` and
//! `sim/netStringTable.cpp` (Copyright (c) 2012 GarageGames, LLC each); the code here is this crate's own.

use std::collections::{HashMap, VecDeque};

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::{NET_ARG_BYTES, NET_REMOTE_SLOTS};
use crate::events::EventCtx;
use crate::strcoder::{read_string, write_string};

/// `NetStringEvent`: a slot and the text it now stands for. `tag` is the sender's global tag id (kept to learn, on delivery,
/// whether the slot still means this tag); it is not on the wire, so a decoded event has `tag` 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetStringEvent {
    pub slot: u8,
    pub text: Vec<u8>,
    pub tag: u32,
}

impl NetStringEvent {
    /// `writeInt(5, slot)`, `writeString(text, 255)` (the packet's prefix-delta string buffer applies).
    pub fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>) {
        w.write_bits(u32::from(self.slot) & 31, 5);
        write_string(w, ctx.table, &self.text, NET_ARG_BYTES);
    }

    pub fn unpack(r: &mut BitReader, ctx: &EventCtx<'_>) -> Result<NetStringEvent, NetError> {
        let slot = r.read_bits(5)? as u8;
        let text = read_string(r, ctx.table)?;
        Ok(NetStringEvent { slot, text, tag: 0 })
    }
}

/// `Mapping string: <text> to index: <slot>`, the line `NetStringEvent::process` prints (TGE 1.4's `sim/connectionStringTable.cc`).
pub fn mapping_line(text: &[u8], slot: u8) -> Vec<u8> {
    let mut line = b"Mapping string: ".to_vec();
    line.extend_from_slice(text);
    line.extend_from_slice(b" to index: ");
    line.extend_from_slice(slot.to_string().as_bytes());
    line
}

#[derive(Debug, Clone, Copy, Default)]
struct SendEntry {
    tag: Option<u32>,
    acked: bool,
    /// The entry holds a reference on its tag, which the host must let go of when the entry is re-keyed or the table is destroyed. The
    /// host decides ([`SendStrings::set_owned`]): a tag id that named nothing when the entry was keyed took no reference (taking one
    /// on any id would count a reference on a dead slot below zero).
    owned: bool,
}

/// What [`SendStrings::check_string`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The slot the tag is (now) in.
    pub slot: u8,
    /// A hit whose NetString event was delivered: the slot may be sent by index without waiting for the event.
    pub acked: bool,
    /// On a miss, the event to post (ordered) before whatever uses the slot.
    pub event: Option<NetStringEvent>,
    /// The tag the entry now holds a reference on (the host adds one) and the one it let go of (the host releases one).
    pub acquired: Option<u32>,
    pub released: Option<u32>,
}

/// The sender's table: 32 entries and the LRU order (front = least recently used).
#[derive(Debug, Clone)]
pub struct SendStrings {
    entries: [SendEntry; NET_REMOTE_SLOTS],
    lru: VecDeque<u8>,
}

impl Default for SendStrings {
    fn default() -> SendStrings {
        SendStrings::new()
    }
}

impl SendStrings {
    /// A fresh table: every entry empty, the LRU list in index order (the constructor links entry 0 first and entry 31 last).
    pub fn new() -> SendStrings {
        SendStrings {
            entries: [SendEntry::default(); NET_REMOTE_SLOTS],
            lru: (0..NET_REMOTE_SLOTS as u8).collect(),
        }
    }

    /// The slot holding `tag`, if any (no LRU change).
    pub fn find(&self, tag: u32) -> Option<u8> {
        self.entries
            .iter()
            .position(|e| e.tag == Some(tag))
            .map(|i| i as u8)
    }

    /// The tag a slot holds.
    pub fn tag_of(&self, slot: u8) -> Option<u32> {
        self.entries.get(usize::from(slot)).and_then(|e| e.tag)
    }

    fn touch(&mut self, slot: u8) {
        if let Some(at) = self.lru.iter().position(|&s| s == slot) {
            self.lru.remove(at);
        }
        self.lru.push_back(slot);
    }

    /// `checkString`: a hit moves the entry to the most-recently-used end and reports its flag; a miss takes the LRU head, re-keys it
    /// to `tag` with the flag cleared, moves it to the MRU end and hands back the event to post. `text` is cut at 255 bytes.
    pub fn check_string(&mut self, tag: u32, text: &[u8]) -> Checked {
        if let Some(slot) = self.find(tag) {
            let acked = self.entries.get(usize::from(slot)).is_some_and(|e| e.acked);
            self.touch(slot);
            return Checked {
                slot,
                acked,
                event: None,
                acquired: None,
                released: None,
            };
        }
        let slot = self.lru.front().copied().unwrap_or(0);
        let mut released = None;
        if let Some(entry) = self.entries.get_mut(usize::from(slot)) {
            released = entry.tag.filter(|_| entry.owned);
            *entry = SendEntry {
                tag: Some(tag),
                acked: false,
                owned: true,
            };
        }
        self.touch(slot);
        let text = text.get(..text.len().min(NET_ARG_BYTES)).unwrap_or(&[]);
        Checked {
            slot,
            acked: false,
            event: Some(NetStringEvent {
                slot,
                text: text.to_vec(),
                tag,
            }),
            acquired: Some(tag),
            released,
        }
    }

    /// Says whether the entry in `slot` keyed to `tag` holds a reference on it (the host did not take one for a tag that did not exist;
    /// see `SendEntry`). A slot that has been re-keyed since is left alone.
    pub fn set_owned(&mut self, slot: u8, tag: u32, owned: bool) {
        if let Some(entry) = self.entries.get_mut(usize::from(slot))
            && entry.tag == Some(tag)
        {
            entry.owned = owned;
        }
    }

    /// `NetStringEvent::notifyDelivered(recved)`: sets the entry's flag if the slot still holds the same tag.
    pub fn delivered(&mut self, slot: u8, tag: u32) {
        if let Some(entry) = self.entries.get_mut(usize::from(slot))
            && entry.tag == Some(tag)
        {
            entry.acked = true;
        }
    }

    /// The connection is destroyed: every tag the table owns a reference on, for the host to release. The table is empty after.
    pub fn release_all(&mut self) -> Vec<u32> {
        let tags = self
            .entries
            .iter()
            .filter(|e| e.owned)
            .filter_map(|e| e.tag)
            .collect();
        *self = SendStrings::new();
        tags
    }
}

/// One receiver handle: the host's global tag id and its text (kept here so an expansion needs no VM).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    pub id: u32,
    pub text: Vec<u8>,
}

/// The receiver's 32 remote handles.
#[derive(Debug, Clone, Default)]
pub struct RemoteStrings {
    slots: [Option<Mapped>; NET_REMOTE_SLOTS],
}

impl RemoteStrings {
    pub fn new() -> RemoteStrings {
        RemoteStrings::default()
    }

    /// `NetStringEvent::process`: the slot now holds `id` / `text` (cut at 255 bytes). Returns the id it displaced, for the host to
    /// release.
    pub fn set(&mut self, slot: u8, id: u32, text: &[u8]) -> Option<u32> {
        let text = text.get(..text.len().min(NET_ARG_BYTES)).unwrap_or(&[]);
        let cell = self.slots.get_mut(usize::from(slot))?;
        cell.replace(Mapped {
            id,
            text: text.to_vec(),
        })
        .map(|old| old.id)
    }

    /// The handle at `slot`. An index outside 0..31 is unmapped.
    pub fn get(&self, slot: i64) -> Option<&Mapped> {
        usize::try_from(slot)
            .ok()
            .and_then(|i| self.slots.get(i))
            .and_then(Option::as_ref)
    }

    /// How many handles are mapped.
    pub fn mapped(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// The connection is destroyed: every id held, for the host to release (`removeString` per handle).
    pub fn release_all(&mut self) -> Vec<u32> {
        let ids = self.slots.iter().flatten().map(|m| m.id).collect();
        *self = RemoteStrings::new();
        ids
    }
}

/// A stand-in for the host's global tag table (`NetStringTable`): the same text is the same id, ids count from 1. The decoder and the
/// tests use it; the host uses the VM's own table.
#[derive(Debug, Clone, Default)]
pub struct LocalTags {
    ids: HashMap<Vec<u8>, u32>,
    texts: Vec<Vec<u8>>,
}

impl LocalTags {
    pub fn new() -> LocalTags {
        LocalTags::default()
    }

    /// `addTaggedString`.
    pub fn add(&mut self, text: &[u8]) -> u32 {
        if let Some(&id) = self.ids.get(text) {
            return id;
        }
        self.texts.push(text.to_vec());
        let id = self.texts.len() as u32;
        self.ids.insert(text.to_vec(), id);
        id
    }

    pub fn text(&self, id: u32) -> Option<&[u8]> {
        usize::try_from(id)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.texts.get(i))
            .map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests;
