//! The ghost manager of one server-side connection: scope, the ghost section of a data packet, notifies, `activateGhosting` /
//! `resetGhosting` and the ghost `ConnectionMessage`s.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `sim/netGhost.cpp` (Copyright (c) 2012
//! GarageGames, LLC) and `sim/netObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the
//! code here is this crate's own.

use std::collections::BTreeMap;

use crate::bits::{BitWriter, bit_length};
use crate::cap::{NET_GHOST_DEPENDENTS, NET_GHOST_LIMIT, NET_GHOST_REFS};
use crate::classes::ghost_id_bits;
use crate::events::{ConnectionMessage, EventCtx, EventIds, PackCtx, WireEvent};

use super::policy::{GhostPolicy, ObjectPolicy, PendingOrder, TableCounts, TorqueGhosts};
use super::source::{
    CamInfo, GhostScripts, ObjId, ObjectSource, flags, net_flags, scope_radius_used,
};
use super::table::Table;

/// Priority of a ghost whose kill record is due.
pub const KILL_PRIORITY: f32 = 10_000.0;

/// The least a ghosting section takes: `flag(1)`, the 5-bit id size and the closing `flag(0)`. A packet writer keeps this much free for
/// the section after its events.
pub const SECTION_MIN_BITS: usize = 7;

/// A `GhostRef` flag: the record carried the class form (a NewGhost).
const REF_NEW_GHOST: u8 = 0x10;
/// A `GhostRef` flag: the record was a kill.
const REF_KILLING: u8 = 0x40;

/// Per-connection settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhostConfig {
    /// The ghost limit ([`crate::profile::DEFAULT_GHOST_LIMIT`] by default); at most [`NET_GHOST_LIMIT`].
    pub max_ghosts: u32,
    /// A loopback connection (the only one that scopes `ScopeLocal` objects).
    pub local: bool,
    /// The width of a class id in a ghost record (`NetClassBitSize` of the object group, 6).
    pub class_bits: u8,
}

impl GhostConfig {
    pub fn standard() -> GhostConfig {
        GhostConfig {
            max_ghosts: crate::profile::DEFAULT_GHOST_LIMIT,
            local: false,
            class_bits: 6,
        }
    }

    /// The width of a ghost index in events: [`ghost_id_bits`] of the limit.
    pub fn ghost_bits(&self) -> u8 {
        ghost_id_bits(self.max_ghosts)
    }
}

/// Why a ghost operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhostError {
    /// `activateGhosting` twice without a reset.
    AlreadyActive,
    /// `GhostAlwaysSet` is larger than the ghost limit.
    TooManyGhostAlways { count: usize, max: u32 },
    /// The configured ghost limit is zero or above [`NET_GHOST_LIMIT`].
    BadLimit(u32),
    /// A client `ConnectionMessage` 0, 2 or 3 reached a server: the connection error is set to
    /// `Invalid packet.(ghost bad connection message state)`.
    BadMessageState,
}

impl GhostError {
    /// The text for `GameConnection::setLastError` where it has one.
    pub fn message(&self) -> &'static str {
        match self {
            GhostError::BadMessageState => "Invalid packet.(ghost bad connection message state)",
            GhostError::AlreadyActive => "ghosting is already active",
            GhostError::TooManyGhostAlways { .. } => "too many ghost-always objects",
            GhostError::BadLimit(_) => "bad ghost limit",
        }
    }
}

impl std::fmt::Display for GhostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for GhostError {}

/// What a `ConnectionMessage` did to the ghost layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageOutcome {
    /// Message 1 with the current ghosting sequence: ghosting is on.
    Ready,
    /// Message 1 with another sequence: nothing happens (no error).
    Ignored,
    /// Message 4 or 5 (or above): not a ghost message.
    NotGhost,
}

/// An event the ghost layer posts. The host wraps it in its event enum (or posts it through `EventQueue<GhostEvent>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhostEvent {
    Message(ConnectionMessage),
    /// `GhostAlwaysObjectEvent`: the object's full state, packed when the event is WRITTEN.
    AlwaysObject {
        obj: ObjId,
        index: u32,
        /// The ghosting sequence the event was made for.
        seq: u32,
    },
}

impl WireEvent for GhostEvent {
    fn class(&self, ids: &EventIds) -> u32 {
        match self {
            GhostEvent::Message(_) => ids.connection_message,
            GhostEvent::AlwaysObject { .. } => ids.ghost_always_object,
        }
    }

    /// **Not for the plain section writer**: a ghost-always event must be written with [`EventQueue::write_section_with`] and a
    /// [`GhostManager::pack_ctx`] (the record is packed when the event is written). Without objects this writes the empty form and
    /// a debug build asserts, so the mistake shows in tests instead of shipping empty events.
    ///
    /// [`EventQueue::write_section_with`]: crate::events::EventQueue::write_section_with
    fn pack(&self, w: &mut BitWriter, ctx: &EventCtx<'_>) {
        match self {
            GhostEvent::Message(m) => m.pack(w, ctx),
            GhostEvent::AlwaysObject { index, .. } => {
                debug_assert!(false, "GhostAlwaysObjectEvent packed without a PackCtx");
                w.write_bits(*index, ctx.ghost_bits);
                w.write_flag(false);
            }
        }
    }

    fn pack_with(&self, w: &mut BitWriter, ctx: &EventCtx<'_>, objs: &mut dyn PackCtx) {
        match self {
            GhostEvent::Message(m) => m.pack(w, ctx),
            GhostEvent::AlwaysObject { obj, index, seq } => {
                w.write_bits(*index, ctx.ghost_bits);
                objs.pack_ghost_always(*obj, *seq, w);
            }
        }
    }
}

/// The [`PackCtx`] of a connection: what `GhostAlwaysObjectEvent::pack` reaches. Made by [`GhostManager::pack_ctx`].
pub struct GhostPackCtx<'a, S: ObjectSource> {
    src: &'a mut S,
    table: &'a Table,
    sequence: u32,
    class_bits: u8,
    left: Vec<(ObjId, u32)>,
}

impl<S: ObjectSource> GhostPackCtx<'_, S> {
    /// The mask bits each ghost-always pack returned (a part it could not send yet, such as a mount on an object the connection
    /// has no ghost of). The host gives them back to the ghosts with [`GhostManager::set_mask`] once the section is written.
    pub fn into_left(self) -> Vec<(ObjId, u32)> {
        self.left
    }
}

/// `getGhostIndex` over a table.
fn index_in(table: &Table, obj: ObjId) -> Option<u32> {
    let id = table.id_of(obj)?;
    let g = table.get(id)?;
    (g.flags & 0x78 == 0).then_some(g.index)
}

impl<S: ObjectSource> PackCtx for GhostPackCtx<'_, S> {
    /// `flag(object exists now)`, then `int(6) classId` (never the implied form) and `packUpdate(conn, 0xFFFFFFFF)`. **Addition:**
    /// an event made for an earlier ghosting sequence (a reset came between) packs the empty form (a standard event carries no sequence
    /// and would pack whatever object has the id with the stale index).
    ///
    /// **Addition:** the mask `packUpdate` returns is kept ([`GhostPackCtx::into_left`]) where the event drops it, so a part
    /// that waited for another object's ghost is sent by a later update.
    fn pack_ghost_always(&mut self, obj: u32, seq: u32, w: &mut BitWriter) {
        let class = if seq == self.sequence && self.src.exists(obj) {
            self.src.class_id(obj)
        } else {
            None
        };
        match class {
            Some(class) => {
                w.write_flag(true);
                w.write_bits(class, self.class_bits);
                let table = self.table;
                let left = self
                    .src
                    .pack_update(obj, 0xFFFF_FFFF, w, &|t| index_in(table, t));
                if left != 0 {
                    self.left.push((obj, left));
                }
            }
            None => w.write_flag(false),
        }
    }

    fn ghost_index(&mut self, obj: u32) -> Option<u32> {
        if self.src.exists(obj) {
            index_in(self.table, obj)
        } else {
            None
        }
    }
}

/// What one ghost section did (for tests and the host's counters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SectionStats {
    pub records: usize,
    pub kills: usize,
    /// Records rolled back because they did not fit the stream.
    pub refused: usize,
    pub full_query: bool,
    /// The work budget ran out inside the section.
    pub out_of_budget: bool,
    /// The section was `flag(0)` alone (not ghosting).
    pub idle: bool,
    /// Updates passed over (left dirty for a later packet) because their share group of the packet was spent
    /// ([`ObjectSource::packet_share`]).
    pub share_deferred: usize,
    /// The most bits one share group's updates took in this section (0 when no object has a share).
    pub share_bits: usize,
}

/// The bits each share group's updates took in one section ([`ObjectSource::packet_share`]), and whether the group is closed
/// (an update of it was rolled back for passing its share: the rest of the group waits for the next packet without being
/// packed). A few groups at most, so a list.
#[derive(Debug, Default)]
struct ShareSpend(Vec<(u32, usize, bool)>);

impl ShareSpend {
    fn entry(&mut self, group: u32) -> &mut (u32, usize, bool) {
        let at = match self.0.iter().position(|e| e.0 == group) {
            Some(at) => at,
            None => {
                self.0.push((group, 0, false));
                self.0.len() - 1
            }
        };
        &mut self.0[at]
    }

    /// Whether the group may write another update under `cap`.
    fn open(&self, group: u32, cap: usize) -> bool {
        self.0
            .iter()
            .find(|e| e.0 == group)
            .is_none_or(|e| !e.2 && e.1 < cap)
    }

    fn spent(&self, group: u32) -> usize {
        self.0.iter().find(|e| e.0 == group).map_or(0, |e| e.1)
    }

    fn add(&mut self, group: u32, bits: usize) {
        self.entry(group).1 += bits;
    }

    fn close(&mut self, group: u32) {
        self.entry(group).2 = true;
    }

    fn most(&self) -> usize {
        self.0.iter().map(|e| e.1).max().unwrap_or(0)
    }
}

/// The highest priority an update with a packet share is given (a plugin's priority above it is held to it): below half of
/// [`KILL_PRIORITY`], so a ghost's kill always goes before any plugin's update.
pub const SHARE_PRIORITY_MAX: f32 = KILL_PRIORITY * 0.5 - 2.0;

/// The priority of a share-group object's first record: above every update of the group (an object a client has never been
/// sent is the largest error there is), still below a kill.
pub const SHARE_FIRST_PRIORITY: f32 = SHARE_PRIORITY_MAX + 1.0;

/// A share-group update's priority, held to `-SHARE_PRIORITY_MAX..=SHARE_PRIORITY_MAX` (a NaN is 0).
pub fn share_priority(p: f32) -> f32 {
    if p.is_nan() {
        0.0
    } else {
        p.clamp(-SHARE_PRIORITY_MAX, SHARE_PRIORITY_MAX)
    }
}

/// The bits of a packet of `packet_bits` a share of `part` (0 to 1; anything else is held to that range, a NaN is 0) leaves its
/// group.
pub fn share_cap(packet_bits: usize, part: f32) -> usize {
    let part = if part.is_nan() {
        0.0
    } else {
        part.clamp(0.0, 1.0)
    };
    (packet_bits as f64 * f64::from(part)) as usize
}

/// A read-only view of one ghost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhostView {
    pub index: u32,
    pub obj: Option<ObjId>,
    pub mask: u32,
    pub flags: u16,
    pub pos: usize,
    pub skip: u8,
}

#[derive(Debug, Clone, Copy)]
struct GhostRef {
    ghost: u32,
    generation: u64,
    mask: u32,
    flags: u8,
}

/// The two work budgets of a ghost section, in units (1 per object a scan visits or a ghost the priority pass reads, `1 +
/// bits / 32` per record; see [`crate::cap::NET_GHOST_WORK_PER_PACKET`]). Each closure is told the units it is asked to pay and
/// answers whether it can. They are separate so that a scan larger than the budget (which resumes with the next packet) can never
/// starve the records: **the records always get their own budget.**
pub struct SectionBudget<'a> {
    /// The scope work: the full query, the recent-list walk, the dependents, the `skipCount` pass and the priority reads.
    pub scan: &'a mut dyn FnMut(u64) -> bool,
    /// The records.
    pub records: &'a mut dyn FnMut(u64) -> bool,
}

/// The work budget of a section.
struct Budget<'a> {
    charge: &'a mut dyn FnMut(u64) -> bool,
    out: bool,
}

impl Budget<'_> {
    fn spend(&mut self, units: u64) -> bool {
        if self.out {
            return false;
        }
        if !(self.charge)(units) {
            self.out = true;
            return false;
        }
        true
    }
}

enum Scoped {
    Refused,
    Existing,
    New,
}

/// The test of the walks besides the full query (the policy's list, the feed): the object is ghostable, not always in scope, and
/// within the scope radius (its distance to the scope object, or its box reaching into the query's).
fn within<S: ObjectSource>(src: &S, obj: ObjId, scope: ObjId, eye: [f32; 3], radius: f32) -> bool {
    let near = src.scope_distance(obj, scope).is_some_and(|d| d < radius)
        || src.reaches_scope(obj, eye, radius);
    near && src.net_flags(obj) & (net_flags::GHOSTABLE | net_flags::SCOPE_ALWAYS)
        == net_flags::GHOSTABLE
}

/// `detachObject` on a ghost: `KillGhost`, a full mask in the non-zero region when its mask was 0, and the object
/// unlinked (the ghost lives on until its kill record is acked). The policy is told when the ghost let go of an object.
fn kill_in<P: GhostPolicy>(table: &mut Table, policy: &mut P, id: u32) {
    let Some(g) = table.get(id) else {
        return;
    };
    let (f, mask, linked) = (g.flags | flags::KILL_GHOST, g.mask, g.obj.is_some());
    table.set_flags(id, f);
    if mask == 0 {
        if let Some(g) = table.get_mut(id) {
            g.mask = 0xFFFF_FFFF;
        }
        table.push_nonzero(id);
    }
    table.unlink_object(id);
    if linked {
        policy.ghost_unlinked(id);
    }
}

/// The ghost state of one connection, under the ghosting rules of `P` (plain Torque's by default).
#[derive(Debug, Clone)]
pub struct GhostManager<P: GhostPolicy = TorqueGhosts> {
    cfg: GhostConfig,
    table: Table,
    /// `activateGhosting` ran since the last reset.
    activated: bool,
    /// `mScoping`.
    scoping: bool,
    /// `mGhosting`: the client said Ready.
    ghosting: bool,
    /// `GhostingSequence`.
    sequence: u32,
    /// A Ready of this sequence ran the script (what [`GhostPolicy::ready_once`] reads).
    ready_done: bool,
    scope_object: Option<ObjId>,
    refs: BTreeMap<u32, Vec<GhostRef>>,
    ref_total: usize,
    /// Ghosts with `ScopedEvent` exist (set by `activate`, cleared by the first matching Ready): a repeated Ready then costs no walk.
    scoped_pending: bool,
    /// Objects a budget-cut scan still has to scope (a depth-first worklist, top last).
    pending_scope: Vec<ObjId>,
    /// How many objects of the host's full-query walk a resumed query has already handled.
    query_cursor: usize,
    /// Where a budget-cut walk of the policy's list between full queries goes on.
    recent_cursor: usize,
    /// A full query ran out of budget: the next one skips the `skipCount` pass it already did.
    query_resume: bool,
    /// The feed of new objects ([`GhostManager::set_fresh`]): the number of the next object to look at.
    fresh: Option<u64>,
    /// The ghosting rules.
    policy: P,
    /// The policy's drop pass: the objects to look at and where it is.
    drops: Vec<ObjId>,
    drop_at: usize,
    /// A scan is under way: the objects it scopes are the walks' (the policy's admission holds for them), not a native's.
    walking: bool,
}

impl GhostManager {
    /// The manager of a new connection under plain Torque ghosting (the array is allocated in `onConnectionEstablished`).
    pub fn new(cfg: GhostConfig) -> Result<GhostManager, GhostError> {
        GhostManager::with_policy(cfg, TorqueGhosts)
    }
}

impl<P: GhostPolicy> GhostManager<P> {
    /// The manager of a new connection under a game's ghosting rules.
    pub fn with_policy(cfg: GhostConfig, policy: P) -> Result<GhostManager<P>, GhostError> {
        if cfg.max_ghosts == 0 || cfg.max_ghosts > NET_GHOST_LIMIT {
            return Err(GhostError::BadLimit(cfg.max_ghosts));
        }
        Ok(GhostManager {
            cfg,
            table: Table::default(),
            activated: false,
            scoping: false,
            ghosting: false,
            sequence: 0,
            ready_done: false,
            scope_object: None,
            refs: BTreeMap::new(),
            ref_total: 0,
            query_resume: false,
            scoped_pending: false,
            pending_scope: Vec::new(),
            query_cursor: 0,
            recent_cursor: 0,
            fresh: None,
            policy,
            drops: Vec::new(),
            drop_at: 0,
            walking: false,
        })
    }

    /// The ghosting rules.
    pub fn policy(&self) -> &P {
        &self.policy
    }

    pub fn policy_mut(&mut self) -> &mut P {
        &mut self.policy
    }

    /// What the policy is told about the table.
    pub fn table_counts(&self) -> TableCounts {
        TableCounts {
            max_ghosts: self.cfg.max_ghosts,
            ghosts: self.table.len(),
        }
    }

    /// Whether a drop pass is under way.
    pub fn dropping(&self) -> bool {
        self.drop_at < self.drops.len()
    }

    /// The feed of new objects, a host's option (off by default) for a policy that spaces its full queries out: every packet's
    /// scan also looks at the objects added since the last one ([`ObjectSource::fresh_objects`]) and scopes those within the
    /// radius, as the walk of the policy's list between queries does (without it a new object is met only at the next full query or
    /// on the policy's list). The feed starts at `start` (the source's [`ObjectSource::fresh_end`] for the next object added; None
    /// turns it off); the full queries still run for what it does not cover.
    pub fn set_fresh(&mut self, start: Option<u64>) {
        self.fresh = start;
    }

    /// The feed's cursor, when the connection has one (the host lets go of the objects every connection has passed).
    pub fn fresh_cursor(&self) -> Option<u64> {
        self.fresh
    }

    pub fn config(&self) -> &GhostConfig {
        &self.cfg
    }

    /// `mGhosting` (script `getGhostsActive`).
    pub fn is_ghosting(&self) -> bool {
        self.ghosting
    }

    pub fn is_active(&self) -> bool {
        self.activated
    }

    pub fn sequence(&self) -> u32 {
        self.sequence
    }

    /// Ghosts in use (`FreeIndex`).
    pub fn ghost_count(&self) -> usize {
        self.table.len()
    }

    /// `ZeroUpdateIndex`: ghosts with something to send.
    pub fn pending(&self) -> usize {
        self.table.zero()
    }

    /// Records written and not yet acked or lost.
    pub fn refs_in_flight(&self) -> usize {
        self.ref_total
    }

    pub fn scope_object(&self) -> Option<ObjId> {
        self.scope_object
    }

    /// The scope object (the control object): the full query runs around it.
    pub fn set_scope_object(&mut self, obj: Option<ObjId>) {
        self.scope_object = obj;
    }

    pub fn view(&self, index: u32) -> Option<GhostView> {
        self.table.get(index).map(|g| GhostView {
            index: g.index,
            obj: g.obj,
            mask: g.mask,
            flags: g.flags,
            pos: g.pos,
            skip: g.skip,
        })
    }

    /// [`GhostManager::view`] of the ghost an object has, if any.
    pub fn view_of(&self, obj: ObjId) -> Option<GhostView> {
        self.table.id_of(obj).and_then(|id| self.view(id))
    }

    /// The ids in array order (`[0, free)`), for tests.
    pub fn array_ids(&self) -> Vec<u32> {
        self.table.ids_in_use().collect()
    }

    /// The pack context for [`crate::events::EventQueue::write_section_with`].
    pub fn pack_ctx<'a, S: ObjectSource>(&'a self, src: &'a mut S) -> GhostPackCtx<'a, S> {
        GhostPackCtx {
            src,
            table: &self.table,
            sequence: self.sequence,
            class_bits: self.cfg.class_bits,
            left: Vec::new(),
        }
    }

    // ---- natives

    /// `getGhostIndex`: the wire id, or `None` until the NewGhost record is acked (flags `NotYetGhosted | Ghosting |
    /// KillGhost | KillingGhost`).
    pub fn ghost_index(&self, obj: ObjId) -> Option<u32> {
        index_in(&self.table, obj)
    }

    /// `resolveObjectFromGhostIndex`: the object behind a wire id, whatever the ghost's state.
    pub fn object_at_index(&self, index: u32) -> Option<ObjId> {
        self.table.get(index).and_then(|g| g.obj)
    }

    /// `NetObject::scopeToClient`: `objectInScope(obj)`, then the object's ghost, if it has
    /// one, gets `ScopeLocalAlways` (0x100) so a full query never drops it.
    pub fn scope_to_client<S: ObjectSource>(&mut self, src: &S, obj: ObjId)
    where
        P: ObjectPolicy<S>,
    {
        let mut free = |_| true;
        let mut b = Budget {
            charge: &mut free,
            out: false,
        };
        self.object_in_scope(src, obj, &mut b);
        if let Some(id) = self.table.id_of(obj)
            && let Some(g) = self.table.get(id)
        {
            let f = g.flags | flags::SCOPE_LOCAL_ALWAYS;
            self.table.set_flags(id, f);
        }
    }

    /// `NetObject::clearScopeToClient`: clears the `ScopeLocalAlways` flag.
    pub fn clear_scope_to_client(&mut self, obj: ObjId) {
        if let Some(id) = self.table.id_of(obj)
            && let Some(g) = self.table.get(id)
        {
            let f = g.flags & !flags::SCOPE_LOCAL_ALWAYS;
            self.table.set_flags(id, f);
        }
    }

    /// `objectInScope` for a native or the host (no work budget).
    pub fn object_in_scope_now<S: ObjectSource>(&mut self, src: &S, obj: ObjId)
    where
        P: ObjectPolicy<S>,
    {
        let mut free = |_| true;
        let mut b = Budget {
            charge: &mut free,
            out: false,
        };
        self.object_in_scope(src, obj, &mut b);
    }

    /// The object was deleted (or stopped being ghostable): `detachObject`. The ghost lives on until its kill record
    /// is acked.
    pub fn detach_object(&mut self, obj: ObjId) {
        if let Some(id) = self.table.id_of(obj) {
            kill_in(&mut self.table, &mut self.policy, id);
        }
    }

    /// `NetObject::collapseDirtyList`'s step for this connection: the object's dirty
    /// bits join its ghost's mask; a ghost whose mask was 0 moves to the non-zero region.
    ///
    /// **The host's contract (order).** `setMaskBits` puts an object at the HEAD of the dirty list the first time it
    /// is dirtied, and the collapse walks the list from the head, so objects are collapsed most-recently-first-dirtied first. The
    /// order in which `set_mask` is called decides the array positions of ghosts entering the non-zero region, hence the send order,
    /// so the host keeps the dirty list in that order and calls this once per object per connection that has a ghost of it.
    ///
    /// **Addition:** `pushNonZero` on a ghost that is already in `[0, Zero)` (a zero-mask ghost left there by [`Self::clear_mask`])
    /// does nothing, where the array would swap it with `array[Zero]` and can move a free struct into the used region.
    pub fn set_mask(&mut self, obj: ObjId, bits: u32) {
        if bits == 0 {
            return;
        }
        let Some(id) = self.table.id_of(obj) else {
            return;
        };
        let Some(g) = self.table.get_mut(id) else {
            return;
        };
        if g.mask == 0 {
            g.mask = bits;
            self.table.push_nonzero(id);
        } else {
            g.mask |= bits;
        }
    }

    /// `NetObject::clearMaskBits` for this connection: if the ghost's mask equals `bits` exactly it becomes 0 and the ghost goes to the zero region; otherwise
    /// `mask &= !bits`.
    ///
    /// As `clearMaskBits` writes it (`updateMask == orMask` is the only test): when `bits` covers the mask without equalling it (`mask & !bits == 0`, `mask != bits`) the mask becomes 0 but
    /// the ghost STAYS in `[0, Zero)`. A zero-mask ghost there is sent a record with mask 0 by the next send loop, which then moves it
    /// to the zero region (`retMask == 0`). `GhostManager::check_invariants` allows exactly this state.
    pub fn clear_mask(&mut self, obj: ObjId, bits: u32) {
        let Some(id) = self.table.id_of(obj) else {
            return;
        };
        let Some(g) = self.table.get_mut(id) else {
            return;
        };
        if g.mask == 0 {
            return;
        }
        if g.mask == bits {
            g.mask = 0;
            self.table.push_to_zero(id);
        } else {
            g.mask &= !bits;
        }
    }

    // ---- activate and reset

    /// `activateGhosting`. Returns the events to post, in order: `ConnectionMessage{seq, 3, count = set size}`, one
    /// `GhostAlwaysObjectEvent` per scoped ghost-always ghost in DESCENDING index order, `ConnectionMessage{seq, 0}`.
    ///
    /// **Addition:** a second call without a reset is refused, and a set larger than the ghost limit is
    /// refused.
    pub fn activate<S: ObjectSource>(&mut self, src: &S) -> Result<Vec<GhostEvent>, GhostError>
    where
        P: ObjectPolicy<S>,
    {
        if self.activated {
            return Err(GhostError::AlreadyActive);
        }
        let set = src.ghost_always();
        let sz = set.len();
        if sz as u64 > u64::from(self.cfg.max_ghosts) {
            return Err(GhostError::TooManyGhostAlways {
                count: sz,
                max: self.cfg.max_ghosts,
            });
        }
        self.sequence = self.sequence.wrapping_add(1);
        self.ready_done = false;
        self.table.init(self.cfg.max_ghosts, sz as u32);
        self.activated = true;
        self.scoped_pending = true;
        self.scoping = true;
        let mut free = |_| true;
        let mut b = Budget {
            charge: &mut free,
            out: false,
        };
        for &o in set {
            if src.net_flags(o) & net_flags::GHOSTABLE != 0 {
                self.object_in_scope(src, o, &mut b);
            }
        }
        let mut events = Vec::with_capacity(self.table.zero() + 2);
        events.push(GhostEvent::Message(ConnectionMessage {
            seq: self.sequence,
            message: 3,
            count: sz as u32,
        }));
        for pos in (0..self.table.zero()).rev() {
            let Some(id) = self.table.at(pos) else {
                continue;
            };
            let Some(g) = self.table.get_mut(id) else {
                continue;
            };
            g.mask = 0;
            let (f, obj) = (
                (g.flags & !flags::NOT_YET_GHOSTED) | flags::SCOPED_EVENT,
                g.obj,
            );
            self.table.push_to_zero(id);
            self.table.set_flags(id, f);
            if let Some(obj) = obj {
                events.push(GhostEvent::AlwaysObject {
                    obj,
                    index: id,
                    seq: self.sequence,
                });
            }
        }
        events.push(GhostEvent::Message(ConnectionMessage {
            seq: self.sequence,
            message: 0,
            count: 0,
        }));
        Ok(events)
    }

    /// `resetGhosting`: ghosting and scoping off, every ghost and every ref in flight dropped (their notifies
    /// find nothing), the sequence advanced. Returns the `ConnectionMessage{old sequence, 2}` to post. The next
    /// [`GhostManager::activate`] builds the table again with ids from 0.
    pub fn reset(&mut self) -> GhostEvent {
        let old = self.sequence;
        self.ghosting = false;
        self.scoping = false;
        self.activated = false;
        self.sequence = self.sequence.wrapping_add(1);
        self.ready_done = false;
        self.table.clear();
        self.refs.clear();
        self.ref_total = 0;
        self.query_resume = false;
        self.scoped_pending = false;
        self.pending_scope.clear();
        self.query_cursor = 0;
        self.recent_cursor = 0;
        self.drops = Vec::new();
        self.drop_at = 0;
        self.policy.reset();
        GhostEvent::Message(ConnectionMessage {
            seq: old,
            message: 2,
            count: 0,
        })
    }

    /// The ghost half of `GameConnection::handleConnectionMessage` for a message the CLIENT sent.
    ///
    /// - 1 (Ready) with the current ghosting sequence: `onGhostAlwaysObjectsReceived`, `mGhosting = 1`, and every ghost with
    ///   `ScopedEvent` loses `Ghosting | ScopedEvent`. A repeated Ready runs the script again (as it does), unless the policy
    ///   says once ([`GhostPolicy::ready_once`]: then it is ignored). Another sequence: ignored, no error.
    /// - 0, 2, 3 on a server (no local ghost table): the connection error `Invalid packet.(ghost bad connection message state)`.
    pub fn on_connection_message<C: GhostScripts>(
        &mut self,
        msg: &ConnectionMessage,
        scripts: &mut C,
    ) -> Result<MessageOutcome, GhostError> {
        match msg.message {
            0 | 2 | 3 => Err(GhostError::BadMessageState),
            1 => {
                if msg.seq != self.sequence || self.ready_done && self.policy.ready_once() {
                    return Ok(MessageOutcome::Ignored);
                }
                self.ready_done = true;
                scripts.on_ghost_always_objects_received();
                self.ghosting = true;
                if std::mem::take(&mut self.scoped_pending) {
                    let ids: Vec<u32> = self.table.ids_in_use().collect();
                    for id in ids {
                        if let Some(g) = self.table.get(id)
                            && g.flags & flags::SCOPED_EVENT != 0
                        {
                            let f = g.flags & 0xff6f;
                            self.table.set_flags(id, f);
                        }
                    }
                }
                Ok(MessageOutcome::Ready)
            }
            _ => Ok(MessageOutcome::NotGhost),
        }
    }

    // ---- scope

    /// `objectInScope` with the dependents recursion, depth first in list order.
    fn object_in_scope<S: ObjectSource>(&mut self, src: &S, obj: ObjId, budget: &mut Budget<'_>)
    where
        P: ObjectPolicy<S>,
    {
        self.run_worklist(src, vec![obj], budget);
    }

    /// Scopes the objects of a depth-first worklist (top = next). When the budget runs out the unprocessed rest is kept in
    /// `pending_scope` and the next packet's scan goes on with it, so a dependents chain cut by the budget is not lost.
    fn run_worklist<S: ObjectSource>(
        &mut self,
        src: &S,
        mut stack: Vec<ObjId>,
        budget: &mut Budget<'_>,
    ) where
        P: ObjectPolicy<S>,
    {
        let mut deps = Vec::new();
        while let Some(o) = stack.pop() {
            if !budget.spend(1) {
                stack.push(o);
                let room = NET_GHOST_DEPENDENTS.saturating_sub(self.pending_scope.len());
                let keep = stack.len().min(room);
                let from = stack.len() - keep;
                self.pending_scope.extend_from_slice(&stack[from..]);
                return;
            }
            if let Scoped::New = self.scope_one(src, o) {
                deps.clear();
                src.dependents(o, &mut deps);
                for &d in deps.iter().rev() {
                    if stack.len() >= NET_GHOST_DEPENDENTS {
                        break;
                    }
                    stack.push(d);
                }
            }
        }
    }

    fn scope_one<S: ObjectSource>(&mut self, src: &S, obj: ObjId) -> Scoped
    where
        P: ObjectPolicy<S>,
    {
        if !self.scoping || !src.exists(obj) {
            return Scoped::Refused;
        }
        let nf = src.net_flags(obj);
        if nf & net_flags::SCOPE_LOCAL != 0 && !self.cfg.local {
            return Scoped::Refused;
        }
        if let Some(id) = self.table.id_of(obj) {
            if let Some(g) = self.table.get(id) {
                let f = g.flags | flags::IN_SCOPE;
                self.table.set_flags(id, f);
            }
            return Scoped::Existing;
        }
        if self.walking {
            let counts = self.table_counts();
            if !self.policy.admit(src, self.scope_object, obj, &counts) {
                return Scoped::Refused;
            }
        }
        let mut f = flags::IN_SCOPE | flags::NOT_YET_GHOSTED;
        if nf & net_flags::SCOPE_ALWAYS != 0 {
            f |= flags::SCOPE_ALWAYS;
        }
        match self.table.alloc(obj, f) {
            Some(id) => {
                self.policy.ghost_made(src, obj, id);
                Scoped::New
            }
            None => Scoped::Refused,
        }
    }

    /// The policy's drop pass (resumable, a unit a ghost): every ghost the policy names whose object it says to drop is dropped,
    /// unless a script scoped it or it is always in scope. When it is done the policy is told.
    fn run_drops<S: ObjectSource>(
        &mut self,
        src: &S,
        scope: ObjId,
        now: u32,
        budget: &mut Budget<'_>,
    ) where
        P: ObjectPolicy<S>,
    {
        if self.drop_at >= self.drops.len() {
            return;
        }
        while let Some(&o) = self.drops.get(self.drop_at) {
            if !budget.spend(1) {
                return;
            }
            self.drop_at += 1;
            let Some(id) = self.table.id_of(o) else {
                continue;
            };
            let keep = self.table.get(id).is_none_or(|g| {
                !self.policy.drop_candidate(id)
                    || g.flags & (flags::SCOPE_ALWAYS | flags::SCOPE_LOCAL_ALWAYS) != 0
            });
            if !keep && self.policy.drop_now(src, scope, o) {
                kill_in(&mut self.table, &mut self.policy, id);
            }
        }
        self.drops = Vec::new();
        self.drop_at = 0;
        self.policy.drop_pass_done(now, self.query_resume);
    }

    /// The end of a complete full query: the ghosts it did not find detached (when the policy says so), then the policy's drop
    /// pass when it asks for one.
    fn end_query(&mut self, radius: f32, now: u32) {
        if self.policy.detach_out_of_scope() {
            for id in self.table.out_of_scope_desc() {
                kill_in(&mut self.table, &mut self.policy, id);
            }
        }
        let counts = self.table_counts();
        if self.policy.query_ended(now, radius, &counts) {
            let policy = &self.policy;
            self.drops = self.table.linked_objects(|id| policy.drop_candidate(id));
            self.drop_at = 0;
        }
    }

    /// The scan half of a packet. Returns whether the full query ran.
    ///
    /// **Work budget:** a scan that runs out of budget resumes with the next packet instead of starting over: the objects
    /// whose dependents were cut wait in `pending_scope`, the full query remembers how many of the objects the host's walk visited
    /// it has handled (`query_cursor`, the walk is repeated and the first `query_cursor` visits skipped) and the recent-list walk
    /// remembers its index. Without a cursor a scan larger than the budget would never get past its first objects.
    fn scan<S: ObjectSource>(
        &mut self,
        src: &S,
        now: u32,
        cam: &CamInfo,
        radius: f32,
        budget: &mut Budget<'_>,
    ) -> bool
    where
        P: ObjectPolicy<S>,
    {
        self.walking = true;
        let ran = self.scan_walks(src, now, cam, radius, budget);
        self.walking = false;
        ran
    }

    fn scan_walks<S: ObjectSource>(
        &mut self,
        src: &S,
        now: u32,
        cam: &CamInfo,
        radius: f32,
        budget: &mut Budget<'_>,
    ) -> bool
    where
        P: ObjectPolicy<S>,
    {
        let Some(scope) = self.scope_object else {
            return false;
        };
        let carried = std::mem::take(&mut self.pending_scope);
        if !carried.is_empty() {
            self.run_worklist(src, carried, budget);
        }
        self.policy.scan_started(src, scope);
        self.run_drops(src, scope, now, budget);
        if let Some(cursor) = self.fresh {
            self.fresh = Some(self.scan_fresh(src, scope, cam.eye, radius, cursor, budget));
        }
        let resuming = self.query_resume;
        if self.policy.full_query_due(now, resuming) || resuming {
            if !resuming {
                self.query_cursor = 0;
                let n = self.table.nonzero_ids_by_position().len();
                budget.spend(1 + (n / 64) as u64);
                self.table.start_query();
                let counts = self.table_counts();
                self.policy.query_started(src, scope, &counts);
            }
            self.query_resume = false;
            // The scene graph is built at start-up, so the query runs with or without a Sky (the policy's radius without one).
            if !resuming {
                self.object_in_scope(src, scope, budget);
                if let Some(m) = src.mount_of(scope) {
                    self.object_in_scope(src, m, budget);
                }
            }
            let mut deps = Vec::new();
            // The walk resumes at its place (the host's count of it); the object a spent budget refused is visited again.
            let mut refused = false;
            let walked = src.scope_query_from(scope, cam, radius, self.query_cursor, &mut |o| {
                if !budget.spend(1) {
                    refused = true;
                    return false;
                }
                // The policy's scope rule: an object it leaves out is not scoped this time.
                if !self.policy.query_visit(src, scope, o) {
                    return true;
                }
                if let Scoped::New = self.scope_one(src, o) {
                    deps.clear();
                    src.dependents(o, &mut deps);
                    let d: Vec<ObjId> = deps.iter().rev().copied().collect();
                    self.run_worklist(src, d, budget);
                }
                !budget.out
            });
            self.query_cursor = walked - usize::from(refused && walked > 0);
            if budget.out {
                self.query_resume = true;
            } else {
                self.query_cursor = 0;
                self.end_query(radius, now);
            }
            true
        } else {
            if let Some(list) = self.policy.between_queries(src) {
                let mut i = if self.recent_cursor < list.len() {
                    self.recent_cursor
                } else {
                    0
                };
                while let Some(&o) = list.get(i) {
                    if !budget.spend(1) {
                        break;
                    }
                    if within(src, o, scope, cam.eye, radius) {
                        self.object_in_scope(src, o, budget);
                    }
                    i += 1;
                }
                self.recent_cursor = if budget.out { i } else { 0 };
            }
            false
        }
    }

    /// The feed's walk: the objects added from `cursor` on, each scoped when it is ghostable, not scope-always and within the radius
    /// of the scope object (the recent-list walk's test). Returns where the next walk starts (the first object a spent budget left).
    fn scan_fresh<S: ObjectSource>(
        &mut self,
        src: &S,
        scope: ObjId,
        eye: [f32; 3],
        radius: f32,
        cursor: u64,
        budget: &mut Budget<'_>,
    ) -> u64
    where
        P: ObjectPolicy<S>,
    {
        let (first, list) = src.fresh_objects(cursor);
        let mut at = cursor.max(first);
        let skip = usize::try_from(at - first).unwrap_or(usize::MAX);
        for &o in list.iter().skip(skip) {
            if budget.out || !budget.spend(1) {
                return at;
            }
            if within(src, o, scope, eye, radius) {
                self.object_in_scope(src, o, budget);
            }
            at += 1;
        }
        at
    }

    // ---- the packet

    /// The ghost section of a data packet (`GameConnection::writePacket`): `flag(mGhosting)` and, when ghosting, the scope
    /// work, the policy's word on it, the priorities, the 5-bit id size, the records and the closing `flag(0)`.
    ///
    /// `packet_bits` is `curRate.packetSize * 8`: a record is started while `w.bits() <= packet_bits` (a record may pass it; the
    /// stream's own limit, 12,000 bits, is the hard bound). `send_seq` is the packet's sequence number (the key of its notify).
    /// `sections` are the work budgets in units (see [`SectionBudget`]); when the record budget refuses, the section ends cleanly.
    pub fn write_section<S: ObjectSource>(
        &mut self,
        w: &mut BitWriter,
        packet_bits: usize,
        send_seq: u32,
        now: u32,
        src: &mut S,
        sections: SectionBudget<'_>,
    ) -> SectionStats
    where
        P: ObjectPolicy<S>,
    {
        let mut stats = SectionStats::default();
        if w.is_overflowed() {
            // A packet whose stream already overflowed gets no ghost section (and no refs).
            stats.idle = true;
            return stats;
        }
        // With no room for the smallest ghosting section the packet says "not ghosting" (the client then
        // reads no records; writing it would pass the end of the stream).
        if !self.ghosting || w.remaining_bits() < SECTION_MIN_BITS {
            w.write_flag(false);
            stats.idle = true;
            return stats;
        }
        // A share is of the room the section starts with: what the control and event sections took is not the ghosts' to share.
        let room = packet_bits.saturating_sub(w.bits());
        w.write_flag(true);
        let mut budget = Budget {
            charge: sections.scan,
            out: false,
        };
        let mut rec = Budget {
            charge: sections.records,
            out: false,
        };
        let scope = self.scope_object;
        let sky = src.sky_visible_distance();
        let radius = scope_radius_used(sky, &self.policy.radius());
        let mut cam = scope.and_then(|s| src.camera_info(s)).unwrap_or_default();
        cam.object = scope.unwrap_or(0);
        cam.visible_distance = radius;

        let ran_full = self.scan(&*src, now, &cam, radius, &mut budget);
        stats.full_query = ran_full;

        // The policy hears of the scan's end (after it, so the counts include what it queued).
        self.policy.scanned(&*src, self.table.zero());

        // Priorities and the dead-entry pass. The max index is read before any free (the loop reads it for every ghost it
        // visits, including the ones it frees).
        let max_id = self.table.max_nonzero_id().unwrap_or(0);
        let order = self.policy.pending_order(self.table.zero(), ran_full);
        for id in self.table.dead_by_position_desc() {
            self.table.free(id);
            self.policy.ghost_freed(id);
        }
        if order == PendingOrder::Priority {
            let zero = self.table.zero();
            budget.spend(zero as u64);
            let mut items: Vec<(f32, u32)> = Vec::with_capacity(zero);
            for &id in self.table.nonzero_ids_by_position() {
                let Some(g) = self.table.get(id) else {
                    continue;
                };
                let p = if g.flags & (flags::GHOSTING | flags::KILLING_GHOST) != 0 {
                    0.0
                } else if g.flags & flags::KILL_GHOST != 0 {
                    KILL_PRIORITY
                } else {
                    g.obj.map_or(0.0, |o| {
                        let p = src.update_priority(o, &cam, g.mask, g.skip);
                        if src.packet_share(o).is_none() {
                            p
                        } else if g.flags & flags::NOT_YET_GHOSTED != 0 {
                            SHARE_FIRST_PRIORITY
                        } else {
                            share_priority(p)
                        }
                    })
                };
                items.push((p, id));
            }
            src.sort_pending(&mut items);
            let ids: Vec<u32> = items.iter().map(|&(_, id)| id).collect();
            self.table.set_prefix_order(&ids);
        } else if order == (PendingOrder::Array { scope_first: true })
            && self.table.zero() > 1
            && let Some(id) = scope.and_then(|s| self.table.id_of(s))
            && let Some(pos) = self.table.get(id).map(|g| g.pos)
            && pos < self.table.zero()
        {
            let last = self.table.zero() - 1;
            self.table.swap_array_positions(pos, last);
        }

        let send_size = bit_length(max_id).max(3);
        w.write_bits(u32::from(send_size - 3), 5);

        // A record is kept only when the closing flag still fits after it (a closing `writeFlag` could fall past the stream; the
        // writer would mark the packet overflowed).
        let closing = w.hold_back(1);
        let mut shares = ShareSpend::default();
        let mut i = self.table.zero() as isize - 1;
        while i >= 0 {
            if w.bits() > packet_bits || rec.out {
                break;
            }
            let at = i as usize;
            i -= 1;
            let Some(id) = self.table.at(at) else {
                continue;
            };
            let Some(g) = self.table.get(id) else {
                continue;
            };
            if g.flags & (flags::GHOSTING | flags::KILLING_GHOST) != 0 {
                continue;
            }
            if self.ref_total >= NET_GHOST_REFS {
                break;
            }
            let (gflags, mask, obj) = (g.flags, g.mask, g.obj);
            // An update of an object with a share of the packet waits for a later packet once its group has spent the share.
            let share = obj
                .filter(|_| gflags & flags::KILL_GHOST == 0)
                .and_then(|o| src.packet_share(o))
                .map(|(group, part)| (group, share_cap(room, part)));
            if let Some((group, cap)) = share
                && !shares.open(group, cap)
            {
                stats.share_deferred += 1;
                continue;
            }
            let start = w.bits();
            if !rec.spend(1) {
                break;
            }
            let was_overflowed = w.is_overflowed();
            let mark = w.mark();
            w.write_flag(true);
            w.write_bits(id, send_size);
            if gflags & flags::KILL_GHOST != 0 {
                w.write_flag(true);
                if w.is_overflowed() && !was_overflowed {
                    w.rewind(mark);
                    stats.refused += 1;
                    continue;
                }
                self.commit_kill(id, send_seq);
                stats.kills += 1;
            } else {
                w.write_flag(false);
                let notyet = gflags & flags::NOT_YET_GHOSTED != 0;
                let Some(o) = obj else {
                    w.rewind(mark);
                    stats.refused += 1;
                    continue;
                };
                if notyet {
                    let flagged = self.policy.implied_class_flag();
                    if flagged && self.policy.implied_class(src, o) {
                        w.write_flag(true);
                    } else if let Some(class) = src.class_id(o) {
                        if flagged {
                            w.write_flag(false);
                        }
                        w.write_bits(class, self.cfg.class_bits);
                    } else {
                        w.rewind(mark);
                        stats.refused += 1;
                        continue;
                    }
                }
                let table = &self.table;
                let ret = src.pack_update(o, mask, w, &|t| index_in(table, t));
                if w.is_overflowed() && !was_overflowed {
                    w.rewind(mark);
                    stats.refused += 1;
                    continue;
                }
                if let Some((group, cap)) = share {
                    let used = w.bits().saturating_sub(start);
                    if shares.spent(group) + used > cap {
                        w.rewind(mark);
                        shares.close(group);
                        stats.share_deferred += 1;
                        continue;
                    }
                    shares.add(group, used);
                }
                self.commit_update(id, send_seq, notyet, ret);
            }
            stats.records += 1;
            rec.spend((w.bits().saturating_sub(start) / 32) as u64);
        }
        w.release(closing);
        w.write_flag(false);
        stats.share_bits = shares.most();
        stats.out_of_budget = budget.out || rec.out;
        stats
    }

    fn push_ref(&mut self, id: u32, send_seq: u32, mask: u32, rflags: u8) {
        let Some(g) = self.table.get_mut(id) else {
            return;
        };
        g.chain.push_back((send_seq, mask));
        let generation = g.generation;
        self.refs.entry(send_seq).or_default().push(GhostRef {
            ghost: id,
            generation,
            mask,
            flags: rflags,
        });
        self.ref_total += 1;
    }

    fn commit_update(&mut self, id: u32, send_seq: u32, notyet: bool, ret: u32) {
        let Some(g) = self.table.get_mut(id) else {
            return;
        };
        let old = g.mask;
        g.mask = ret;
        g.skip = 0;
        let f = if notyet {
            (g.flags & !flags::NOT_YET_GHOSTED) | flags::GHOSTING
        } else {
            g.flags
        };
        self.table.set_flags(id, f);
        self.push_ref(
            id,
            send_seq,
            old & !ret,
            if notyet { REF_NEW_GHOST } else { 0 },
        );
        if ret == 0 {
            self.table.push_to_zero(id);
        }
    }

    fn commit_kill(&mut self, id: u32, send_seq: u32) {
        let Some(g) = self.table.get_mut(id) else {
            return;
        };
        let old = g.mask;
        g.mask = 0;
        g.skip = 0;
        let f = (g.flags & !flags::KILL_GHOST) | flags::KILLING_GHOST;
        self.table.set_flags(id, f);
        self.push_ref(id, send_seq, old, REF_KILLING);
        self.table.push_to_zero(id);
    }

    /// The fate of the data packet sent as `send_seq` (the packet layer's notify): the ack or drop routine over
    /// the packet's refs, newest record first. Notifies must come oldest packet first. An unknown or repeated `send_seq` does nothing.
    pub fn on_notify(&mut self, send_seq: u32, delivered: bool) {
        let Some(refs) = self.refs.remove(&send_seq) else {
            return;
        };
        self.ref_total = self.ref_total.saturating_sub(refs.len());
        for r in refs.iter().rev() {
            let Some(g) = self.table.get_mut(r.ghost) else {
                continue;
            };
            if g.generation != r.generation {
                continue;
            }
            let Some(at) = g.chain.iter().position(|&(s, _)| s == send_seq) else {
                continue;
            };
            let newer = g.chain.iter().skip(at + 1).fold(0u32, |a, &(_, m)| a | m);
            g.chain.remove(at);
            if delivered {
                if r.flags & REF_NEW_GHOST != 0 {
                    let f = g.flags & !flags::GHOSTING;
                    self.table.set_flags(r.ghost, f);
                }
                if r.flags & REF_KILLING != 0 {
                    self.table.free(r.ghost);
                    self.policy.ghost_freed(r.ghost);
                }
            } else {
                let lost = r.mask & !newer;
                let mut f = g.flags;
                let was_zero = g.mask == 0;
                if lost != 0 {
                    g.mask = if was_zero { lost } else { g.mask | lost };
                }
                if r.flags & REF_NEW_GHOST != 0 {
                    f = (f | flags::NOT_YET_GHOSTED) & !flags::GHOSTING;
                }
                if r.flags & REF_KILLING != 0 {
                    f = (f | flags::KILL_GHOST) & !flags::KILLING_GHOST;
                }
                self.table.set_flags(r.ghost, f);
                if lost != 0 && was_zero {
                    self.table.push_nonzero(r.ghost);
                }
            }
        }
    }

    /// Checks the table's invariants (tests and fuzzing).
    #[cfg(test)]
    pub(crate) fn check_invariants(&self) -> Result<(), String> {
        self.table.check_invariants()?;
        let mut total = 0;
        for refs in self.refs.values() {
            total += refs.len();
        }
        if total != self.ref_total {
            return Err(format!("ref count {total} != {}", self.ref_total));
        }
        Ok(())
    }
}
