//! The server process list (`ProcessList`): the order the objects tick in, and the way a pass walks it.
//!
//! | piece |
//! |---|
//! | `GameBase::onAdd` appends the object at the TAIL, once its datablock is taken (`onNewDataBlock`) |
//! | `GameBase::onRemove` unlinks it |
//! | `GameBase::processAfter(target)`: the object ticks after `target`; a target that follows the object is let go; the list is dirty |
//! | `orderList`, run at the start of a pass only when the list is dirty |
//! | `advanceObjects` (one 32 ms step) |
//!
//! **`orderList`.** The objects are visited in list order. Each visited object is tagged; while the object that it follows is neither
//! tagged nor missing, that one is tagged and pulled in front of the chain too (so a chain `a follows b follows c` is laid out `c b a`);
//! the chain goes at the end of the new list. Objects that were pulled are not visited again; a target that is already tagged (placed
//! earlier, or part of a loop) ends the chain. Clearing an edge (the Camera's orbit) does not make the list dirty.
//!
//! **`advanceObjects`.** The whole list moves to a temporary list. The first object of it is taken off, linked at the TAIL of the real
//! list, and then ticked if it is not deleted and its `mProcessTick` flag is set. So an object made during a tick is linked before the
//! ones not visited yet, and from the next pass on it sits ahead of them; it is not ticked in the pass that made it. An object removed
//! during the pass is gone from whichever list it was on and is not ticked. [`ProcessList::begin_pass`] and
//! [`ProcessList::next_in_pass`] are that walk; the host does the ticking between them.
//!
//! The list is keyed by object id and every node has a serial, so an id that is reused after its object left is a new node: a stale
//! entry of the temporary list, or a stale `after` edge, is recognised by its serial. An edge to an object that is not in the list
//! is ignored (no such object can be named by script).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/gameBase/processList.cpp`,
//! `T3D/gameBase/gameBase.cpp` (each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.) and
//! `T3D/gameBase/gameProcess.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ObjectKey;

struct Node<T> {
    serial: u64,
    prev: Option<ObjectKey>,
    next: Option<ObjectKey>,
    /// On the real list (not on the temporary one of a pass).
    linked: bool,
    after: Option<(ObjectKey, u64)>,
    /// The `mProcessTick` flag, true from the constructor.
    ticks: bool,
    class: &'static str,
    /// Whatever the host ticks with; taken out while the object is being ticked.
    payload: Option<T>,
}

/// The process list. `T` is what the host ticks an object with (a boxed [`Ticking`](crate::Ticking)).
pub struct ProcessList<T> {
    nodes: HashMap<ObjectKey, Node<T>>,
    first: Option<ObjectKey>,
    last: Option<ObjectKey>,
    /// The objects of a pass that were not visited yet, with their serials.
    temp: VecDeque<(ObjectKey, u64)>,
    next_serial: u64,
    dirty: bool,
}

impl<T> Default for ProcessList<T> {
    fn default() -> Self {
        ProcessList {
            nodes: HashMap::new(),
            first: None,
            last: None,
            temp: VecDeque::new(),
            next_serial: 1,
            dirty: false,
        }
    }
}

/// What [`ProcessList::next_in_pass`] hands out.
pub struct Visit<T> {
    pub key: ObjectKey,
    /// Give this back to [`ProcessList::restore`] with the payload.
    pub serial: u64,
    pub class: &'static str,
    /// `None` when the payload is out already (the object is being ticked further up).
    pub payload: Option<T>,
    /// The `mProcessTick` flag: when clear, `processTick(NULL)` is not called, but a connection's control object still runs its moves
    /// (the control branch does not read the flag).
    pub ticks: bool,
}

impl<T> ProcessList<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Objects in the list (the temporary list of a pass included).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn contains(&self, key: ObjectKey) -> bool {
        self.nodes.contains_key(&key)
    }

    pub fn class_of(&self, key: ObjectKey) -> Option<&'static str> {
        self.nodes.get(&key).map(|n| n.class)
    }

    /// Whether the next pass re-sorts first.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// `GameBase::onAdd`: the object enters at the tail. `after` is the edge its class asks for at once
    /// ([`Ticking::process_after`](crate::Ticking::process_after)). False, and nothing changes, when the key is in the list already.
    pub fn add(
        &mut self,
        key: ObjectKey,
        class: &'static str,
        payload: T,
        after: Option<ObjectKey>,
    ) -> bool {
        if self.nodes.contains_key(&key) {
            return false;
        }
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1);
        self.nodes.insert(
            key,
            Node {
                serial,
                prev: None,
                next: None,
                linked: false,
                after: None,
                ticks: true,
                class,
                payload: Some(payload),
            },
        );
        self.link_tail(key);
        if let Some(target) = after {
            self.process_after(key, Some(target));
        }
        true
    }

    /// `GameBase::onRemove`: the object leaves, from whichever list it is on. Returns the payload if it was in.
    pub fn remove(&mut self, key: ObjectKey) -> Option<T> {
        self.unlink(key);
        self.nodes.remove(&key).and_then(|n| n.payload)
    }

    /// The `mProcessTick` flag: an object whose flag is clear stays in the list and is not ticked.
    pub fn set_ticks(&mut self, key: ObjectKey, ticks: bool) {
        if let Some(n) = self.nodes.get_mut(&key) {
            n.ticks = ticks;
        }
    }

    /// `GameBase::processAfter`: `key` ticks after `target` (or after nothing). A target that follows `key` is let go (a loop of
    /// two). The list is dirty, whatever changed.
    pub fn process_after(&mut self, key: ObjectKey, target: Option<ObjectKey>) {
        if !self.nodes.contains_key(&key) {
            return;
        }
        let edge = target.and_then(|t| self.nodes.get(&t).map(|n| (t, n.serial)));
        if let Some(n) = self.nodes.get_mut(&key) {
            n.after = edge;
        }
        if let Some((t, _)) = edge
            && let Some(other) = self.nodes.get_mut(&t)
            && other.after.is_some_and(|(k, _)| k == key)
        {
            other.after = None;
        }
        self.dirty = true;
    }

    /// The edge let go without a dirty list (`Camera`'s orbit before it takes the next object).
    pub fn clear_after(&mut self, key: ObjectKey) {
        if let Some(n) = self.nodes.get_mut(&key) {
            n.after = None;
        }
    }

    /// The object `key` follows, if the edge is still good.
    pub fn after_of(&self, key: ObjectKey) -> Option<ObjectKey> {
        self.valid_after(key)
    }

    fn valid_after(&self, key: ObjectKey) -> Option<ObjectKey> {
        let (target, serial) = self.nodes.get(&key)?.after?;
        let t = self.nodes.get(&target)?;
        (t.serial == serial).then_some(target)
    }

    /// The keys in list order (the real list; during a pass, the objects visited so far and the ones made since).
    pub fn order(&self) -> Vec<ObjectKey> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut at = self.first;
        while let Some(k) = at {
            out.push(k);
            at = self.nodes.get(&k).and_then(|n| n.next);
            if out.len() > self.nodes.len() {
                break;
            }
        }
        out
    }

    fn link_tail(&mut self, key: ObjectKey) {
        let last = self.last;
        if let Some(n) = self.nodes.get_mut(&key) {
            n.prev = last;
            n.next = None;
            n.linked = true;
        } else {
            return;
        }
        match last {
            Some(l) => {
                if let Some(ln) = self.nodes.get_mut(&l) {
                    ln.next = Some(key);
                }
            }
            None => self.first = Some(key),
        }
        self.last = Some(key);
    }

    fn unlink(&mut self, key: ObjectKey) {
        let Some(n) = self.nodes.get_mut(&key) else {
            return;
        };
        if !n.linked {
            return;
        }
        n.linked = false;
        let (prev, next) = (n.prev.take(), n.next.take());
        match prev {
            Some(p) => {
                if let Some(pn) = self.nodes.get_mut(&p) {
                    pn.next = next;
                }
            }
            None => self.first = next,
        }
        match next {
            Some(x) => {
                if let Some(xn) = self.nodes.get_mut(&x) {
                    xn.prev = prev;
                }
            }
            None => self.last = prev,
        }
    }

    /// `orderList`: see the module documentation.
    pub fn order_list(&mut self) {
        self.finish_pass();
        let old = self.order();
        let mut seen: HashSet<ObjectKey> = HashSet::with_capacity(old.len());
        let mut fresh: Vec<ObjectKey> = Vec::with_capacity(old.len());
        for key in old {
            if !seen.insert(key) {
                continue;
            }
            let mut chain = vec![key];
            let mut at = key;
            while let Some(target) = self.valid_after(at) {
                if !seen.insert(target) {
                    break;
                }
                chain.push(target);
                at = target;
            }
            fresh.extend(chain.into_iter().rev());
        }
        self.first = None;
        self.last = None;
        for n in self.nodes.values_mut() {
            n.linked = false;
        }
        for key in fresh {
            self.link_tail(key);
        }
        self.dirty = false;
    }

    /// `orderList` when the list is dirty (what the start of a pass does).
    pub fn order_list_if_dirty(&mut self) {
        if self.dirty {
            self.order_list();
        }
    }

    /// The start of one 32 ms step: the list moves to a temporary one. A pass sorts a dirty list before its steps
    /// ([`order_list_if_dirty`](Self::order_list_if_dirty)), not between them.
    pub fn begin_pass(&mut self) {
        self.finish_pass();
        for key in self.order() {
            if let Some(n) = self.nodes.get(&key) {
                self.temp.push_back((key, n.serial));
            }
        }
        self.first = None;
        self.last = None;
        for n in self.nodes.values_mut() {
            n.linked = false;
        }
    }

    /// The next object of the pass: taken off the temporary list and linked at the tail, skipped if it left. One whose `mProcessTick`
    /// is clear is handed out too, with [`Visit::ticks`] false: the flag is tested only on its `processTick(NULL)` branch, after
    /// the control branch (a control object runs its moves whatever the flag says). None when the pass is done.
    pub fn next_in_pass(&mut self) -> Option<Visit<T>> {
        while let Some((key, serial)) = self.temp.pop_front() {
            match self.nodes.get(&key) {
                Some(n) if n.serial == serial && !n.linked => {}
                _ => continue,
            }
            self.link_tail(key);
            let n = self.nodes.get_mut(&key)?;
            return Some(Visit {
                key,
                serial,
                class: n.class,
                payload: n.payload.take(),
                ticks: n.ticks,
            });
        }
        None
    }

    /// Puts a payload back after its tick, if the object is still the same one.
    pub fn restore(&mut self, key: ObjectKey, serial: u64, payload: T) {
        if let Some(n) = self.nodes.get_mut(&key)
            && n.serial == serial
        {
            n.payload = Some(payload);
        }
    }

    /// Links whatever a pass left on the temporary list (a pass that ended early), in its order.
    pub fn finish_pass(&mut self) {
        while let Some((key, serial)) = self.temp.pop_front() {
            if self
                .nodes
                .get(&key)
                .is_some_and(|n| n.serial == serial && !n.linked)
            {
                self.link_tail(key);
            }
        }
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
