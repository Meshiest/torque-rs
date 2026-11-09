//! The client's ghosts as script objects: TGE's `NetConnection::ghostReadPacket` constructs an
//! object of each new ghost's class, registers it and adds it to the connection (`ServerConnection` is a group: its members are the
//! datablocks the server sent, then the ghosts in the order they came); a ghost the server deletes is deleted. Scripts reach them
//! by `ServerConnection.getControlObject()`, `resolveGhostID(index)`, `getGhostID(obj)` and the group's `getObject(i)`.
//!
//! The objects are mirrors: the client's world keeps the ghosts' state and draws them. The sync reads only its record world
//! ([`ghost_world::ClientWorld`], over any game's records); [`player_state`] reads the ShapeBase part through the game's records
//! ([`EngineRecord::shape_state`]). Each object is made by the engine ([`Vm::add_engine_object`]: no `onAdd`, its fields read as
//! "" by script) with its `dataBlock` (what `getDataBlock()` reads) from the ghost's record, repointed when an update changes it
//! (`GameBase::unpackUpdate`); `Player::getState()` answers from the record ([`player_state`]).
//!
//! **Cost.** The sync follows the world's ghost log ([`ClientWorld::take_ghost_log`]): each ghost made or deleted since the last
//! sync is one entry, so a packet of updates costs nothing and a large mission's static ghosts cost one object each as they come.
//! Only a reset or a log past [`ghost_world::world::MAX_GHOST_LOG`] entries between two syncs walks the ghost table and the mirrors
//! once. The mirrors are found by index and by object in O(1) (`getGhostID`, `Player::getState`). In a release build, 200,000
//! ghosts made in packets of 500 cost about 1 us of sync each; a packet of updates costs about 0.05 us of sync at 200,000 ghosts as
//! at 2,000. Each mirror is one engine object, about 576 bytes for an object with 12 static fields (the object map's entry, the
//! slots, its name, the group's and the mirror maps' entries): about 115 MB at 200,000, as a TGE client spends one object per ghost.
//!
//! Credit: follows TGE's client ghosting as Torque3D carries it (MIT: `sim/netGhost.cpp`, Copyright (c) 2012 GarageGames, LLC).

use std::cell::RefCell;
use std::rc::Rc;

use ghost_world::world::GhostLogEntry;
use ghost_world::{ClientSet, ClientWorld};
use ts_vm::sim::ObjId;
use ts_vm::vm::Vm;

use crate::conn::ConnGame;
use crate::game::GhostWidths;
use crate::records::EngineRecord;
use crate::state::ClientState;

/// The mirror of one ghost: its object and the ghost's place in the world's birth order (a new ghost under the same index is
/// another object).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mirror {
    pub obj: ObjId,
    pub birth: u64,
}

/// The connection's mirrors, by ghost index and by object. An index is kept only below the connection's ghost table (`2^ghost_bits`
/// of its accept, at most `2^NET_MAX_GHOST_BITS` from an engine server and the game's widest table from any other; the widest table
/// before the accept: [`GhostWidths`]), whatever a server sends. Below the game's widest table (at most 2^20) by index in a dense
/// list: half a million ghosts in a hash table would double it inside a frame (tens of milliseconds of rehashing). An engine
/// server's indices past it go in a map with
/// foldhash's seeded hash, so its memory follows the mirrors kept, not the indices a server names (a hostile one naming an index just
/// under 2^24 makes one entry, not 16 million). By object in the VM's id map (the object ids are the client's own, made in sequence).
#[derive(Debug, Clone)]
pub struct Mirrors {
    pub by_index: Vec<Option<Mirror>>,
    /// The mirrors at indices past the dense list's (`dense`).
    pub wide: std::collections::HashMap<u32, Mirror, ts_vm::namehash::NameBuild>,
    count: usize,
    by_obj: ts_vm::idhash::IdMap<ObjId, u32>,
    /// The indices kept are below it.
    pub bound: usize,
    /// The indices below it are kept in the dense list (the game's widest table); an engine server's wider ones in the map.
    dense: usize,
}

impl Mirrors {
    /// The mirrors of a connection before its accept: indices up to the game's widest table.
    pub fn new(widths: GhostWidths) -> Mirrors {
        let dense = 1usize << widths.max;
        Mirrors {
            by_index: Vec::new(),
            wide: Default::default(),
            count: 0,
            by_obj: Default::default(),
            bound: dense,
            dense,
        }
    }

    /// The mirrors of a connection whose ghost indices are `ghost_bits` wide (held to the widths an accept may carry), the list made
    /// for them at once up to the game's widest table.
    pub fn for_ghost_bits(ghost_bits: u8, widths: GhostWidths) -> Mirrors {
        let bits = ghost_bits.clamp(widths.min, net::cap::NET_MAX_GHOST_BITS);
        let bound = 1usize << bits;
        let mirrors = Mirrors::new(widths);
        Mirrors {
            by_index: Vec::with_capacity(bound.min(mirrors.dense)),
            bound,
            ..mirrors
        }
    }

    /// The ghost index of `obj`, if it is one of the mirrors (`getGhostID` on a client: the object's `mNetIndex`).
    pub fn index_of(&self, obj: ObjId) -> Option<u32> {
        self.by_obj.get(&obj).copied()
    }

    /// The mirror of ghost `index`.
    pub fn get(&self, index: u32) -> Option<Mirror> {
        if (index as usize) < self.dense {
            self.by_index.get(index as usize).copied().flatten()
        } else {
            self.wide.get(&index).copied()
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Every mirror with its index, in index order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, Mirror)> + '_ {
        let mut wide: Vec<(u32, Mirror)> = self.wide.iter().map(|(&i, &m)| (i, m)).collect();
        wide.sort_unstable_by_key(|&(i, _)| i);
        self.by_index
            .iter()
            .enumerate()
            .filter_map(|(index, m)| Some((u32::try_from(index).ok()?, (*m)?)))
            .chain(wide)
    }

    /// Keeps the mirror of ghost `index` (nothing for an index past the ghost table's size).
    pub fn insert(&mut self, index: u32, m: Mirror) {
        let at = index as usize;
        if at >= self.bound {
            return;
        }
        let old = if at < self.dense {
            if self.by_index.len() <= at {
                self.by_index.resize(at + 1, None);
            }
            self.by_index[at].replace(m)
        } else {
            self.wide.insert(index, m)
        };
        match old {
            Some(old) => {
                self.by_obj.remove(&old.obj);
            }
            None => self.count += 1,
        }
        self.by_obj.insert(m.obj, index);
    }

    pub fn remove(&mut self, index: u32) -> Option<Mirror> {
        let m = if (index as usize) < self.dense {
            self.by_index.get_mut(index as usize)?.take()?
        } else {
            self.wide.remove(&index)?
        };
        self.count -= 1;
        self.by_obj.remove(&m.obj);
        Some(m)
    }
}

/// A ghost to mirror: its birth, index, class name and datablock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Born {
    pub birth: u64,
    pub index: u32,
    pub class: &'static str,
    pub datablock: Option<u32>,
}

/// What a sync has to do, from the world's ghost log (taken): the mirrors' objects to delete (already out of `mirrors`) and the
/// ghosts to mirror, in birth order. `work` counts the log entries, mirrors and ghosts looked at (a packet of updates: 0).
#[derive(Debug, Default)]
pub struct Plan {
    pub dead: Vec<ObjId>,
    pub born: Vec<Born>,
    /// Mirrors whose ghost's datablock changed: the object and the new id (the walk after an overflow lists every mirror with its
    /// ghost's current one).
    pub repoint: Vec<(ObjId, Option<u32>)>,
    pub work: usize,
}

/// The plan of a sync ([`Plan`]): the log's entries in order, or, when the log overflowed, one walk of the table and the mirrors.
pub fn plan<S: ClientSet>(world: &mut ClientWorld<S>, mirrors: &mut Mirrors) -> Plan {
    let log = world.take_ghost_log();
    let mut p = Plan::default();
    let mut born: Vec<(u32, u64)> = Vec::new();
    if log.overflowed {
        p.work += mirrors.len() + world.birth_order.len();
        let stale: Vec<u32> = mirrors
            .iter()
            .filter(|(index, m)| world.birth_order.get(index) != Some(&m.birth))
            .map(|(index, _)| index)
            .collect();
        for index in stale {
            if let Some(m) = mirrors.remove(index) {
                p.dead.push(m.obj);
            }
        }
        born.extend(
            world
                .birth_order
                .iter()
                .filter(|(index, birth)| mirrors.get(**index).map(|m| m.birth) != Some(**birth))
                .map(|(index, birth)| (*index, *birth)),
        );
        p.repoint.extend(
            mirrors
                .iter()
                .map(|(index, m)| (m.obj, world.ghosts.get(&index).and_then(|g| g.datablock()))),
        );
    } else {
        p.work += log.entries.len();
        for entry in log.entries {
            match entry {
                GhostLogEntry::Gone { index, birth } => {
                    if mirrors.get(index).is_some_and(|m| m.birth == birth)
                        && let Some(m) = mirrors.remove(index)
                    {
                        p.dead.push(m.obj);
                    }
                }
                GhostLogEntry::Born { index, birth } => {
                    if let Some(m) = mirrors.get(index).filter(|m| m.birth != birth) {
                        mirrors.remove(index);
                        p.dead.push(m.obj);
                    }
                    born.push((index, birth));
                }
                // A ghost born since the last sync is made with its datablock as it is now.
                GhostLogEntry::Repointed {
                    index,
                    birth,
                    datablock,
                } => {
                    if let Some(m) = mirrors.get(index).filter(|m| m.birth == birth) {
                        p.repoint.push((m.obj, Some(datablock)));
                    }
                }
            }
        }
    }
    // A ghost made and deleted between two syncs, a placeholder (no class), or an index past the ghost table, is not mirrored.
    p.born = born
        .into_iter()
        .filter(|(index, _)| (*index as usize) < mirrors.bound)
        .filter(|(index, birth)| world.birth_order.get(index) == Some(birth))
        .filter_map(|(index, birth)| {
            let g = world.ghosts.get(&index)?;
            Some(Born {
                birth,
                index,
                class: S::class_name(g.class?),
                datablock: g.datablock(),
            })
        })
        .collect();
    p.born.sort_unstable();
    p
}

/// Deletes the plan's dead objects and makes its new ones (members of `conn`), returning the new mirrors to insert. A class the
/// client's table does not have (none today) is not mirrored.
pub fn carry_out(vm: &mut Vm, conn: ObjId, plan: Plan) -> Vec<(u32, Mirror)> {
    for obj in plan.dead {
        if vm.objects().contains(obj) {
            let _ = vm.delete_object_quiet(obj);
        }
    }
    for (obj, datablock) in plan.repoint {
        if vm.objects().contains(obj) {
            let text = datablock
                .filter(|d| vm.objects().contains(*d))
                .map_or_else(Vec::new, |d| d.to_string().into_bytes());
            vm.set_object_member(obj, b"dataBlock", b"", &text);
        }
    }
    let mut made = Vec::with_capacity(plan.born.len());
    for b in plan.born {
        let data = b
            .datablock
            .filter(|d| vm.objects().contains(*d))
            .map(|d| d.to_string().into_bytes());
        let fields: Vec<(&[u8], &[u8], &[u8])> = data
            .as_deref()
            .map(|d| vec![(&b"dataBlock"[..], &b""[..], d)])
            .unwrap_or_default();
        let Some(obj) = vm.add_engine_object(b.class.as_bytes(), &fields) else {
            continue;
        };
        if vm.objects().contains(conn) {
            vm.objects_mut().add(conn, obj);
        }
        made.push((
            b.index,
            Mirror {
                obj,
                birth: b.birth,
            },
        ));
    }
    made
}

/// Brings the connection's mirrors up to its world ([`plan`], [`carry_out`]): deleted ghosts' objects deleted, new ghosts' objects
/// made in the order the client read them and added to the connection object `conn`.
pub fn sync<G: ConnGame>(vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>, conn: ObjId) {
    let plan = {
        let mut st = client.borrow_mut();
        let Some(c) = st.conn.conn.as_mut() else {
            return;
        };
        let c = &mut **c;
        plan(&mut c.world, &mut c.mirrors)
    };
    if plan.dead.is_empty() && plan.born.is_empty() && plan.repoint.is_empty() {
        return;
    }
    let made = carry_out(vm, conn, plan);
    let mut st = client.borrow_mut();
    if let Some(c) = st.conn.conn.as_mut() {
        for (index, m) in made {
            c.mirrors.insert(index, m);
        }
    }
}

/// The mirror object of ghost `index`, if any.
pub fn object_of<G: ConnGame>(client: &Rc<RefCell<ClientState<G>>>, index: u32) -> Option<ObjId> {
    let st = client.borrow();
    st.conn.conn.as_ref()?.mirrors.get(index).map(|m| m.obj)
}

/// The ghost index of the mirror object `obj`, if it is one.
pub fn index_of<G: ConnGame>(client: &Rc<RefCell<ClientState<G>>>, obj: ObjId) -> Option<u32> {
    client.borrow().conn.conn.as_ref()?.mirrors.index_of(obj)
}

/// `Player::getState()` of a mirrored Player or AIPlayer, from its record: "Dead" when its damage state is not Enabled, "Mounted"
/// when it has a mount, else "Move" (the recover state is the server's). `None` for an object that is not a mirrored player.
pub fn player_state<G: ConnGame>(
    client: &Rc<RefCell<ClientState<G>>>,
    obj: ObjId,
) -> Option<&'static str> {
    let st = client.borrow();
    let c = st.conn.conn.as_ref()?;
    let index = c.mirrors.index_of(obj)?;
    let shape = c.world.ghosts.get(&index)?.record.as_ref()?.shape_state()?;
    let damaged = shape.damage.is_some_and(|d| d != 0);
    let mounted = shape.mount.is_some();
    Some(if damaged {
        "Dead"
    } else if mounted {
        "Mounted"
    } else {
        "Move"
    })
}

/// The ghost objects' own methods on the client, over the server host's: `NetObject::getGhostID()` (the object's
/// net index, the ghost index of a ghost's object) and `Player::getState()` ([`player_state`]). Any other object keeps the server
/// host's answer.
pub fn register<G: ConnGame>(vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>) {
    let prev = vm.native_of("NetObject", "getGhostID").map(|t| t.3);
    let st = Rc::clone(client);
    torque_game::methods::register_method(vm, "NetObject", "getGhostID", move |vm, c| {
        let index = c.this_object.and_then(|o| {
            let s = st.borrow();
            s.conn.conn.as_ref().and_then(|x| x.mirrors.index_of(o))
        });
        match (index, &prev) {
            (Some(i), _) => ts_vm::vm::Native::Int(i as i32),
            (None, Some(p)) => p(vm, c),
            (None, None) => ts_vm::vm::Native::Int(-1),
        }
    });
    let prev = vm.native_of("Player", "getState").map(|t| t.3);
    let st = Rc::clone(client);
    torque_game::methods::register_method(vm, "Player", "getState", move |vm, c| {
        let state = c.this_object.and_then(|o| player_state(&st, o));
        match (state, &prev) {
            (Some(s), _) => ts_vm::vm::Native::Str(s.as_bytes().to_vec()),
            (None, Some(p)) => p(vm, c),
            (None, None) => ts_vm::vm::Native::Str(Vec::new()),
        }
    });
}
