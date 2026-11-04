//! The hub: the registry of ghostable objects, the dirty list, and every connection's ghost manager.
//!
//! **The registry** mirrors what `NetObject::onAdd` and the constructors do: each object of a class the host can pack gets its
//! net flags (Ghostable; ScopeAlways on top for the classes whose constructor sets it, the codecs'
//! [`GhostClassInfo`](super::codecs::GhostClassInfo)) and joins `GhostAlwaysSet` when ScopeAlways (creation order fixes the
//! slot numbers). The ghost game hears of each Ghostable object ([`GhostGame::ghostable_added`]) and of each pass's end
//! ([`GhostGame::end_pass`]); what it keeps of them is its own.
//!
//! **The dirty list** is `NetObject::setMaskBits`': the first time an object is marked it goes to the HEAD of the list and its
//! bits accumulate; [`GhostHub::collapse`] walks the list from the head, so objects are handed to the connections' managers
//! most-recently-first-marked first (that order decides the array positions of the ghosts that enter the non-zero region, hence
//! the send order). Here the order is a sequence number per entry. A deleted object leaves the list; [`GhostHub::clear_mask`]
//! is `clearMaskBits`.
//!
//! **Detach is immediate.** [`GhostHub::removed`] runs `detachObject` in every connection from the object's `onRemove`.
//!
//! Credit: the hub follows TGE as Torque3D, its MIT-licensed release, keeps it: `sim/netObject.cpp` (`Copyright (c) 2012
//! GarageGames, LLC`, `Copyright (C) 2015 Faust Logic, Inc.`) and `sim/netGhost.cpp` (`Copyright (c) 2012 GarageGames, LLC`);
//! the scope eye notes `T3D/player.cpp` (`Copyright (c) 2012 GarageGames, LLC`, `Copyright (C) 2015 Faust Logic, Inc.`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::codecs::NetCodecs;
use torque_ghosts::NetStr;
use torque_net::bits::BitWriter;
use torque_net::events::{ConnectionMessage, PackCtx as EventPackCtx};
use torque_net::ghost::{
    GhostConfig, GhostError, GhostEvent, GhostManager, GhostPolicy, GhostScripts, MessageOutcome,
    ObjectPolicy, ObjectSource, SectionBudget, SectionStats,
};
use torque_net::handshake::ConnId;
use torque_net::huffman::Table;
use torque_net::netstring::{NetStringEvent, SendStrings};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use torque_game::dbvalues::DbValues;

/// The scan work (objects looked at) and the record work (`1 + bits / 32` a record) all connections together may spend in a
/// send pass, each (the pass's one credit is shared equally in the pass's rotating order, [`PassCredit`]): the packets of a
/// burst draw on it, and a section that finds it spent ends cleanly and goes on at the next packet (the manager's cursor). At
/// about a microsecond a unit in a release build, a quarter of a million units is a quarter of a second of work a tick at the
/// very most.
pub const NET_GHOST_WORK_PER_TICK: i64 = 250_000;

/// `NetObject::mNetFlags` bits.
pub mod flags {
    pub use torque_net::ghost::net_flags::{GHOSTABLE, SCOPE_ALWAYS, SCOPE_LOCAL};
}

/// `NetObject::setScopeAlways` also adds the object to the script-visible `GhostAlwaysSet` SimSet, which a script can count and
/// walk (`GhostAlwaysSet.getCount()`), and `activateGhosting` walks that set for the slot numbers, so a script's
/// `GhostAlwaysSet.add` / `.remove` changes what is ghosted-always. A set that does not exist (a VM without the engine's sets)
/// is left alone.
fn join_always_set(vm: &mut Vm, id: ObjId) {
    if let Some(set) = vm.objects().find(b"GhostAlwaysSet") {
        vm.objects_mut().add(set, id);
    }
}

/// What the registry knows of one ghostable object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// The codecs' number of the object's ghost class ([`GhostClassInfo::code`](super::codecs::GhostClassInfo::code)).
    pub code: u16,
    /// The class id of the object group (6 bits).
    pub class_id: u32,
    /// `mNetFlags`: Ghostable, and ScopeAlways for the classes whose constructor sets it.
    pub flags: u32,
}

/// How many records with unsent bits [`Registry::unsent_log`] keeps.
pub const UNSENT_LOG_CAP: usize = 4_096;

/// The ghostable objects, `GhostAlwaysSet`, the feed of new objects and the dirty list: everything that does not belong to
/// one connection.
#[derive(Debug, Default)]
pub struct Registry {
    pub entries: torque_ts_vm::idhash::IdMap<ObjId, Entry>,
    /// The Sky, if there is one (the engine's one sky).
    pub sky: Option<ObjId>,
    /// A diagnostic log: each record whose `packUpdate` left bits unsent (object, the mask it was asked for, the bits it kept),
    /// the first [`UNSENT_LOG_CAP`] of them. Tests read it; nothing depends on it.
    pub unsent_log: std::cell::RefCell<Vec<(ObjId, u32, u32)>>,
    /// object -> (the sequence it was first marked at, its accumulated bits).
    dirty: HashMap<ObjId, (u64, u32)>,
    dirty_seq: u64,
    /// The feed of the connections whose peer is the engine's own client (`GhostManager::set_fresh`): the ghostable objects
    /// added while one of them exists, in order, the first numbered `fresh_base`. At most [`FRESH_CAP`]; the oldest go first (a
    /// connection that falls that far behind meets them at its next full query).
    pub fresh: Vec<ObjId>,
    pub fresh_base: u64,
    /// Whether any connection reads the feed (nothing is kept otherwise).
    pub fresh_wanted: bool,
    /// How many times the feed let go of objects at its cap before every reader had passed them.
    pub fresh_drops: u64,
}

/// The most objects the feed holds (4 MiB).
pub const FRESH_CAP: usize = 1 << 20;

impl Registry {
    /// Appends a new object to the feed, when a connection reads it.
    pub fn push_fresh(&mut self, id: ObjId) {
        if !self.fresh_wanted {
            return;
        }
        if self.fresh.len() >= FRESH_CAP {
            let drop = FRESH_CAP / 4;
            self.fresh.drain(..drop);
            self.fresh_base += drop as u64;
            self.fresh_drops += 1;
        }
        self.fresh.push(id);
    }

    /// Lets go of the objects every reader has passed (`cursors`: the readers' next numbers); with no reader, of all of them.
    pub fn trim_fresh(&mut self, cursors: impl Iterator<Item = u64>) {
        let low = cursors.min();
        self.fresh_wanted = low.is_some();
        let low = low.unwrap_or(self.fresh_base + self.fresh.len() as u64);
        let n = usize::try_from(low.saturating_sub(self.fresh_base))
            .unwrap_or(usize::MAX)
            .min(self.fresh.len());
        if n == 0 {
            return;
        }
        // Drained in large steps only (each moves the rest of the list).
        if n == self.fresh.len() || n >= 4_096 {
            self.fresh.drain(..n);
            self.fresh_base += n as u64;
        }
    }

    /// The feed's end: the number the next object gets.
    pub fn fresh_end(&self) -> u64 {
        self.fresh_base + self.fresh.len() as u64
    }

    /// `setMaskBits`: an object that is not a ghostable object of this host is ignored.
    pub fn set_mask(&mut self, obj: ObjId, bits: u32) {
        if bits == 0 || !self.entries.contains_key(&obj) {
            return;
        }
        let seq = &mut self.dirty_seq;
        let slot = self.dirty.entry(obj).or_insert_with(|| {
            *seq += 1;
            (*seq, 0)
        });
        slot.1 |= bits;
    }

    /// The pending bits of an object that no connection has seen yet (`mDirtyMaskBits`).
    pub fn pending(&self, obj: ObjId) -> u32 {
        self.dirty.get(&obj).map_or(0, |d| d.1)
    }

    /// The pending bits `clearMaskBits` leaves: the object leaves the list when none are left.
    pub fn clear_pending(&mut self, obj: ObjId, bits: u32) {
        if let Some(d) = self.dirty.get_mut(&obj) {
            d.1 &= !bits;
            if d.1 == 0 {
                self.dirty.remove(&obj);
            }
        }
    }

    /// The dirty list head first (the most recently first-marked object first), emptied.
    pub fn take_dirty(&mut self) -> Vec<(ObjId, u32)> {
        let mut list: Vec<(u64, ObjId, u32)> =
            self.dirty.drain().map(|(o, (s, b))| (s, o, b)).collect();
        list.sort_unstable_by_key(|e| std::cmp::Reverse(e.0));
        list.into_iter().map(|(_, o, b)| (o, b)).collect()
    }

    /// The number of objects waiting in the dirty list.
    pub fn dirty_len(&self) -> usize {
        self.dirty.len()
    }
}

/// A script call the ghost layer wants made once the hub is released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhostCall {
    /// The policy's busy/quiet switch: the game's script call for it ([`GhostGame::mode_switch_call`]).
    ModeSwitch(bool),
    /// `%client.onGhostAlwaysObjectsReceived()`.
    AlwaysObjectsReceived,
}

/// Records the manager's script calls.
struct Calls<'a>(&'a mut Vec<GhostCall>);

impl GhostScripts for Calls<'_> {
    fn on_ghost_always_objects_received(&mut self) {
        self.0.push(GhostCall::AlwaysObjectsReceived);
    }
}

/// The sums of [`SectionStats`] over a connection's sections.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub sections: u64,
    pub idle: u64,
    pub records: u64,
    pub kills: u64,
    pub refused: u64,
    pub full_queries: u64,
    pub out_of_budget: u64,
    /// The scan and record work units spent (the credit used).
    pub scan_units: u64,
    pub record_units: u64,
    /// Wall time spent writing the sections, in nanoseconds.
    pub nanos: u64,
}

/// One connection's ghost state.
#[derive(Debug)]
pub struct ConnGhost<P: GhostPolicy, C = ()> {
    pub mgr: GhostManager<P>,
    /// The `GameConnection` object (the script's `%client`).
    pub obj: ObjId,
    /// Script calls made during the connection's packet work, in order.
    pub calls: Vec<GhostCall>,
    /// The scan and record work the connection may still spend this send pass: its share of the pass's one credit
    /// ([`PassCredit`]), drawn when the pass first writes a section for it.
    pub scan_credit: i64,
    pub record_credit: i64,
    /// Whether this pass has drawn the connection's share yet.
    pub allotted: bool,
    /// `GameConnection::getControlCameraFov` (degrees; 90 until the client sends one).
    pub fov_degrees: f32,
    /// `mCameraPos`: the constructor's 0.0, then each client head's first flag (`readPacket`'s
    /// server branch: set gives 0.0, clear 1.0). `isFirstPerson` and the mounted Player's head test it for 0.
    pub camera_pos: f32,
    /// The game's part of the connection ([`GhostGame::Conn`]).
    pub game: C,
    /// The control object (`setControlObject`); the scope object is the same object.
    pub control: Option<ObjId>,
    /// The scope object's eye transform as the send pass last took it (`ShapeBase::onCameraScopeQuery` asks
    /// `getEyeTransform`; a Player's animates its nodes first on a server, as Torque3D's `Player::getEyeBaseTransform` does),
    /// with the object it is of.
    pub scope_eye: Option<(ObjId, torque_collide::math::Matrix)>,
    /// What the sections written for this connection did, added up.
    pub totals: Totals,
    /// The peer is the engine's own client: the game may ghost classes to it that other clients do not know.
    pub engine_peer: bool,
}

/// One connection's ghost state with the game `G`'s part.
pub type Conns<G> = ConnGhost<<G as GhostGame>::Policy, <G as GhostGame>::Conn>;

/// Everything the hub holds.
pub struct HubState<G: GhostGame> {
    pub reg: Registry,
    pub conns: HashMap<ConnId, ConnGhost<G::Policy, G::Conn>>,
    /// The game's part: its ghosting policy's state and what its ghost source reads besides the registry and the container.
    pub game: G,
    /// The server container: its bins are the scope walk's second half.
    pub container: Rc<torque_game::scene::container::Container>,
    pub values: DbValues,
    /// The scan and record credit one send pass has for all the connections together ([`NET_GHOST_WORK_PER_TICK`] each; a test
    /// lowers it).
    pub work_per_tick: i64,
    /// What is left of this pass's credit and who is still to be served.
    pub pass: PassCredit,
    /// The sender's string table of the connection a packet is being written for, lent by the event layer for the packet's
    /// event and ghost sections ([`GhostHub::lend_strings`]).
    pub lent: Option<(ConnId, RefCell<PackStrings>)>,
    /// Each connection's scope walk's place ([`GhostGame::Cursor`]).
    pub scope_cursors: HashMap<ConnId, RefCell<G::Cursor>>,
    /// The game's codecs (the ghost classes, the ghost-id width).
    pub codecs: Rc<dyn NetCodecs>,
}

/// A connection's sender string table while its packet is written: the views map a net string field through it as
/// `NetConnection::packNetStringHandleU` does with `checkString`, and what the mapping did goes back to the event
/// layer with the table ([`GhostHub::take_strings`]).
#[derive(Debug, Default)]
pub struct PackStrings {
    pub send: SendStrings,
    /// The NetString events the misses made, in order, for the connection's ordered queue (as `checkString` makes them).
    pub events: Vec<NetStringEvent>,
    /// The tags the table took a reference on and let go of one on, for the host's tag table.
    pub acquired: Vec<u32>,
    pub released: Vec<u32>,
}

impl PackStrings {
    /// The net string `tag` (whose text is `text`) packs as: its slot once the client has the mapping (`checkString`'s "on the
    /// other side"), else the text (cut at 255 bytes, `writeString(.., 255)`). A miss re-keys the least recently used slot and
    /// makes the event.
    pub fn map(&mut self, tag: u32, text: &[u8]) -> NetStr {
        let checked = self.send.check_string(tag, text);
        self.events.extend(checked.event);
        self.acquired.extend(checked.acquired);
        self.released.extend(checked.released);
        if checked.acked {
            NetStr::Slot(checked.slot)
        } else {
            NetStr::Text(text.get(..text.len().min(255)).unwrap_or(&[]).to_vec())
        }
    }
}

/// The ghost work credit of one send pass, shared by every connection (the bound must not grow with the number of clients, or a
/// hostile one reconnecting in a loop would multiply it).
///
/// The connections are served in the order of the send pass (the rotation it keeps for fairness). Each draws, when its first
/// section of the pass is written, an equal share of what is left: the credit left over the connections still to be served. A
/// connection that uses less hands the rest back when the next one draws, so later connections can use it; one that runs dry
/// goes on at the next pass from its cursor (the manager keeps it), and nothing is lost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PassCredit {
    /// Scan and record units not yet drawn.
    pub scan: i64,
    pub record: i64,
    /// The connections ghosting at the start of the pass that have not drawn yet.
    pub waiting: usize,
    /// The connection served last and the units it was given that it can still hand back.
    pub holder: Option<ConnId>,
}

/// What a pack needs besides the hub: the VM (read), the string coder's table and the ghost-index width.
#[derive(Clone, Copy)]
pub struct SourceEnv<'a> {
    pub vm: &'a Vm,
    pub table: Option<&'a Table>,
    pub ghost_bits: u8,
}

/// The shared handle.
/// What a game adds to the hub: its connections' ghosting policy and the object source a connection's manager walks.
pub trait GhostGame: Sized + 'static {
    /// The connections' ghosting policy.
    type Policy: GhostPolicy + for<'a> ObjectPolicy<Self::Source<'a>> + std::fmt::Debug;
    /// The world as one connection's manager sees it, for the length of one call.
    type Source<'a>: ObjectSource;
    /// Where a connection's scope walk stopped (kept between the calls of a send pass and the passes).
    type Cursor: Default + std::fmt::Debug;
    /// The game's part of each connection ([`ConnGhost::game`]).
    type Conn: Default + Clone + std::fmt::Debug + 'static;

    /// A new connection's policy.
    fn new_policy(&self) -> Self::Policy;

    /// A ghostable object entered the registry.
    fn ghostable_added(&mut self, obj: ObjId) {
        let _ = obj;
    }

    /// The tick pass ended at `now`.
    fn end_pass(&mut self, now: u32) {
        let _ = now;
    }

    /// Whether the connection's policy is in its quiet mode (the scope walk then takes every type).
    fn quiet(mgr: &GhostManager<Self::Policy>) -> bool {
        let _ = mgr;
        true
    }

    /// The policy's busy/quiet switches since the last call, in order ([`GhostCall::ModeSwitch`]).
    fn take_mode_switches(mgr: &mut GhostManager<Self::Policy>) -> Vec<bool> {
        let _ = mgr;
        Vec::new()
    }

    /// Runs `f` with a source over the hub's registry and the connection table. `conn` is the connection whose control object
    /// and field of view the source reports.
    fn with_source<R>(
        hub: &GhostHub<Self>,
        env: SourceEnv<'_>,
        conn: Option<ConnId>,
        f: impl FnOnce(&mut Self::Source<'_>, &mut HashMap<ConnId, Conns<Self>>) -> R,
    ) -> R;

    /// The script method a busy/quiet switch calls on the connection, and its arguments (none by default).
    fn mode_switch_call(on: bool) -> Option<(&'static [u8], Vec<Vec<u8>>)> {
        let _ = on;
        None
    }

    /// Lets `f` set the source's quiet mode for the write that follows.
    fn set_quiet(src: &mut Self::Source<'_>, quiet: bool) {
        let _ = (src, quiet);
    }

    /// Runs `f` with the connection's pack context for the events that name objects (`GhostAlwaysObjectEvent::pack`); None
    /// for a connection the hub does not know.
    fn with_pack_ctx<R>(
        hub: &GhostHub<Self>,
        env: SourceEnv<'_>,
        conn: ConnId,
        f: impl FnOnce(&mut dyn EventPackCtx) -> R,
    ) -> Option<R> {
        Self::with_source(hub, env, Some(conn), |src, conns| {
            let c = conns.get(&conn)?;
            let mut ctx = c.mgr.pack_ctx(src);
            Some(f(&mut ctx))
        })
    }
}

pub struct GhostHub<G: GhostGame>(pub Rc<RefCell<HubState<G>>>);

impl<G: GhostGame> Clone for GhostHub<G> {
    fn clone(&self) -> GhostHub<G> {
        GhostHub(Rc::clone(&self.0))
    }
}

impl<G: GhostGame> std::fmt::Debug for GhostHub<G> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GhostHub")
    }
}

impl<G: GhostGame> PartialEq for GhostHub<G> {
    fn eq(&self, other: &GhostHub<G>) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl<G: GhostGame> GhostHub<G> {
    /// Runs `f` with the game's source over the hub ([`GhostGame::with_source`]).
    pub fn with_source<R>(
        &self,
        env: SourceEnv<'_>,
        conn: Option<ConnId>,
        f: impl FnOnce(&mut G::Source<'_>, &mut HashMap<ConnId, Conns<G>>) -> R,
    ) -> R {
        G::with_source(self, env, conn, f)
    }

    /// Runs `f` with the connection's pack context ([`GhostGame::with_pack_ctx`]).
    pub fn with_pack_ctx<R>(
        &self,
        env: SourceEnv<'_>,
        conn: ConnId,
        f: impl FnOnce(&mut dyn EventPackCtx) -> R,
    ) -> Option<R> {
        G::with_pack_ctx(self, env, conn, f)
    }

    pub fn new(
        game: G,
        container: Rc<torque_game::scene::container::Container>,
        values: DbValues,
        codecs: Rc<dyn NetCodecs>,
    ) -> GhostHub<G> {
        GhostHub(Rc::new(RefCell::new(HubState {
            reg: Registry::default(),
            conns: HashMap::new(),
            game,
            container,
            values,
            work_per_tick: NET_GHOST_WORK_PER_TICK,
            pass: PassCredit {
                scan: NET_GHOST_WORK_PER_TICK,
                record: NET_GHOST_WORK_PER_TICK,
                waiting: 0,
                holder: None,
            },
            lent: None,
            scope_cursors: HashMap::new(),
            codecs,
        })))
    }

    /// Lends the hub `conn`'s sender string table for the sections of the packet being written; [`GhostHub::take_strings`]
    /// hands it back with what the packs did to it. A table still lent is handed back by this call's caller only: lending again
    /// replaces it.
    pub fn lend_strings(&self, conn: ConnId, send: SendStrings) {
        self.0.borrow_mut().lent = Some((
            conn,
            RefCell::new(PackStrings {
                send,
                ..PackStrings::default()
            }),
        ));
    }

    /// The table [`GhostHub::lend_strings`] lent, and the events and references the packs made.
    pub fn take_strings(&self) -> Option<PackStrings> {
        self.0.borrow_mut().lent.take().map(|(_, s)| s.into_inner())
    }

    /// Whether the registry holds the object.
    pub fn knows(&self, obj: ObjId) -> bool {
        self.0.borrow().reg.entries.contains_key(&obj)
    }

    /// Read access to the state (tests, the natives).
    pub fn with<R>(&self, f: impl FnOnce(&HubState<G>) -> R) -> R {
        f(&self.0.borrow())
    }

    pub fn with_mut<R>(&self, f: impl FnOnce(&mut HubState<G>) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }

    // ---- registration

    /// An object entered the tables (`NetObject::onAdd`): a class the host can pack joins the registry; a ScopeAlways one joins
    /// `GhostAlwaysSet`, and the connections that are ghosting at that moment scope it at once.
    pub fn added(&self, vm: &mut Vm, id: ObjId) {
        let Some(object) = vm.objects().get(id) else {
            return;
        };
        let name = vm.objects().classes.name(object.class());
        let codecs = Rc::clone(&self.0.borrow().codecs);
        let Some(info) = std::str::from_utf8(name)
            .ok()
            .and_then(|n| codecs.ghost_class(n))
        else {
            return;
        };
        let flags = info.flags;
        let always = flags & flags::SCOPE_ALWAYS != 0;
        {
            let mut guard = self.0.borrow_mut();
            let hub = &mut *guard;
            let reg = &mut hub.reg;
            reg.entries.insert(
                id,
                Entry {
                    code: info.code,
                    class_id: info.class_id,
                    flags,
                },
            );
            if info.sky {
                reg.sky = Some(id);
            }
            if flags & flags::GHOSTABLE != 0 {
                hub.game.ghostable_added(id);
                if !always {
                    reg.push_fresh(id);
                }
            }
        }
        if always {
            join_always_set(vm, id);
            self.scope_everywhere(vm, id);
        }
    }

    /// An object is leaving (`onRemove`): every connection detaches its ghost at once and the registry forgets it.
    pub fn removed(&self, id: ObjId) {
        let mut hub = self.0.borrow_mut();
        let hub = &mut *hub;
        for c in hub.conns.values_mut() {
            c.mgr.detach_object(id);
            if c.control == Some(id) {
                c.control = None;
                c.mgr.set_scope_object(None);
            }
        }
        hub.container.bins_mut().remove(id);
        let reg = &mut hub.reg;
        if reg.entries.remove(&id).is_some() {
            if reg.sky == Some(id) {
                reg.sky = None;
            }
            reg.dirty.remove(&id);
        }
    }

    /// `NetObject::setScopeAlways`: a ScopeAlways object that appears while a connection is ghosting is scoped at once (it
    /// reaches the client as an ordinary ghost update, not as a `GhostAlwaysObjectEvent`).
    fn scope_everywhere(&self, vm: &Vm, id: ObjId) {
        let ghosting: Vec<ConnId> = self
            .0
            .borrow()
            .conns
            .iter()
            .filter(|(_, c)| c.mgr.is_ghosting())
            .map(|(k, _)| *k)
            .collect();
        for conn in ghosting {
            let env = SourceEnv {
                vm,
                table: None,
                ghost_bits: self.conn_ghost_bits(conn),
            };
            G::with_source(self, env, Some(conn), |src, conns| {
                if let Some(c) = conns.get_mut(&conn) {
                    c.mgr.object_in_scope_now(&*src, id);
                }
            });
        }
    }

    // ---- masks

    /// `setMaskBits`.
    pub fn set_mask(&self, obj: ObjId, bits: u32) {
        self.0.borrow_mut().reg.set_mask(obj, bits);
    }

    /// `clearMaskBits`: the object's pending bits and every connection's ghost mask lose `bits`.
    pub fn clear_mask(&self, obj: ObjId, bits: u32) {
        let mut hub = self.0.borrow_mut();
        let hub = &mut *hub;
        hub.reg.clear_pending(obj, bits);
        for c in hub.conns.values_mut() {
            c.mgr.clear_mask(obj, bits);
        }
    }

    /// `collapseDirtyList`: the dirty list, head first, into every connection's ghosts. Runs once per tick pass before the
    /// connections send.
    pub fn collapse(&self) {
        let mut hub = self.0.borrow_mut();
        let hub = &mut *hub;
        if hub.conns.is_empty() {
            // No connection has a ghost: a new ghost starts with every bit set, so the marks have nothing to add to.
            hub.reg.dirty.clear();
            return;
        }
        for (obj, bits) in hub.reg.take_dirty() {
            for c in hub.conns.values_mut() {
                c.mgr.set_mask(obj, bits);
            }
        }
    }

    /// The tick pass's end at `now`: the ghost game's hook ([`GhostGame::end_pass`]), then the feed of new objects drops what
    /// every reader has read.
    pub fn end_pass(&self, now: u32) {
        let mut hub = self.0.borrow_mut();
        let hub = &mut *hub;
        hub.game.end_pass(now);
        hub.reg
            .trim_fresh(hub.conns.values().filter_map(|c| c.mgr.fresh_cursor()));
    }

    /// Makes `conn` read the feed of new objects (its peer is the engine's own client: the local connection, or a client that
    /// said so when it connected), or stop reading it.
    pub fn set_fresh(&self, conn: ConnId, on: bool) {
        let mut hub = self.0.borrow_mut();
        let hub = &mut *hub;
        if on {
            hub.reg.fresh_wanted = true;
        }
        let end = hub.reg.fresh_end();
        if let Some(c) = hub.conns.get_mut(&conn) {
            c.mgr.set_fresh(on.then_some(end));
        }
    }

    /// Marks `conn`'s peer as the engine's own client (the local connection, or a client that said so when it connected): the
    /// game may ghost it classes that other clients do not know.
    pub fn set_engine_peer(&self, conn: ConnId) {
        if let Some(c) = self.0.borrow_mut().conns.get_mut(&conn) {
            c.engine_peer = true;
        }
    }

    /// Whether `conn` reads the feed of new objects.
    pub fn is_fresh(&self, conn: ConnId) -> bool {
        self.0
            .borrow()
            .conns
            .get(&conn)
            .is_some_and(|c| c.mgr.fresh_cursor().is_some())
    }

    /// The start of a send pass: the pass's one credit is full again and no connection has drawn its share.
    pub fn begin_pass(&self) {
        let mut hub = self.0.borrow_mut();
        let credit = hub.work_per_tick;
        let waiting = hub.conns.values().filter(|c| c.mgr.is_ghosting()).count();
        hub.pass = PassCredit {
            scan: credit,
            record: credit,
            waiting,
            holder: None,
        };
        for c in hub.conns.values_mut() {
            c.scan_credit = 0;
            c.record_credit = 0;
            c.allotted = false;
        }
    }

    // ---- connections

    /// A connection was accepted (`onConnectionEstablished` allocates the ghost array). A limit the manager refuses leaves the
    /// connection without ghosts (and the section a single clear flag).
    pub fn add_conn(&self, conn: ConnId, obj: ObjId, cfg: GhostConfig) -> Result<(), GhostError> {
        let policy = self.0.borrow().game.new_policy();
        let mgr = GhostManager::with_policy(cfg, policy)?;
        self.0.borrow_mut().conns.insert(
            conn,
            ConnGhost {
                mgr,
                obj,
                calls: Vec::new(),
                scan_credit: 0,
                record_credit: 0,
                allotted: false,
                fov_degrees: 90.0,
                camera_pos: 0.0,
                game: G::Conn::default(),
                control: None,
                scope_eye: None,
                totals: Totals::default(),
                engine_peer: false,
            },
        );
        Ok(())
    }

    /// The connection's scope object (its control object), if it has one.
    pub fn scope_object_of(&self, conn: ConnId) -> Option<ObjId> {
        self.0.borrow().conns.get(&conn)?.mgr.scope_object()
    }

    /// The scope object's eye transform for the connection's next scope query ([`ConnGhost::scope_eye`]).
    pub fn set_scope_eye(&self, conn: ConnId, eye: Option<(ObjId, torque_collide::math::Matrix)>) {
        if let Some(c) = self.0.borrow_mut().conns.get_mut(&conn) {
            c.scope_eye = eye;
        }
    }

    /// A connection was accepted with ghost ids `ghost_bits` wide: its ghost manager's table has `1 << ghost_bits` slots (the
    /// server's ghost limit; an engine client's may be wider). A width the manager refuses leaves it without ghosts (the
    /// section is then a single clear flag).
    pub fn connect(&self, conn: ConnId, obj: ObjId, ghost_bits: u8) {
        let cfg = GhostConfig {
            max_ghosts: 1u32.checked_shl(u32::from(ghost_bits)).unwrap_or(0),
            local: false,
            class_bits: self.class_bits(),
        };
        let _ = self.add_conn(conn, obj, cfg);
    }

    /// The size of `conn`'s ghost table (its accept's width), if it has one.
    pub fn conn_ghost_limit(&self, conn: ConnId) -> Option<u32> {
        self.0
            .borrow()
            .conns
            .get(&conn)
            .map(|c| c.mgr.config().max_ghosts)
    }

    /// The width of a ghost index on `conn` (its ghost manager's limit: the width its accept carried; the codecs' starting
    /// width for an unknown connection).
    pub fn conn_ghost_bits(&self, conn: ConnId) -> u8 {
        let hub = self.0.borrow();
        hub.conns.get(&conn).map_or_else(
            || hub.codecs.ghost_id_bits(hub.codecs.start_ghost_limit()),
            |c| c.mgr.config().ghost_bits(),
        )
    }

    /// Whether the connection is ghosting (`mGhosting`).
    pub fn is_ghosting(&self, conn: ConnId) -> bool {
        self.0
            .borrow()
            .conns
            .get(&conn)
            .is_some_and(|c| c.mgr.is_ghosting())
    }

    /// The connection is gone. `GameConnection::onRemove` lets go of the control object after the script's `onDrop`
    /// (`setControllingClient(0)`): the object is marked with the codecs' controlling-client mask
    /// ([`NetCodecs::controlling_client_mask`]), unguarded.
    pub fn remove_conn(&self, conn: ConnId) {
        let mut hub = self.0.borrow_mut();
        let hub = &mut *hub;
        if let Some(c) = hub.conns.remove(&conn)
            && let Some(control) = c.control
        {
            let mask = hub.codecs.controlling_client_mask();
            hub.reg.set_mask(control, mask);
        }
    }

    pub fn has_conn(&self, conn: ConnId) -> bool {
        self.0.borrow().conns.contains_key(&conn)
    }

    /// Whether the connection is in the quiet mode (false for an unknown one: busy, as a new connection is).
    pub fn quiet(&self, conn: ConnId) -> bool {
        self.0
            .borrow()
            .conns
            .get(&conn)
            .is_some_and(|c| G::quiet(&c.mgr))
    }

    /// The script calls recorded for the connection since the last call.
    pub fn take_calls(&self, conn: ConnId) -> Vec<GhostCall> {
        self.0
            .borrow_mut()
            .conns
            .get_mut(&conn)
            .map(|c| std::mem::take(&mut c.calls))
            .unwrap_or_default()
    }

    pub fn set_scope_object(&self, conn: ConnId, obj: Option<ObjId>) {
        if let Some(c) = self.0.borrow_mut().conns.get_mut(&conn) {
            c.mgr.set_scope_object(obj);
        }
    }

    /// The connection `%client` object `obj` belongs to.
    pub fn conn_of_object(&self, obj: ObjId) -> Option<ConnId> {
        self.0
            .borrow()
            .conns
            .iter()
            .find_map(|(k, c)| (c.obj == obj).then_some(*k))
    }

    // ---- the packet

    /// The ghost section of a data packet for `conn` (`flag(mGhosting)` and the rest). `None` when the connection has no ghost
    /// state.
    pub fn write_section(
        &self,
        env: SourceEnv<'_>,
        conn: ConnId,
        w: &mut BitWriter,
        packet_bits: usize,
        send_seq: u32,
    ) -> Option<SectionStats> {
        let now = env.vm.sim_time();
        self.draw_share(conn);
        G::with_source(self, env, Some(conn), |src, conns| {
            let c = conns.get_mut(&conn)?;
            let ConnGhost {
                mgr,
                calls,
                scan_credit,
                record_credit,
                totals,
                ..
            } = c;
            let mut scan_left = *scan_credit;
            let mut rec_left = *record_credit;
            let mut scan = |units: u64| {
                scan_left = scan_left.saturating_sub(i64::try_from(units).unwrap_or(i64::MAX));
                scan_left >= 0
            };
            let mut records = |units: u64| {
                rec_left = rec_left.saturating_sub(i64::try_from(units).unwrap_or(i64::MAX));
                rec_left >= 0
            };
            let timer = std::time::Instant::now();
            G::set_quiet(src, G::quiet(mgr));
            let stats = mgr.write_section(
                w,
                packet_bits,
                send_seq,
                now,
                src,
                SectionBudget {
                    scan: &mut scan,
                    records: &mut records,
                },
            );
            // The busy/quiet switches the section made, in order.
            calls.extend(
                G::take_mode_switches(mgr)
                    .into_iter()
                    .map(GhostCall::ModeSwitch),
            );
            let (scan_used, record_used) = (
                *scan_credit - scan_left.max(0),
                *record_credit - rec_left.max(0),
            );
            *scan_credit = scan_left.max(0);
            *record_credit = rec_left.max(0);
            totals.nanos += u64::try_from(timer.elapsed().as_nanos()).unwrap_or(u64::MAX);
            totals.scan_units += u64::try_from(scan_used).unwrap_or(0);
            totals.record_units += u64::try_from(record_used).unwrap_or(0);
            let t = totals;
            t.sections += 1;
            t.idle += u64::from(stats.idle);
            t.records += stats.records as u64;
            t.kills += stats.kills as u64;
            t.refused += stats.refused as u64;
            t.full_queries += u64::from(stats.full_query);
            t.out_of_budget += u64::from(stats.out_of_budget);
            Some(stats)
        })
    }

    /// The first section of a pass for `conn`: it draws its share of the pass's credit (see [`PassCredit`]) after the
    /// connection served before it hands its unused units back. A connection that has drawn keeps what it has for the rest of
    /// the pass (the burst).
    fn draw_share(&self, conn: ConnId) {
        let mut hub = self.0.borrow_mut();
        let hub = &mut *hub;
        // A connection that is not ghosting writes a flag and spends nothing: it takes no share.
        if hub
            .conns
            .get(&conn)
            .is_none_or(|c| c.allotted || !c.mgr.is_ghosting())
        {
            return;
        }
        if let Some(prev) = hub.pass.holder.take()
            && let Some(p) = hub.conns.get_mut(&prev)
        {
            hub.pass.scan += std::mem::take(&mut p.scan_credit);
            hub.pass.record += std::mem::take(&mut p.record_credit);
        }
        let ways = i64::try_from(hub.pass.waiting.max(1)).unwrap_or(i64::MAX);
        let scan = hub.pass.scan.max(0) / ways;
        let record = hub.pass.record.max(0) / ways;
        hub.pass.scan -= scan;
        hub.pass.record -= record;
        hub.pass.waiting = hub.pass.waiting.saturating_sub(1);
        hub.pass.holder = Some(conn);
        if let Some(c) = hub.conns.get_mut(&conn) {
            c.scan_credit = scan;
            c.record_credit = record;
            c.allotted = true;
        }
    }

    /// The fate of a data packet (the packet layer's notify).
    pub fn on_notify(&self, conn: ConnId, send_seq: u32, delivered: bool) {
        if let Some(c) = self.0.borrow_mut().conns.get_mut(&conn) {
            c.mgr.on_notify(send_seq, delivered);
        }
    }

    /// A client's `ConnectionMessage`. `Ready` records the script call.
    pub fn on_message(
        &self,
        conn: ConnId,
        msg: &ConnectionMessage,
    ) -> Result<MessageOutcome, GhostError> {
        let mut hub = self.0.borrow_mut();
        let Some(c) = hub.conns.get_mut(&conn) else {
            return Ok(MessageOutcome::NotGhost);
        };
        let ConnGhost { mgr, calls, .. } = c;
        mgr.on_connection_message(msg, &mut Calls(calls))
    }

    /// `activateGhosting`: the events to post, in order.
    pub fn activate(
        &self,
        env: SourceEnv<'_>,
        conn: ConnId,
    ) -> Result<Vec<GhostEvent>, GhostError> {
        G::with_source(self, env, Some(conn), |src, conns| {
            match conns.get_mut(&conn) {
                Some(c) => c.mgr.activate(&*src),
                None => Ok(Vec::new()),
            }
        })
    }

    /// `resetGhosting`: the event to post (the old sequence's `EndGhosting`).
    pub fn reset(&self, conn: ConnId) -> Option<GhostEvent> {
        self.0
            .borrow_mut()
            .conns
            .get_mut(&conn)
            .map(|c| c.mgr.reset())
    }

    /// The width of a class id in a record.
    pub fn class_bits(&self) -> u8 {
        self.0.borrow().codecs.ghost_class_bits()
    }

    // ---- natives

    /// `NetObject::scopeToClient` for a connection of the hub.
    pub fn scope_to_client(&self, vm: &Vm, conn: ConnId, obj: ObjId) {
        let env = SourceEnv {
            vm,
            table: None,
            ghost_bits: self.conn_ghost_bits(conn),
        };
        G::with_source(self, env, Some(conn), |src, conns| {
            if let Some(c) = conns.get_mut(&conn) {
                c.mgr.scope_to_client(&*src, obj);
            }
        });
    }

    /// `NetObject::clearScopeToClient`.
    pub fn clear_scope_to_client(&self, conn: ConnId, obj: ObjId) {
        if let Some(c) = self.0.borrow_mut().conns.get_mut(&conn) {
            c.mgr.clear_scope_to_client(obj);
        }
    }

    /// `NetObject::setScopeAlways`: an object that is Ghostable and not a ghost becomes ScopeAlways, joins `GhostAlwaysSet`
    /// and is scoped on every connection that is ghosting.
    pub fn set_scope_always(&self, vm: &mut Vm, obj: ObjId) {
        let applied = {
            let mut hub = self.0.borrow_mut();
            let reg = &mut hub.reg;
            match reg.entries.get_mut(&obj) {
                Some(e) if e.flags & 0x102 == flags::GHOSTABLE => {
                    e.flags |= flags::SCOPE_ALWAYS;
                    true
                }
                _ => false,
            }
        };
        if applied {
            join_always_set(vm, obj);
            self.scope_everywhere(vm, obj);
        }
    }

    /// `getGhostIndex`: the wire id, -1 until the ghost's first record is acknowledged.
    pub fn ghost_index(&self, conn: ConnId, obj: ObjId) -> Option<u32> {
        self.0.borrow().conns.get(&conn)?.mgr.ghost_index(obj)
    }

    /// The object behind a ghost index, whatever the ghost's state.
    pub fn object_at(&self, conn: ConnId, index: u32) -> Option<ObjId> {
        self.0.borrow().conns.get(&conn)?.mgr.object_at_index(index)
    }

    /// The connection's `GameConnection` object of the client that controls `obj` (`ShapeBase::getControllingClient`).
    pub fn controlling_client(&self, obj: ObjId) -> Option<ObjId> {
        self.0
            .borrow()
            .conns
            .values()
            .find_map(|c| (c.control == Some(obj)).then_some(c.obj))
    }

    /// The field of view of the connection's control object as `setControlCameraFov` last read it back: what the scope walk
    /// sizes its view cone by.
    pub fn set_fov(&self, conn: ConnId, degrees: f32) {
        if let Some(c) = self.0.borrow_mut().conns.get_mut(&conn) {
            c.fov_degrees = degrees;
        }
    }

    /// Sets the game's part of `conn` ([`ConnGhost::game`]) when the game's part is a `T`.
    pub fn set_conn_game<T: 'static>(&self, conn: ConnId, value: T) {
        if let Some(c) = self.0.borrow_mut().conns.get_mut(&conn)
            && let Some(slot) = (&mut c.game as &mut dyn std::any::Any).downcast_mut::<T>()
        {
            *slot = value;
        }
    }

    /// The camera position a client head set (see [`ConnGhost::camera_pos`]).
    pub fn set_camera_pos(&self, conn: ConnId, pos: f32) {
        if let Some(c) = self.0.borrow_mut().conns.get_mut(&conn) {
            c.camera_pos = pos;
        }
    }

    /// The core of `GameConnection::setControlObject` for a ShapeBase `new`: nothing happens when it already is the control
    /// object (the scope object is left alone too). Else the old one is let go (`setControllingClient(0)`: the codecs'
    /// [`controlling_client_mask`](NetCodecs::controlling_client_mask)), another connection that controlled `new` loses its
    /// control (and with it its scope object), `new` is marked with that mask, and it becomes the control object and the scope
    /// object. True when something changed.
    pub fn adopt_control(&self, conn: ConnId, new: ObjId) -> bool {
        let mut hub = self.0.borrow_mut();
        let hub = &mut *hub;
        let Some(old) = hub.conns.get(&conn).map(|c| c.control) else {
            return false;
        };
        if old == Some(new) {
            return false;
        }
        let mask = hub.codecs.controlling_client_mask();
        if let Some(old) = old {
            hub.reg.set_mask(old, mask);
        }
        for (k, c) in hub.conns.iter_mut() {
            if *k != conn && c.control == Some(new) {
                c.control = None;
                c.mgr.set_scope_object(None);
            }
        }
        hub.reg.set_mask(new, mask);
        if let Some(c) = hub.conns.get_mut(&conn) {
            c.control = Some(new);
            c.mgr.set_scope_object(Some(new));
        }
        true
    }

    /// `GameConnection::setControlObject(0)`: the old control object is let go (the codecs'
    /// [`controlling_client_mask`](NetCodecs::controlling_client_mask)) and the scope object is none.
    pub fn release_control(&self, conn: ConnId) {
        let mut hub = self.0.borrow_mut();
        let hub = &mut *hub;
        let Some(c) = hub.conns.get_mut(&conn) else {
            return;
        };
        if let Some(old) = c.control.take() {
            hub.reg.set_mask(old, hub.codecs.controlling_client_mask());
        }
        c.mgr.set_scope_object(None);
    }

    /// Hands the control object over without the side effects (tests that set the scene up).
    pub fn set_control(&self, conn: ConnId, obj: Option<ObjId>) {
        if let Some(c) = self.0.borrow_mut().conns.get_mut(&conn) {
            c.control = obj;
            c.mgr.set_scope_object(obj);
        }
    }

    /// Sets the work each connection may spend per send pass (a test of the budget lowers it).
    pub fn set_work_per_tick(&self, units: i64) {
        self.0.borrow_mut().work_per_tick = units;
    }

    /// What the sections of `conn` did so far.
    pub fn totals(&self, conn: ConnId) -> Option<Totals> {
        self.0.borrow().conns.get(&conn).map(|c| c.totals)
    }

    pub fn control_of(&self, conn: ConnId) -> Option<ObjId> {
        self.0.borrow().conns.get(&conn)?.control
    }
}

/// The links between the hub's connections and the objects they control.
impl<G: GhostGame> GhostHub<G> {
    /// `ShapeBase::getControllingClient`: the `GameConnection` whose control chain `obj` is in
    /// (`torque_game::scene::camera::chain`: TGE's control object hands its client to the object it controls in turn).
    pub fn controlling_client_of(&self, vm: &Vm, obj: ObjId) -> Option<ObjId> {
        let hub = self;
        let direct = |o: torque_ts_vm::sim::ObjId| {
            hub.with(|h| {
                h.conns
                    .values()
                    .find(|c| {
                        c.control.is_some_and(|ctl| {
                            torque_game::scene::camera::chain(vm, ctl).contains(&o)
                        })
                    })
                    .map(|c| c.obj)
            })
        };
        // An object a Player controls has the Player's client (`Player::setControlObject` calls `setControllingClient` with its
        // own client): the controller links are followed up, `NET_CONTROL_CHAIN` of them at most.
        let mut at = obj;
        for _ in 0..hub.with(|h| h.codecs.control_chain()) {
            if let Some(c) = direct(at) {
                return Some(c);
            }
            match torque_game::sim::mount::controlling_object(vm, at) {
                Some(holder)
                    if holder != at && torque_game::scene::shapebase::is_player(vm, holder) =>
                {
                    at = holder
                }
                _ => return None,
            }
        }
        None
    }
    /// The camera position (`mCameraPos`) of `obj`'s controlling client ([`Self::controlling_client_of`]), `None` without one.
    pub fn controlling_camera_pos(&self, vm: &Vm, obj: ObjId) -> Option<f32> {
        let client = self.controlling_client_of(vm, obj)?;
        let hub = self;
        hub.with(|h| {
            h.conns
                .values()
                .find(|c| c.obj == client)
                .map(|c| c.camera_pos)
        })
    }
    /// The game's part of the connection whose object is `client`, when the game's part is a `T`.
    pub fn client_game<T: Clone + 'static>(&self, client: ObjId) -> Option<T> {
        self.with(|h| {
            let c = h.conns.values().find(|c| c.obj == client)?;
            (&c.game as &dyn std::any::Any).downcast_ref::<T>().cloned()
        })
    }

    /// The connection whose control chain holds `obj` loses its control object (`GameConnection::setControlObject(0)`).
    pub fn release_controlling_connection(&self, vm: &Vm, obj: ObjId) {
        let hub = self;
        let conn = hub.with(|h| {
            h.conns
                .iter()
                .find(|(_, c)| {
                    c.control.is_some_and(|ctl| {
                        torque_game::scene::camera::chain(vm, ctl).contains(&obj)
                    })
                })
                .map(|(k, _)| *k)
        });
        if let Some(conn) = conn {
            hub.release_control(conn);
        }
    }
}

/// The hub as the scene classes' link to it ([`torque_game::ghost`]).
impl<G: GhostGame> torque_game::ghost::GhostLink for GhostHub<G> {
    fn set_mask(&self, obj: torque_ts_vm::sim::ObjId, bits: u32) {
        GhostHub::set_mask(self, obj, bits);
    }

    fn clear_mask(&self, obj: torque_ts_vm::sim::ObjId, bits: u32) {
        GhostHub::clear_mask(self, obj, bits);
    }

    fn ghosted(&self, obj: torque_ts_vm::sim::ObjId) -> bool {
        self.with(|h| h.reg.entries.contains_key(&obj))
    }

    fn transform_mask(&self, obj: torque_ts_vm::sim::ObjId) -> u32 {
        self.with(|h| {
            h.reg
                .entries
                .get(&obj)
                .map_or(0, |e| h.codecs.transform_mask(e.code))
        })
    }

    fn check_bins(&self, vm: &torque_ts_vm::vm::Vm, obj: torque_ts_vm::sim::ObjId) {
        let container = self.with(|h| std::rc::Rc::clone(&h.container));
        torque_game::scene::object::check_bins(vm, &container, obj);
    }

    fn controlling_client(
        &self,
        vm: &torque_ts_vm::vm::Vm,
        obj: torque_ts_vm::sim::ObjId,
    ) -> Option<torque_ts_vm::sim::ObjId> {
        GhostHub::controlling_client_of(self, vm, obj)
    }

    fn controlling_camera_pos(
        &self,
        vm: &torque_ts_vm::vm::Vm,
        obj: torque_ts_vm::sim::ObjId,
    ) -> Option<f32> {
        GhostHub::controlling_camera_pos(self, vm, obj)
    }

    fn is_control_object(&self, obj: torque_ts_vm::sim::ObjId) -> bool {
        self.with(|h| h.conns.values().any(|c| c.control == Some(obj)))
    }

    fn release_controlling_connection(
        &self,
        vm: &torque_ts_vm::vm::Vm,
        obj: torque_ts_vm::sim::ObjId,
    ) {
        GhostHub::release_controlling_connection(self, vm, obj);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
