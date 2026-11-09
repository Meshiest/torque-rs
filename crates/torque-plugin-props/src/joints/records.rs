//! The `Joint` ghost record ([`JointGhost`]) and the field types it is made of: what a new ghost is told once (the kind and the two
//! ends), the length, the spring, and the flags (at its limit, broken). Lengths, points and springs travel as whole floats, so a
//! client predicting a grapple gives its Player the server's tether bit for bit. Every value a client decodes is held to its
//! range (a non-finite one reads as 0).

use torque_ghosts::codec::{Codec, F32, R};
use torque_ghosts::plugin::{FieldSpec, NetField};
use torque_ghosts::{Link, ObjectView, Value};
use torque_net::NetError;

use super::{KindId, MAX_DAMPING, MAX_LENGTH, MAX_STIFFNESS};
use crate::quant::MAX_COORD;

/// The mask bit of what a new ghost is told once (and again when a tether becomes a winch): the kind and the two ends.
pub const MASK_INIT: u32 = 1 << 0;
/// The mask bit of the length and the shortest.
pub const MASK_LENGTH: u32 = 1 << 1;
/// The mask bit of the spring.
pub const MASK_SPRING: u32 = 1 << 2;
/// The mask bit of the flags.
pub const MASK_FLAGS: u32 = 1 << 3;

/// A value from the wire held to `-max..=max`, a non-finite one to 0.
fn held(v: f32, max: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-max, max)
    } else {
        0.0
    }
}

/// One end as a client is sent it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum EndWire {
    /// Nothing (a ghost the client does not have).
    #[default]
    Free,
    /// A point in the world.
    Point([f32; 3]),
    /// A ghost (a prop or a player), at `offset` in its own frame (a player's: its position moved by it).
    Ghost { ghost: u32, offset: [f32; 3] },
    /// A ghost the server has not made for this client yet: sent as free, and sent again once it has.
    Waiting,
}

impl NetField for EndWire {
    const MIN_BITS: usize = 2;
    const COMPOUND: bool = true;
    const HAS_FLOATS: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut kind = match self {
            EndWire::Free | EndWire::Waiting => 0,
            EndWire::Point(_) => 1,
            EndWire::Ghost { .. } => 2,
        };
        c.int(name, &mut kind, 2)?;
        match kind {
            0 => *self = EndWire::Free,
            1 => {
                let mut at = match self {
                    EndWire::Point(p) => p.map(F32::new),
                    _ => [F32::default(); 3],
                };
                c.floats(name, &mut at)?;
                *self = EndWire::Point(at.map(|v| held(v.get(), MAX_COORD)));
            }
            2 => {
                let (mut ghost, offset) = match *self {
                    EndWire::Ghost { ghost, offset } => (ghost, offset),
                    _ => (0, [0.0; 3]),
                };
                let bits = c.ghost_bits();
                c.int(name, &mut ghost, bits)?;
                let mut off = offset.map(F32::new);
                let mut has = offset.iter().any(|v| *v != 0.0);
                c.flag(name, &mut has)?;
                if has {
                    c.floats(name, &mut off)?;
                } else {
                    off = [F32::default(); 3];
                }
                *self = EndWire::Ghost {
                    ghost,
                    offset: off.map(|v| held(v.get(), MAX_COORD)),
                };
            }
            _ => return Err(NetError::Invalid("a joint end's kind")),
        }
        Ok(())
    }

    /// The server joint's console fields `<name>Mode` (1 a point, 2 an object), `<name>Point`, `<name>Object` (an object's id)
    /// and `<name>Offset`.
    fn from_view(view: &dyn ObjectView, name: &str, _index: usize, _spec: FieldSpec) -> EndWire {
        let vec = |field: &str| {
            std::array::from_fn(|k| {
                held(
                    torque_ghosts::fld(view, &format!("{name}{field}"), k),
                    MAX_COORD,
                )
            })
        };
        match torque_ghosts::int(view, &format!("{name}Mode"), 0) {
            1 => EndWire::Point(vec("Point")),
            2 => match view.link(&format!("{name}Object")) {
                Link::Ghost(ghost) => EndWire::Ghost {
                    ghost,
                    offset: vec("Offset"),
                },
                Link::NotGhosted => EndWire::Waiting,
                Link::Null => EndWire::Free,
            },
            _ => EndWire::Free,
        }
    }

    fn pending(&self) -> bool {
        *self == EndWire::Waiting
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Str(
            match self {
                EndWire::Free | EndWire::Waiting => "free".to_string(),
                EndWire::Point([x, y, z]) => format!("{x} {y} {z}"),
                EndWire::Ghost { ghost, .. } => format!("ghost {ghost}"),
            }
            .into_bytes(),
        ));
    }
}

/// What a new ghost is told once: the kind and the two ends.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct InitWire {
    pub kind: KindId,
    pub a: EndWire,
    pub b: EndWire,
}

impl NetField for InitWire {
    const MIN_BITS: usize = 3 + 2 * EndWire::MIN_BITS;
    const COMPOUND: bool = true;
    const HAS_FLOATS: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, spec: FieldSpec) -> R {
        let mut kind = self.kind.number();
        c.int(name, &mut kind, 3)?;
        self.kind = KindId::of(kind);
        self.a.layout(c, name, spec)?;
        self.b.layout(c, name, spec)
    }

    /// The server joint's `kind` and its ends `a` and `b` ([`EndWire::from_view`]).
    fn from_view(view: &dyn ObjectView, _name: &str, index: usize, spec: FieldSpec) -> InitWire {
        InitWire {
            kind: KindId::of(torque_ghosts::int(view, "kind", 0).clamp(0, 7) as u32),
            a: EndWire::from_view(view, "a", index, spec),
            b: EndWire::from_view(view, "b", index, spec),
        }
    }

    fn pending(&self) -> bool {
        self.a.pending() || self.b.pending()
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Str(self.kind.name().as_bytes().to_vec()));
    }
}

/// The length (a tether's, a winch's now, a grapple's) and the shortest.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LengthWire {
    pub length: f32,
    pub min: f32,
}

impl NetField for LengthWire {
    const MIN_BITS: usize = 64;
    const COMPOUND: bool = true;
    const HAS_FLOATS: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut raw = [self.length, self.min].map(F32::new);
        c.floats(name, &mut raw)?;
        let length = held(raw[0].get(), MAX_LENGTH).max(0.0);
        *self = LengthWire {
            length,
            min: held(raw[1].get(), MAX_LENGTH).clamp(0.0, length),
        };
        Ok(())
    }

    /// The server joint's `length` and `minLength`.
    fn from_view(
        view: &dyn ObjectView,
        _name: &str,
        _index: usize,
        _spec: FieldSpec,
    ) -> LengthWire {
        LengthWire {
            length: torque_ghosts::fld(view, "length", 0),
            min: torque_ghosts::fld(view, "minLength", 0),
        }
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::F32(self.length));
    }
}

/// A soft joint's spring (stiffness 0: hard).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SpringWire {
    pub stiffness: f32,
    pub damping: f32,
}

impl NetField for SpringWire {
    const MIN_BITS: usize = 1;
    const COMPOUND: bool = true;
    const HAS_FLOATS: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut soft = self.stiffness != 0.0;
        c.flag(name, &mut soft)?;
        if !soft {
            *self = SpringWire::default();
            return Ok(());
        }
        let mut raw = [self.stiffness, self.damping].map(F32::new);
        c.floats(name, &mut raw)?;
        *self = SpringWire {
            stiffness: held(raw[0].get(), MAX_STIFFNESS).max(0.0),
            damping: held(raw[1].get(), MAX_DAMPING).max(0.0),
        };
        Ok(())
    }

    /// The server joint's `stiffness` and `damping`.
    fn from_view(
        view: &dyn ObjectView,
        _name: &str,
        _index: usize,
        _spec: FieldSpec,
    ) -> SpringWire {
        SpringWire {
            stiffness: torque_ghosts::fld(view, "stiffness", 0),
            damping: torque_ghosts::fld(view, "damping", 0),
        }
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::F32(self.stiffness));
    }
}

/// Whether a joint is at its limit and whether it is broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlagsWire {
    pub at_limit: bool,
    pub broken: bool,
}

impl NetField for FlagsWire {
    const MIN_BITS: usize = 2;
    const COMPOUND: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        c.flag(name, &mut self.at_limit)?;
        c.flag(name, &mut self.broken)
    }

    /// The server joint's `atLimit` and `broken`.
    fn from_view(view: &dyn ObjectView, _name: &str, _index: usize, _spec: FieldSpec) -> FlagsWire {
        FlagsWire {
            at_limit: torque_ghosts::int(view, "atLimit", 0) != 0,
            broken: torque_ghosts::int(view, "broken", 0) != 0,
        }
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Bool(self.broken));
    }
}

// ANCHOR: ghost
/// A joint as a client is sent it.
#[derive(Debug, Clone, PartialEq, Default, torque_engine::GhostRecord)]
pub struct JointGhost {
    /// The kind and the two ends.
    #[net(mask = MASK_INIT)]
    pub init: Option<InitWire>,
    /// The length and the shortest.
    #[net(mask = MASK_LENGTH)]
    pub length: Option<LengthWire>,
    /// The spring.
    #[net(mask = MASK_SPRING)]
    pub spring: Option<SpringWire>,
    /// At its limit, broken.
    #[net(mask = MASK_FLAGS)]
    pub flags: Option<FlagsWire>,
}
// ANCHOR_END: ghost

#[cfg(test)]
#[path = "records_tests.rs"]
mod tests;
