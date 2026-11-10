//! What goes on the wire: the `Prop` ghost record ([`PropGhost`]) and the field types it is made of. Each group is one `Option`
//! with its own mask bit, so an update sends only what changed: the state of a moving prop each time it changes, the flags when it
//! falls asleep, wakes, freezes or thaws (with one last exact state), and what a new ghost is told once.

use torque_ghosts::ObjectView;
use torque_ghosts::codec::{Codec, F32, R, Xf};
use torque_ghosts::plugin::{DatablockRef, FieldSpec, NetField};
use torque_ghosts::{Value, int};

use crate::hulls::HullDigest;
use crate::quant::{Quantized, ROT_BITS};

/// The mask bit of what a new ghost is told once (and again when its owner changes): its slot, whether it is the client's own,
/// its predict id.
pub const MASK_INIT: u32 = 1 << 0;
/// The mask bit of the state (pose and speeds).
pub const MASK_STATE: u32 = 1 << 1;
/// The mask bit of the sleep and freeze flags.
pub const MASK_FLAGS: u32 = 1 << 2;

/// The bits of a prop's slot: the key both sides give its collision shape (the server holds at most 1,024 props).
pub const SLOT_BITS: u8 = 10;

/// The server's console fields the state is kept in, in [`Quantized::words`] order.
pub const STATE_FIELDS: [&str; 13] = [
    "qPosX", "qPosY", "qPosZ", "qRotI", "qRotA", "qRotB", "qRotC", "qLinX", "qLinY", "qLinZ",
    "qAngX", "qAngY", "qAngZ",
];

/// The widths a position component takes, by its size class.
const POS_WIDTHS: [u8; 4] = [12, 18, 24, 28];
/// The widths a speed or spin component takes.
const VEL_WIDTHS: [u8; 4] = [6, 10, 14, 18];

/// Writes or reads a signed integer as a 2-bit size class and that many bits (its value offset to be unsigned). A decoded value is
/// within the widest class's range; the caller holds it to its own.
fn sint<C: Codec>(c: &mut C, name: &'static str, v: &mut i32, widths: [u8; 4]) -> R {
    let fits = |bits: u8, v: i32| {
        let half = 1i64 << (bits - 1);
        (-half..half).contains(&i64::from(v))
    };
    let mut class = widths
        .iter()
        .position(|&b| fits(b, *v))
        .unwrap_or(widths.len() - 1) as u32;
    c.int(name, &mut class, 2)?;
    let bits = widths[(class as usize).min(widths.len() - 1)];
    let half = 1i64 << (bits - 1);
    let mut raw = (i64::from(*v).clamp(-half, half - 1) + half) as u32;
    c.int(name, &mut raw, bits)?;
    *v = (i64::from(raw) - half) as i32;
    Ok(())
}

/// A prop's quantized state on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StateWire(pub Quantized);

impl NetField for StateWire {
    const MIN_BITS: usize = 3 * 14 + 2 + 3 * 15 + 1;
    const COMPOUND: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let q = &mut self.0;
        for v in &mut q.pos {
            sint(c, name, v, POS_WIDTHS)?;
        }
        let mut index = u32::from(q.rot_index.min(3));
        c.int(name, &mut index, 2)?;
        q.rot_index = index as u8;
        for r in &mut q.rot {
            let mut v = u32::from(*r);
            c.int(name, &mut v, ROT_BITS)?;
            *r = v as u16;
        }
        let mut moving = q.moving();
        c.flag(name, &mut moving)?;
        if moving {
            for v in q.lin.iter_mut().chain(&mut q.ang) {
                sint(c, name, v, VEL_WIDTHS)?;
            }
        } else {
            q.lin = [0; 3];
            q.ang = [0; 3];
        }
        *q = q.held();
        Ok(())
    }

    /// The server prop's thirteen state fields ([`STATE_FIELDS`]).
    fn from_view(view: &dyn ObjectView, _name: &str, _index: usize, _spec: FieldSpec) -> StateWire {
        let words = STATE_FIELDS
            .map(|f| int(view, f, 0).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
        StateWire(Quantized::from_words(words))
    }

    fn values(&self, out: &mut Vec<Value>) {
        let [x, y, z] = self.0.position();
        out.push(Value::Str(format!("{x} {y} {z}").into_bytes()));
    }

    /// The pose as a transform record: the position and the quaternion in Torque's form (the inverse turn's: Torque's
    /// `QuatF::setMatrix` turns the other way), so a client draws it where the server has it.
    fn transform(&self) -> Option<Xf> {
        Some(xf_of(self.0.position(), self.0.rotation()))
    }
}

/// The transform record of a pose (a unit quaternion `x, y, z, w` that turns the prop's points into the world's).
pub fn xf_of(position: [f32; 3], q: [f32; 4]) -> Xf {
    Xf {
        position: position.map(F32::new),
        quat: [-q[0], -q[1], -q[2]].map(F32::new),
        negative_w: q[3] < 0.0,
    }
}

/// What a new ghost is told once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InitWire {
    /// The prop's slot on the server (0 to 1,023): the key of its collision shape on both sides.
    pub slot: u16,
    /// Whether the connection it is sent to owns it.
    pub mine: bool,
    /// The id the owner's client gave a prop it showed at once (1 to 255), for that client alone; 0 for none.
    pub predict: u8,
    /// What the server collides it as when that is its shape's hulls ([`HullDigest`]); none when it is the datablock's box. A
    /// client predicts the hulls only if its own build of the shape has this digest.
    pub hulls: Option<HullDigest>,
}

impl NetField for InitWire {
    const MIN_BITS: usize = SLOT_BITS as usize + 3;
    const COMPOUND: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut slot = u32::from(self.slot) & ((1 << SLOT_BITS) - 1);
        c.int(name, &mut slot, SLOT_BITS)?;
        self.slot = slot as u16;
        c.flag(name, &mut self.mine)?;
        let mut has = self.predict != 0;
        c.flag(name, &mut has)?;
        if has {
            let mut p = u32::from(self.predict);
            c.int(name, &mut p, 8)?;
            self.predict = p as u8;
        } else {
            self.predict = 0;
        }
        let mut has = self.hulls.is_some();
        c.flag(name, &mut has)?;
        if has {
            let d = self.hulls.get_or_insert_with(HullDigest::default);
            let (mut hulls, mut points) = (u32::from(d.hulls), u32::from(d.points));
            c.int(name, &mut hulls, 8)?;
            c.int(name, &mut points, 16)?;
            c.int(name, &mut d.volume, 32)?;
            d.hulls = hulls as u8;
            d.points = points as u16;
        } else {
            self.hulls = None;
        }
        Ok(())
    }

    /// The server prop's `slot`, `ownerConn` (the owner's connection object), `predict` and the digest of its hulls
    /// (`hullCount`, `hullPoints`, `hullVolume`: none for a count of 0), against the connection packed for.
    fn from_view(view: &dyn ObjectView, _name: &str, _index: usize, _spec: FieldSpec) -> InitWire {
        let owner = int(view, "ownerConn", 0);
        let conn = int(view, "@connection", 0);
        let mine = owner > 0 && owner == conn;
        let predict = int(view, "predict", 0);
        InitWire {
            slot: int(view, "slot", 0).clamp(0, (1 << SLOT_BITS) - 1) as u16,
            mine,
            predict: if mine && (1..=255).contains(&predict) {
                predict as u8
            } else {
                0
            },
            hulls: (int(view, "hullCount", 0) > 0).then(|| HullDigest {
                hulls: int(view, "hullCount", 0).clamp(0, 255) as u8,
                points: int(view, "hullPoints", 0).clamp(0, 65_535) as u16,
                volume: int(view, "hullVolume", 0) as u32,
            }),
        }
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Int(i64::from(self.slot)));
    }
}

/// Whether a prop sleeps, whether it is frozen and whether a player holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlagsWire {
    pub asleep: bool,
    pub frozen: bool,
    pub held: bool,
}

impl NetField for FlagsWire {
    const MIN_BITS: usize = 3;
    const COMPOUND: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        c.flag(name, &mut self.asleep)?;
        c.flag(name, &mut self.frozen)?;
        c.flag(name, &mut self.held)
    }

    /// The server prop's `asleep`, `frozen` and `held`.
    fn from_view(view: &dyn ObjectView, _name: &str, _index: usize, _spec: FieldSpec) -> FlagsWire {
        FlagsWire {
            asleep: int(view, "asleep", 0) != 0,
            frozen: int(view, "frozen", 0) != 0,
            held: int(view, "held", 0) != 0,
        }
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Bool(self.asleep));
    }
}

// ANCHOR: ghost
/// A prop as a client is sent it.
#[derive(Debug, Clone, PartialEq, Default, torque_engine::GhostRecord)]
pub struct PropGhost {
    /// The `PropData` datablock.
    pub data_block: DatablockRef,
    /// Its slot, whether it is the client's own, its predict id.
    #[net(mask = MASK_INIT)]
    pub init: Option<InitWire>,
    /// The pose and the speeds, at the wire's precision (the server's state exactly).
    #[net(mask = MASK_STATE)]
    pub state: Option<StateWire>,
    /// Asleep, frozen, held.
    #[net(mask = MASK_FLAGS)]
    pub flags: Option<FlagsWire>,
}
// ANCHOR_END: ghost

#[cfg(test)]
#[path = "records_tests.rs"]
mod tests;
