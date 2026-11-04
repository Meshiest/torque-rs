//! The net classes' ids and widths, derived by the class-ordering rule (`AbstractClassRep::initialize`), never typed.
//!
//! The class representatives that are networked have a group mask of 1 and a type of 0 (object), 1 (datablock) or 2 (event). For
//! each type in turn the members are sorted by the comparator (the names compared byte
//! by byte as **unsigned** bytes, so upper case sorts before lower case and `fxSunLight` after `WheeledVehicle`; names are unique,
//! so the sort's instability cannot show) and an id is a member's position. The bit width of a class id on the wire is the bit
//! length of the type's count (`NetClassBitSize`: log2 of the next power of two strictly above it): Torque's 13 events take 4
//! bits, 16 to 31 classes 5, 32 to 63 classes 6.
//!
//! The ghost-index width is a different number: `ceil(log2 limit)` of the connection's ghost limit ([`ghost_id_bits`]), 12 at
//! TGE's limit of 4,096 ([`crate::profile::DEFAULT_GHOST_LIMIT`]).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `console/consoleObject.cpp`
//! (`AbstractClassRep::initialize`; Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here
//! is this crate's own.

use crate::bits::bit_length;

/// A net class representative's type word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Object = 0,
    DataBlock = 1,
    Event = 2,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Object, Kind::DataBlock, Kind::Event];

    /// The kind a type word names (0, 1, 2), or None.
    pub fn from_word(word: u32) -> Option<Kind> {
        match word {
            0 => Some(Kind::Object),
            1 => Some(Kind::DataBlock),
            2 => Some(Kind::Event),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Object => "objects",
            Kind::DataBlock => "datablocks",
            Kind::Event => "events",
        }
    }
}

/// An event's direction word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Any,
    ServerToClient,
    ClientToServer,
}

impl Direction {
    pub fn from_word(word: u32) -> Option<Direction> {
        match word {
            0 => Some(Direction::Any),
            1 => Some(Direction::ServerToClient),
            2 => Some(Direction::ClientToServer),
            _ => None,
        }
    }
}

/// What a representative says about a class, before ids exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetRep {
    pub name: String,
    pub kind: Kind,
    pub direction: Direction,
}

/// A net class with its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetClass {
    pub id: u32,
    pub name: String,
    pub kind: Kind,
    pub direction: Direction,
}

/// Why ids could not be derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassError {
    /// Two classes of one kind share a name (the sort rule needs unique names).
    DuplicateName(String),
    /// A class has no name.
    EmptyName,
}

impl std::fmt::Display for ClassError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClassError::DuplicateName(n) => write!(f, "two net classes are named {n}"),
            ClassError::EmptyName => f.write_str("a net class has no name"),
        }
    }
}

impl std::error::Error for ClassError {}

/// The ids of every net class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetClasses {
    by_kind: [Vec<NetClass>; 3],
}

impl NetClasses {
    /// `AbstractClassRep::initialize`: per kind, sort by name (unsigned bytes) and number from 0.
    pub fn derive(reps: &[NetRep]) -> Result<NetClasses, ClassError> {
        let mut by_kind: [Vec<NetClass>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for kind in Kind::ALL {
            let mut members: Vec<&NetRep> = reps.iter().filter(|r| r.kind == kind).collect();
            members.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
            for pair in members.windows(2) {
                if let [a, b] = pair
                    && a.name == b.name
                {
                    return Err(ClassError::DuplicateName(a.name.clone()));
                }
            }
            if members.iter().any(|m| m.name.is_empty()) {
                return Err(ClassError::EmptyName);
            }
            if let Some(slot) = by_kind.get_mut(kind as usize) {
                *slot = members
                    .iter()
                    .enumerate()
                    .map(|(id, m)| NetClass {
                        id: id as u32,
                        name: m.name.clone(),
                        kind,
                        direction: m.direction,
                    })
                    .collect();
            }
        }
        Ok(NetClasses { by_kind })
    }

    /// The classes of `kind` in id order.
    pub fn classes(&self, kind: Kind) -> &[NetClass] {
        self.by_kind.get(kind as usize).map_or(&[], Vec::as_slice)
    }

    pub fn count(&self, kind: Kind) -> usize {
        self.classes(kind).len()
    }

    pub fn total(&self) -> usize {
        Kind::ALL.iter().map(|&k| self.count(k)).sum()
    }

    /// The bits a class id of `kind` takes on the wire: the bit length of the count.
    pub fn id_bits(&self, kind: Kind) -> u8 {
        bit_length(self.count(kind) as u32)
    }

    pub fn id_of(&self, kind: Kind, name: &str) -> Option<u32> {
        self.classes(kind)
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.id)
    }

    pub fn by_id(&self, kind: Kind, id: u32) -> Option<&NetClass> {
        self.classes(kind).get(id as usize)
    }

    /// How many classes of `kind` have each direction: (either way, server to client, client to server).
    pub fn direction_counts(&self, kind: Kind) -> (usize, usize, usize) {
        let n = |d: Direction| {
            self.classes(kind)
                .iter()
                .filter(|c| c.direction == d)
                .count()
        };
        (
            n(Direction::Any),
            n(Direction::ServerToClient),
            n(Direction::ClientToServer),
        )
    }
}

/// The ghost-index width of a table of `limit` ghosts: `ceil(log2 limit)` within 1..=[`crate::cap::NET_MAX_GHOST_BITS`].
pub fn ghost_id_bits(limit: u32) -> u8 {
    let ceil_log2 = if limit <= 1 { 0 } else { bit_length(limit - 1) };
    ceil_log2.clamp(1, crate::cap::NET_MAX_GHOST_BITS)
}

#[cfg(test)]
#[path = "classes_tests.rs"]
mod tests;
