//! A client's half of the `props-frozen` channel: the table of frozen props the server sent that way ([`FrozenTable`]), the
//! channel's logic that fills it ([`FrozenChannel`]) and the stills it publishes to the renderer ([`publish`]).
//!
//! Everything here comes from a server, so it is held to caps: at most [`MAX_FROZEN_PROPS`] props in the table (a slot is below
//! [`MAX_PROPS`], so there are never more), a message of at most [`messages::MAX_MESSAGE_BYTES`] bytes and
//! [`messages::MAX_MESSAGE_OPS`] ops,
//! every field inside its range (a message with one that is not is refused whole and counted), a datablock's numbers clamped as
//! the server's are, and a shape of at most [`MAX_SHAPE_BYTES`] bytes drawn at a scale the renderer accepts.
//!
//! **One prop, one picture.** A prop is known by its slot. A client that holds a prop as a ghost does not draw or collide with
//! the table's copy of the same slot ([`FrozenTable::stills`] is told which slots have ghosts, and so is the collision source,
//! [`crate::client::ClientSource`]): during a handover, in either direction, the ghost wins, so the prop is drawn once and
//! there is no frame where it is not drawn.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use torque_engine::net_classes::{PluginStills, Still};
use torque_net::chunk::Kind;
use torque_net::chunk::registry::ChunkChannel;
use torque_ts_vm::vm::{FieldTarget, Vm};

use super::messages::{self, Frozen, Op};
use crate::client::{ClientProps, number, params_of};
use crate::data::{MAX_SHAPE_BYTES, PropParams, draw_scale};
use crate::quant::Quantized;
use crate::world::MAX_PROPS;

/// The most frozen props a client keeps: a prop is in a slot below [`MAX_PROPS`], and a slot holds one.
pub const MAX_FROZEN_PROPS: usize = MAX_PROPS as usize;

/// What a client knows of a datablock, read once.
#[derive(Clone, Debug, PartialEq)]
struct Block {
    params: PropParams,
    /// The shape file and the one scale it is drawn at; none when it is not drawn.
    shape: Option<(String, f32)>,
}

/// A frozen prop as a client holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrozenProp {
    pub prop: Frozen,
    /// How many times the server told the client of it.
    pub updates: u32,
}

/// The frozen props a client was sent over the channel, by slot.
#[derive(Debug, Default)]
pub struct FrozenTable {
    props: BTreeMap<u16, FrozenProp>,
    /// The datablocks the props name that the client has, read once.
    blocks: BTreeMap<u32, Block>,
    /// The revision [`FrozenTable::resolve`] last looked at, and whether a prop names a datablock the client did not have then.
    resolved_for: Option<u64>,
    unresolved: bool,
    /// Messages refused for what was wrong with them.
    pub rejected: u64,
    /// Moves whenever the table, or what it knows of a datablock, changes ([`torque_collide::source::next_revision`]; 0 for a
    /// table never changed).
    revision: u64,
}

impl FrozenTable {
    /// The revision of what the table holds.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn len(&self) -> usize {
        self.props.len()
    }

    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    pub fn get(&self, slot: u16) -> Option<&FrozenProp> {
        self.props.get(&slot)
    }

    /// Every frozen prop, by slot.
    pub fn iter(&self) -> impl Iterator<Item = (u16, &FrozenProp)> {
        self.props.iter().map(|(&slot, p)| (slot, p))
    }

    /// The numbers of the datablock `data`, when the client has it and the table has read it ([`FrozenTable::resolve`]).
    pub fn params(&self, data: u32) -> Option<PropParams> {
        self.blocks.get(&data).map(|b| b.params)
    }

    /// Takes a message from the server: a snapshot replaces the table, a delta changes it. False when the message was refused
    /// (it changed nothing).
    pub fn apply(&mut self, kind: Kind, bytes: &[u8]) -> bool {
        let Ok(ops) = messages::read(bytes) else {
            self.rejected += 1;
            return false;
        };
        if kind == Kind::Snapshot {
            self.props.clear();
        }
        for op in ops {
            match op {
                Op::Set { slot, prop } => {
                    if let Some(held) = self.props.get_mut(&slot) {
                        held.prop = prop;
                        held.updates = held.updates.wrapping_add(1);
                    } else {
                        self.props.insert(slot, FrozenProp { prop, updates: 1 });
                    }
                }
                Op::Remove { slot } => {
                    self.props.remove(&slot);
                }
            }
        }
        self.revision = torque_collide::source::next_revision();
        true
    }

    /// Reads the datablocks the props name that the table has not read yet, and lets go of those no prop names any more. A
    /// datablock the client does not have is not remembered: it is looked for again at the next call while a prop names it, so
    /// a server that names ids it never sent cannot fill the table's cache. True when what the table knows changed.
    pub fn resolve(&mut self, vm: &Vm) -> bool {
        if self.resolved_for == Some(self.revision) && !self.unresolved {
            return false;
        }
        let used: BTreeSet<u32> = self.props.values().map(|p| p.prop.data).collect();
        let before = self.blocks.len();
        self.blocks.retain(|data, _| used.contains(data));
        let mut changed = self.blocks.len() != before;
        self.unresolved = false;
        for data in used {
            if self.blocks.contains_key(&data) {
                continue;
            }
            match block_of(vm, data) {
                Some(block) => {
                    self.blocks.insert(data, block);
                    changed = true;
                }
                None => self.unresolved = true,
            }
        }
        if changed {
            self.revision = torque_collide::source::next_revision();
        }
        self.resolved_for = Some(self.revision);
        changed
    }

    /// The datablocks the table has read (at most those its props name).
    pub fn cached_blocks(&self) -> usize {
        self.blocks.len()
    }

    /// The stills of the props in no slot `ghosted` names: one for each whose datablock has a shape drawn at one scale.
    pub fn stills(&self, ghosted: &dyn Fn(u16) -> bool) -> Vec<(u32, Still)> {
        self.iter()
            .filter(|(slot, _)| !ghosted(*slot))
            .filter_map(|(slot, p)| {
                let (shape, scale) = self.blocks.get(&p.prop.data)?.shape.clone()?;
                Some((
                    u32::from(slot),
                    Still {
                        shape,
                        scale,
                        position: p.prop.q.position(),
                        rotation: still_rotation(&p.prop.q),
                    },
                ))
            })
            .collect()
    }

    /// Every prop forgotten (a new connection).
    pub fn clear(&mut self) {
        self.props.clear();
        self.blocks.clear();
        self.revision = torque_collide::source::next_revision();
    }

    /// The table's words for a trace hash: the slot, the datablock, the flag and the state's thirteen integers, for each prop.
    pub fn state_words(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.props.len() * 16);
        for (&slot, p) in &self.props {
            out.push(u32::from(slot));
            out.push(p.prop.data);
            out.push(u32::from(p.prop.mine));
            out.extend(p.prop.q.words().map(|w| w as u32));
        }
        out
    }
}

/// The turn of a prop as the renderer's transform takes it: the physics world's quaternion with its vector part negated
/// (Torque's turns the other way, as a ghost's transform record says it).
fn still_rotation(q: &Quantized) -> [f32; 4] {
    let [x, y, z, w] = q.rotation();
    [-x, -y, -z, w]
}

/// What the client's datablock `data` says (none when the client has no such object).
fn block_of(vm: &Vm, data: u32) -> Option<Block> {
    let params = params_of(vm, data)?;
    let text = vm.read_field_of(FieldTarget::Live(data), b"shapeFile", b"");
    let shape = std::str::from_utf8(&text)
        .ok()
        .filter(|s| !s.is_empty() && s.len() <= MAX_SHAPE_BYTES)
        .map(str::to_owned);
    let element = |k: usize| {
        let v = number(vm, data, b"drawScale", k);
        if v.is_nan() { 1.0 } else { v }
    };
    let scale = draw_scale([element(0), element(1), element(2)]);
    // A still has one scale: a shape drawn at an uneven one is left to its ghost (a prop sent over the channel with one is
    // not drawn).
    let shape = shape.filter(|_| scale[0] == scale[1] && scale[1] == scale[2]);
    Some(Block {
        params,
        shape: shape.map(|s| (s, scale[0])),
    })
}

/// The channel's logic. A client's copy fills its table; the server's copy has nothing to do (the server sends from its tick,
/// and the client's table is empty when a connection starts).
pub struct FrozenChannel {
    book: ClientProps,
}

impl FrozenChannel {
    pub fn new(book: ClientProps) -> FrozenChannel {
        FrozenChannel { book }
    }
}

impl ChunkChannel for FrozenChannel {
    fn receive(&mut self, kind: Kind, data: &[u8]) {
        if let Ok(mut book) = self.book.0.try_borrow_mut() {
            book.frozen_mut().apply(kind, data);
        }
    }

    fn reset(&mut self) {
        self.closed();
    }

    fn closed(&mut self) {
        if let Ok(mut book) = self.book.0.try_borrow_mut() {
            book.frozen_mut().clear();
        }
    }
}

/// The client tick's part: the datablocks the table's props name read, and the stills for the props the client draws from the
/// table made again when what it shows changed (a prop's ghost came or went, the table changed). `last` is the book's revision
/// the stills were made for.
pub fn publish(vm: &Vm, book: &ClientProps, last: &Cell<u64>) {
    let Some(stills) = vm.extension::<PluginStills>() else {
        return;
    };
    let Ok(mut book) = book.0.try_borrow_mut() else {
        return;
    };
    if book.frozen().is_empty() && last.get() == 0 {
        return;
    }
    book.frozen_mut().resolve(vm);
    let revision = book.shown_revision();
    if revision == last.get() {
        return;
    }
    last.set(revision);
    stills.replace(book.shown_stills());
}
