//! A client's half of joints: the book of the joints its connection was sent ([`JointBook`]), where their ends are as the client
//! knows them, and the tether a grapple puts on the Player the client predicts.
//!
//! Joints interpolate with the props at their ends: a client draws them (a rope following one, [`super::rope`]) where its props
//! are. [`JointBook::connected`] gives the props a joint ties to a prop, for a predictor that pulls them in with it.
//!
//! Everything here comes from a server, so it is held to caps: at most [`MAX_CLIENT_JOINTS`] joints in the book (a ghost past it
//! is left out), every value held to its range by its decoder, and a tether given to the prediction only when it is finite.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use torque_engine::net_classes::GhostEvent;
use torque_game::player_tether::{Tether, TetherSource};

use super::records::{EndWire, JointGhost};
use super::{KindId, MIN_LENGTH, point_on};
use crate::client::ClientProps;

/// The most joints a client keeps.
pub const MAX_CLIENT_JOINTS: usize = 4096;

/// One joint as a client knows it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClientJoint {
    pub kind: KindId,
    pub a: EndWire,
    pub b: EndWire,
    /// The length now and the shortest.
    pub length: f32,
    pub min: f32,
    /// The spring past the length (stiffness 0: hard).
    pub stiffness: f32,
    pub damping: f32,
    pub at_limit: bool,
    pub broken: bool,
}

/// The joints a client's connection was sent, by ghost index.
#[derive(Debug, Default)]
pub struct JointBook {
    joints: BTreeMap<u32, ClientJoint>,
    /// Ghosts left out because the book was full.
    pub dropped: u64,
}

impl JointBook {
    pub fn joints(&self) -> &BTreeMap<u32, ClientJoint> {
        &self.joints
    }

    pub fn get(&self, index: u32) -> Option<&ClientJoint> {
        self.joints.get(&index)
    }

    pub fn len(&self) -> usize {
        self.joints.len()
    }

    pub fn is_empty(&self) -> bool {
        self.joints.is_empty()
    }

    /// A ghost's record, as a client's handler hears it (the whole record, updates merged).
    pub fn heard(&mut self, index: u32, record: &JointGhost) {
        let full = self.joints.len() >= MAX_CLIENT_JOINTS;
        let entry = match self.joints.get_mut(&index) {
            Some(e) => e,
            None if full => {
                self.dropped += 1;
                return;
            }
            None => self.joints.entry(index).or_default(),
        };
        if let Some(init) = record.init {
            entry.kind = init.kind;
            entry.a = init.a;
            entry.b = init.b;
        }
        if let Some(l) = record.length {
            entry.length = l.length;
            entry.min = l.min.min(l.length);
        }
        if let Some(s) = record.spring {
            entry.stiffness = s.stiffness;
            entry.damping = s.damping;
        }
        if let Some(f) = record.flags {
            entry.at_limit = f.at_limit;
            entry.broken = f.broken;
        }
    }

    /// A ghost the server deleted (or the connection ended).
    pub fn removed(&mut self, index: u32) {
        self.joints.remove(&index);
    }

    pub fn clear(&mut self) {
        self.joints.clear();
    }

    /// The ghosts the unbroken joints tie to the ghost `index` (a prop or a player), in ghost order: what a predictor pulls in
    /// with a prop it predicts.
    pub fn connected(&self, index: u32) -> Vec<u32> {
        let ghost = |e: &EndWire| match *e {
            EndWire::Ghost { ghost, .. } => Some(ghost),
            _ => None,
        };
        let mut out: Vec<u32> = self
            .joints
            .values()
            .filter(|j| !j.broken)
            .filter_map(|j| match (ghost(&j.a), ghost(&j.b)) {
                (Some(a), Some(b)) if a == index => Some(b),
                (Some(a), Some(b)) if b == index => Some(a),
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The grapple whose first end is the ghost `player`: none when there are none or two.
    pub fn grapple_on(&self, player: u32) -> Option<&ClientJoint> {
        let mut grapples = self.joints.values().filter(|j| {
            j.kind == KindId::Grapple
                && matches!(j.a, EndWire::Ghost { ghost, .. } if ghost == player)
        });
        let one = grapples.next()?;
        grapples.next().is_none().then_some(one)
    }

    /// The joints as words, in ghost order (the index, the kind, the length and the shortest by their bits, the flags), for a
    /// trace hash.
    pub fn state_words(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.joints.len() * 5);
        for (&i, j) in &self.joints {
            out.extend([
                i,
                j.kind.number(),
                j.length.to_bits(),
                j.min.to_bits(),
                u32::from(j.at_limit) | u32::from(j.broken) << 1,
            ]);
        }
        out
    }
}

/// The engine resource that holds a client's joints. One client per engine (as with every plugin's client state).
#[derive(Clone, Debug, Default)]
pub struct ClientJoints(pub Rc<RefCell<JointBook>>);

impl ClientJoints {
    /// Reads the book, if no one is changing it.
    pub fn with<R>(&self, f: impl FnOnce(&JointBook) -> R) -> Option<R> {
        self.0.try_borrow().ok().map(|b| f(&b))
    }
}

/// The client's handler of `Joint` ghosts.
pub fn heard(book: &ClientJoints, event: GhostEvent<'_, JointGhost>) {
    let Ok(mut book) = book.0.try_borrow_mut() else {
        return;
    };
    match event {
        GhostEvent::Added { index, record } | GhostEvent::Changed { index, record } => {
            book.heard(index, record);
        }
        GhostEvent::Removed { index } => book.removed(index),
    }
}

/// Where an end is as the client knows it: a point, or a prop at its last state (a point of its frame). None for a ghost the
/// client has no prop of, or a free end.
pub fn end_point(end: &EndWire, props: &ClientProps) -> Option<[f32; 3]> {
    match *end {
        EndWire::Point(p) => Some(p),
        EndWire::Ghost { ghost, offset } => props
            .with(|b| b.get(ghost).map(|p| point_on(&p.q, offset)))
            .flatten(),
        EndWire::Free | EndWire::Waiting => None,
    }
}

/// The grapples' tethers on the Player a client predicts, from its joints and props: the tether of the grapple whose first end is
/// that Player's ghost, found as the server finds its own ([`crate::world::PropWorld::grapple_on`]): a player has one grapple, a
/// broken one puts no tether on it, and a book that holds two for one player (between the server's deletion of one and its
/// making of the next) gives none.
#[derive(Clone, Debug)]
pub struct ClientTethers {
    pub joints: ClientJoints,
    pub props: ClientProps,
}

impl TetherSource for ClientTethers {
    fn tether(&self, player: u32) -> Option<Tether> {
        let j = self
            .joints
            .with(|book| book.grapple_on(player).copied())
            .flatten()
            .filter(|j| !j.broken)?;
        let (EndWire::Ghost { offset, .. }, b, length, stiffness, damping) =
            (j.a, j.b, j.length, j.stiffness, j.damping)
        else {
            return None;
        };
        let anchor = end_point(&b, &self.props)?;
        Tether {
            anchor,
            offset,
            length: length.max(MIN_LENGTH),
            stiffness,
            damping,
        }
        .held()
    }
}
