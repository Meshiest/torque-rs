//! Scope control: a plugin keeps an object out of what a server ghosts to one connection, or to every connection, and lets it
//! back in ([`NetRegistry::scope_control`]).
//!
//! ```
//! use torque_engine::scope::{Audience, ScopeControl};
//! use torque_engine::{EngineBuilder, ServerStage};
//!
//! let mut builder = EngineBuilder::new();
//! builder.net().scope_control();
//! let engine = builder.build()?;
//! let mut vm = torque_ts_vm::vm::Vm::new();
//! engine.replay(ServerStage::Classes, &mut vm)?;
//! // Two connections (a server's are made by its network host).
//! vm.register_class("GameConnection", "SimObject", &[]);
//! let (a, b) = (
//!     vm.new_object(b"GameConnection", &[])?.expect("a connection"),
//!     vm.new_object(b"GameConnection", &[])?.expect("a connection"),
//! );
//!
//! // Object 40 is not ghosted to the connection a; every other connection still has it.
//! let scope = ScopeControl::of(&vm).expect("the plugin asked for scope control");
//! assert_eq!(scope.hide(&vm, 40, Audience::Connection(a)), Ok(true));
//! assert!(scope.is_hidden(40, Some(a)));
//! assert!(!scope.is_hidden(40, Some(b)));
//! // Hidden from everyone, then let back in for everyone.
//! assert_eq!(scope.hide(&vm, 40, Audience::Everyone), Ok(true));
//! assert!(scope.is_hidden(40, Some(b)));
//! assert!(scope.show(40, Audience::Everyone));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! **What it does.** A server asks the control before it makes a ghost of an object for a connection: a hidden object is
//! not scoped by any walk (the full query, an object's relatives, the feed of new objects), whether or not it is always in
//! scope (`ScopeAlways`, the ghost-always set) and whether or not a script scoped it (`scopeToClient`). An object that has a
//! ghost on the connection when it is hidden loses it with the connection's next packet, as if the object had been deleted
//! there: a kill record goes out at once (no update need be pending), and the ghosts the scene hierarchy hangs on it are sent
//! again without the parent they no longer have there (a child whose parent is not ghosted on a connection is that connection's
//! root, with its own pose). An object shown again is ghosted again by the next scan, an always-in-scope one included. The
//! ghosts of other connections are not touched by a hide for one connection.
//!
//! **Who may ask.** A hide is the server's own business: the control is on the server's script engine ([`ScopeControl::of`]),
//! and nothing a client sends can reach it. It only takes things away from what a client is sent, so a plugin that hides too
//! much leaves a client with less, never with more than the server's rules allow. It composes with every other rule of the
//! server's ghosting: whatever else would scope an object leaves it out while it is hidden.
//!
//! **Several plugins.** A plugin hides and shows under its own name ([`ScopeControl::hide_as`], [`ScopeControl::show_as`]; the
//! calls without a name are one anonymous user's), so one plugin's show never undoes another's hide: an object is hidden for a
//! connection while any user hides it. A plugin that sends an object some other way asks whether another user keeps it from a
//! connection ([`ScopeControl::is_hidden_by_other`]) and reads what changed since it last looked
//! ([`ScopeControl::changes_since`], a log of at most [`MAX_LOG`] entries; a cursor that fell behind it is told so and looks at
//! everything once). At most [`MAX_USERS`] names are kept.
//!
//! **Keeping it whole.** A connection is named by its `GameConnection` object. An object that is deleted, and a connection that
//! goes, take their entries with them (the server calls [`ScopeControl::forget_object`] and [`ScopeControl::sweep`]). At most
//! [`MAX_HIDDEN`] entries are held in all and [`MAX_PER_AUDIENCE`] for one connection (or for everyone), so one plugin's room
//! cannot take another connection's; a hide past a cap is refused ([`HideError::Full`], counted in [`ScopeControl::refused`])
//! and the object stays as it was, and a hide for an object that is no `GameConnection` is refused
//! ([`HideError::NotAConnection`]).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::rc::Rc;

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::{NetRegistry, ServerStage};

/// The most hidden entries (an object hidden from a connection, or from everyone) a control holds in all.
pub const MAX_HIDDEN: usize = 1 << 18;

/// The most entries one audience (one connection, or everyone) may have: a plugin that hoards one audience's room cannot take
/// the others' of the pool.
pub const MAX_PER_AUDIENCE: usize = 1 << 14;

/// The class a connection named by [`Audience::Connection`] is of.
pub const CONNECTION_CLASS: &str = "GameConnection";

/// The most plugins (names) that hide objects through one control.
pub const MAX_USERS: usize = 31;

/// The most changes the control remembers for a reader ([`ScopeControl::changes_since`]).
pub const MAX_LOG: usize = 1 << 16;

/// The name hides are made under when no plugin gives its own ([`ScopeControl::hide`]).
pub const ANONYMOUS: &str = "";

/// Who an object is hidden from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Audience {
    /// Every connection.
    Everyone,
    /// One connection, named by its `GameConnection` object.
    Connection(ObjId),
}

/// Why a hide was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HideError {
    /// The audience has [`MAX_PER_AUDIENCE`] entries already, or the control [`MAX_HIDDEN`] in all, or [`MAX_USERS`] plugins hide.
    Full,
    /// The audience names an object that is no `GameConnection`.
    NotAConnection(ObjId),
}

impl std::fmt::Display for HideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HideError::Full => write!(
                f,
                "the scope control holds {MAX_HIDDEN} hidden objects, or {MAX_PER_AUDIENCE} for one audience, already"
            ),
            HideError::NotAConnection(o) => write!(f, "object {o} is no {CONNECTION_CLASS}"),
        }
    }
}

impl std::error::Error for HideError {}

/// A change of what is hidden, for a reader that looks at what concerns it ([`ScopeControl::changes_since`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    /// The object hidden or shown.
    pub obj: ObjId,
    /// From whom.
    pub audience: Audience,
    /// The bit of the plugin whose hide was made or lifted.
    user: u32,
}

#[derive(Debug, Default)]
struct Hidden {
    /// Who hides each object from everyone, and from each connection: a bit for each plugin that does.
    all: HashMap<ObjId, u32>,
    per_connection: BTreeMap<ObjId, HashMap<ObjId, u32>>,
    /// The names of the plugins that hide, in the order their bits were given.
    users: Vec<&'static str>,
    /// Entries held in all.
    count: usize,
    /// The last change of what is hidden from everyone, and of what is hidden from each connection (a value of `changes`, so a
    /// connection's stamp only moves when its own answers may have).
    all_stamp: u64,
    conn_stamps: BTreeMap<ObjId, u64>,
    /// Counts the changes.
    changes: u64,
    /// The last [`MAX_LOG`] changes, and the place of the first.
    log: VecDeque<Change>,
    log_base: u64,
}

impl Hidden {
    /// Notes that what `audience` is hidden from changed.
    fn touch(&mut self, audience: Audience, obj: ObjId, user: u32) {
        self.changes += 1;
        match audience {
            Audience::Everyone => self.all_stamp = self.changes,
            Audience::Connection(c) => {
                self.conn_stamps.insert(c, self.changes);
            }
        }
        if self.log.len() >= MAX_LOG {
            self.log.pop_front();
            self.log_base += 1;
        }
        self.log.push_back(Change {
            obj,
            audience,
            user,
        });
    }

    /// The bit of the plugin `name`, given one if it has none (none when [`MAX_USERS`] others hide).
    fn bit_of(&mut self, name: &'static str) -> Option<u32> {
        if let Some(at) = self.users.iter().position(|&u| u == name) {
            return Some(1 << at);
        }
        (self.users.len() < MAX_USERS).then(|| {
            self.users.push(name);
            1 << (self.users.len() - 1)
        })
    }

    fn who(&self, obj: ObjId, audience: Audience) -> u32 {
        match audience {
            Audience::Everyone => self.all.get(&obj).copied(),
            Audience::Connection(c) => self
                .per_connection
                .get(&c)
                .and_then(|set| set.get(&obj).copied()),
        }
        .unwrap_or(0)
    }
}

/// What a server's plugins hide from its connections ([module docs](self)): a server's script engine has it as its extension when a
/// plugin asked for it ([`NetRegistry::scope_control`]).
#[derive(Debug, Default)]
pub struct ScopeControl {
    hidden: RefCell<Hidden>,
    /// The hides refused so far, for a plugin or a log to see that its room is gone.
    refused: Cell<u64>,
}

impl ScopeControl {
    /// The control of `vm`'s server, when a plugin asked for one.
    pub fn of(vm: &Vm) -> Option<Rc<ScopeControl>> {
        vm.extension::<ScopeControl>()
    }

    /// [`ScopeControl::hide_as`] under no plugin's name ([`ANONYMOUS`]).
    ///
    /// # Errors
    ///
    /// As [`ScopeControl::hide_as`].
    pub fn hide(&self, vm: &Vm, obj: ObjId, audience: Audience) -> Result<bool, HideError> {
        self.hide_as(vm, ANONYMOUS, obj, audience)
    }

    /// Keeps `obj` out of the ghosts sent to `audience`, as the plugin `user` (an object stays hidden while any plugin hides
    /// it, and each can tell its own hides from the others': [`ScopeControl::is_hidden_by_other`]). `Ok(true)` when that
    /// changed something, `Ok(false)` when this plugin hid it from them already. `vm` is the server's script engine, which
    /// says whether a connection is one.
    ///
    /// # Errors
    ///
    /// [`HideError::NotAConnection`] when `audience` names an object that is no `GameConnection`, [`HideError::Full`] when its room
    /// is gone (counted in [`ScopeControl::refused`]); nothing is changed.
    pub fn hide_as(
        &self,
        vm: &Vm,
        user: &'static str,
        obj: ObjId,
        audience: Audience,
    ) -> Result<bool, HideError> {
        if let Audience::Connection(c) = audience
            && !is_connection(vm, c)
        {
            return Err(HideError::NotAConnection(c));
        }
        let mut h = self.hidden.borrow_mut();
        let Some(bit) = h.bit_of(user) else {
            self.refused.set(self.refused.get() + 1);
            return Err(HideError::Full);
        };
        let held = h.who(obj, audience);
        if held & bit != 0 {
            return Ok(false);
        }
        if held == 0 {
            let room = match audience {
                Audience::Everyone => h.all.len(),
                Audience::Connection(c) => h.per_connection.get(&c).map_or(0, HashMap::len),
            };
            if h.count >= MAX_HIDDEN || room >= MAX_PER_AUDIENCE {
                self.refused.set(self.refused.get() + 1);
                return Err(HideError::Full);
            }
            h.count += 1;
        }
        match audience {
            Audience::Everyone => *h.all.entry(obj).or_insert(0) |= bit,
            Audience::Connection(c) => {
                *h.per_connection
                    .entry(c)
                    .or_default()
                    .entry(obj)
                    .or_insert(0) |= bit;
            }
        }
        h.touch(audience, obj, bit);
        Ok(true)
    }

    /// [`ScopeControl::show_as`] under no plugin's name.
    pub fn show(&self, obj: ObjId, audience: Audience) -> bool {
        self.show_as(ANONYMOUS, obj, audience)
    }

    /// Lets `obj` back into the ghosts sent to `audience` as far as the plugin `user` is concerned (it stays hidden from whoever
    /// else hides it, and from whoever else it was hidden from). True when this plugin hid it from them.
    pub fn show_as(&self, user: &'static str, obj: ObjId, audience: Audience) -> bool {
        let mut h = self.hidden.borrow_mut();
        let Some(bit) = h.users.iter().position(|&u| u == user).map(|at| 1u32 << at) else {
            return false;
        };
        let held = h.who(obj, audience);
        if held & bit == 0 {
            return false;
        }
        let left = held & !bit;
        match audience {
            Audience::Everyone => {
                if left == 0 {
                    h.all.remove(&obj);
                } else {
                    h.all.insert(obj, left);
                }
            }
            Audience::Connection(c) => {
                if let Some(set) = h.per_connection.get_mut(&c) {
                    if left == 0 {
                        set.remove(&obj);
                    } else {
                        set.insert(obj, left);
                    }
                    if set.is_empty() {
                        h.per_connection.remove(&c);
                    }
                }
            }
        }
        if left == 0 {
            h.count -= 1;
        }
        h.touch(audience, obj, bit);
        true
    }

    /// Whether `obj` is kept out of the ghosts of the connection whose `GameConnection` object is `connection` (none: a source
    /// made for no connection, which only the hides from everyone reach).
    pub fn is_hidden(&self, obj: ObjId, connection: Option<ObjId>) -> bool {
        self.hidden_by(obj, connection, u32::MAX) != 0
    }

    /// Whether a plugin other than `user` keeps `obj` out of the ghosts of `connection`.
    pub fn is_hidden_by_other(
        &self,
        user: &'static str,
        obj: ObjId,
        connection: Option<ObjId>,
    ) -> bool {
        let own = {
            let h = self.hidden.borrow();
            h.users
                .iter()
                .position(|&u| u == user)
                .map_or(0, |at| 1u32 << at)
        };
        self.hidden_by(obj, connection, !own) != 0
    }

    /// The bits of the plugins in `mask` that hide `obj` from `connection`.
    fn hidden_by(&self, obj: ObjId, connection: Option<ObjId>, mask: u32) -> u32 {
        let h = self.hidden.borrow();
        if h.count == 0 {
            return 0;
        }
        let everyone = h.all.get(&obj).copied().unwrap_or(0);
        let own = connection
            .and_then(|c| h.per_connection.get(&c))
            .and_then(|set| set.get(&obj).copied())
            .unwrap_or(0);
        (everyone | own) & mask
    }

    /// How many objects are hidden from `audience` ([`Audience::Connection`] counts only what is hidden from that connection alone).
    pub fn hidden_count(&self, audience: Audience) -> usize {
        let h = self.hidden.borrow();
        match audience {
            Audience::Everyone => h.all.len(),
            Audience::Connection(c) => h.per_connection.get(&c).map_or(0, HashMap::len),
        }
    }

    /// The hides refused so far for want of room.
    pub fn refused(&self) -> u64 {
        self.refused.get()
    }

    /// A number that moves whenever an answer of [`ScopeControl::is_hidden`] for `connection` may have changed, and not for a change
    /// that only concerns another connection (it only grows).
    pub fn stamp_for(&self, connection: Option<ObjId>) -> u64 {
        let h = self.hidden.borrow();
        let own = connection
            .and_then(|c| h.conn_stamps.get(&c))
            .copied()
            .unwrap_or(0);
        h.all_stamp.max(own)
    }

    /// Where the next change will be in the log, for a reader to start from.
    pub fn log_end(&self) -> u64 {
        let h = self.hidden.borrow();
        h.log_base + h.log.len() as u64
    }

    /// Adds to `out` what plugins other than `user` hid or showed since `cursor` (an object and audience once for each change), and
    /// gives the place to read from next. `Err` with that place when changes since `cursor` were forgotten (a reader far behind):
    /// the reader looks at everything again.
    ///
    /// # Errors
    ///
    /// The changes were forgotten; the new place is the error.
    pub fn changes_since(
        &self,
        user: &'static str,
        cursor: u64,
        out: &mut Vec<(ObjId, Audience)>,
    ) -> Result<u64, u64> {
        let h = self.hidden.borrow();
        let end = h.log_base + h.log.len() as u64;
        if cursor < h.log_base {
            return Err(end);
        }
        let own = h
            .users
            .iter()
            .position(|&u| u == user)
            .map_or(0, |at| 1u32 << at);
        out.extend(
            h.log
                .iter()
                .skip((cursor - h.log_base) as usize)
                .filter(|c| c.user == 0 || c.user != own)
                .map(|c| (c.obj, c.audience)),
        );
        Ok(end)
    }

    /// `obj` is gone: every entry for it is dropped.
    pub fn forget_object(&self, obj: ObjId) {
        let mut h = self.hidden.borrow_mut();
        if h.count == 0 {
            return;
        }
        if h.all.remove(&obj).is_some() {
            h.count -= 1;
            h.touch(Audience::Everyone, obj, 0);
        }
        let holding: Vec<ObjId> = h
            .per_connection
            .iter()
            .filter(|(_, set)| set.contains_key(&obj))
            .map(|(&c, _)| c)
            .collect();
        for c in holding {
            if let Some(set) = h.per_connection.get_mut(&c) {
                set.remove(&obj);
                if set.is_empty() {
                    h.per_connection.remove(&c);
                }
            }
            h.count -= 1;
            h.touch(Audience::Connection(c), obj, 0);
        }
    }

    /// Drops what is hidden from connections that are no longer objects of `vm`.
    pub fn sweep(&self, vm: &Vm) {
        let mut h = self.hidden.borrow_mut();
        let objects = vm.objects();
        let gone: Vec<ObjId> = h
            .conn_stamps
            .keys()
            .copied()
            .filter(|c| !objects.contains(*c))
            .collect();
        for c in gone {
            h.conn_stamps.remove(&c);
            if let Some(set) = h.per_connection.remove(&c) {
                h.count -= set.len();
            }
        }
    }
}

/// Whether `obj` is a `GameConnection` of `vm`.
fn is_connection(vm: &Vm, obj: ObjId) -> bool {
    let objects = vm.objects();
    objects
        .get(obj)
        .is_some_and(|o| objects.classes.name(o.class()) == CONNECTION_CLASS.as_bytes())
}

/// The marker that the recipe installing a control into each server is recorded.
struct ScopeInstalls;

impl NetRegistry<'_> {
    /// Gives each server's script engine a [`ScopeControl`] ([module docs](crate::scope)): a plugin keeps objects out of a
    /// connection's ghosts, or everyone's, and lets them back in. Calling it again, from this plugin or another, changes
    /// nothing. Only a server has one, and what it keeps out only matters to the connections that are ghosted objects at all
    /// (the engine's own clients and Torque's alike).
    pub fn scope_control(&mut self) -> &mut Self {
        if self.engine.resource::<ScopeInstalls>().is_none() {
            self.engine.insert_resource(ScopeInstalls);
            self.engine.server(ServerStage::Classes, |vm, _| {
                vm.set_extension(Rc::new(ScopeControl::default()));
                Ok(())
            });
        }
        self
    }
}

#[cfg(test)]
#[path = "scope_tests.rs"]
mod tests;
