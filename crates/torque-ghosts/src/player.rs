//! TGE's `Player` (and `AIPlayer`, which sends Player's record) over [`TgeShapeBase`].
//!
//! ```text
//! ShapeBase part
//! flag(Impact, bit 27, && !initial): int(3) impact sound
//! flag(Action, bit 25, && action != NullAnimation && action >= NumTableActionAnims):
//!     int(9) action, flag holdAtEnd, flag atEnd, flag firstPerson; when !atEnd: flag(initial && position > 0): int(6) the
//!     thread position (writeSignedFloat)
//! flag(Action && arm action != NullAnimation && (!initial || arm action != the datablock's look action)): int(9) arm action
//! flag(controlled by this connection && !initial): the record ends
//! flag(Move, bit 26): flag falling, int(3) state, flag(state == Recover): int(7) recover ticks; compressed point (position);
//!     velocity block (flag(len > 0.02): normal vector (10), int(13) speed); int(7) yaw (writeFloat of rot z / 2 pi);
//!     int(6) head x, int(6) head z (writeSignedFloat of head / maxLookAngle); the move (Move::pack); flag(!NoWarp)
//! int(5) energy (writeFloat of energy / maxEnergy)
//! ```
//! `NullAnimation` is 511 and `NumTableActionAnims` 8 (root, run forward, back, side, fall, jump, stand jump, land): the table
//! actions follow from the move and are not sent.
//!
//! **View fields:** the ShapeBase part's; `controlled_here` (this connection controls it), `impact_sound`, `action`,
//! `action_hold`, `action_at_end`, `action_first_person`, `action_position` (the action thread's position), `arm_action`,
//! `look_action`, `falling`, `state`, `recover_ticks`, `position` 0..3, `velocity` 0..3, `rotation` 0..3, `head` 0..3,
//! `max_look_angle`, the move ([`PackedMove::from_view`]), `energy`, `max_energy`.
//!
//! Credit: this follows TGE 1.4's `game/player.cc` (Copyright (C) GarageGames.com, Inc.), whose MIT-licensed successor is
//! Torque3D's `T3D/player.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The layout is
//! TGE's: Torque3D's pose, swimming and jetting fields are not part of it. The code is this crate's own.

use crate::codec::{Codec, Layout, R, mask_bits, opt};
use crate::motion::{CPoint, PackedMove, Velocity, float_bits, signed_float_bits};
use crate::parts::mask::INITIAL;
use crate::shapebase::{ENERGY_LEVEL_BITS, TgeShapeBase, mask::NO_WARP, pack_shapebase};
use crate::{ObjectView, PackCtx, boolean, fld, int, raws};

/// Player's mask bits.
pub mod mask {
    pub const ACTION: u32 = 1 << 25;
    pub const MOVE: u32 = 1 << 26;
    pub const IMPACT: u32 = 1 << 27;
}

pub const IMPACT_BITS: u8 = 3;
pub const ACTION_ANIM_BITS: u8 = 9;
/// No animation (`NullAnimation`).
pub const NULL_ANIMATION: u32 = (1 << ACTION_ANIM_BITS) - 1;
/// The action table's animations, chosen by the move on both sides (`NumTableActionAnims`).
pub const NUM_TABLE_ACTION_ANIMS: u32 = 8;
pub const STATE_BITS: u8 = 3;
/// `RecoverState`.
pub const RECOVER_STATE: u32 = 2;
pub const RECOVER_DELAY_BITS: u8 = 7;
pub const VELOCITY_NORMAL_BITS: u8 = 10;
pub const YAW_BITS: u8 = 7;
pub const HEAD_BITS: u8 = 6;
pub const ACTION_POSITION_BITS: u8 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Action {
    pub action: u32,
    pub hold_at_end: bool,
    pub at_end: bool,
    pub first_person: bool,
    /// When not at the end: `flag(initial && position > 0)` and the position (writeSignedFloat, 6 bits).
    pub position: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayerMove {
    pub falling: bool,
    pub state: u32,
    pub recover_ticks: Option<u32>,
    pub position: CPoint,
    pub velocity: Option<Velocity>,
    pub yaw: u32,
    /// Head x and z.
    pub head: [u32; 2],
    pub mv: PackedMove,
    /// `flag(!(mask & NoWarp))`.
    pub warp: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Player {
    pub shape: TgeShapeBase,
    pub impact: Option<u32>,
    pub action: Option<Action>,
    pub arm: Option<u32>,
    /// Controlled by this connection and not an initial update: nothing follows.
    pub controlled: bool,
    pub motion: Option<PlayerMove>,
    /// `writeFloat(energy / maxEnergy, 5)`; not sent to the controlling connection.
    pub energy: u32,
}

impl Layout for Player {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        opt(c, "player.impact?", &mut self.impact, |c, v| {
            c.int("player.impact", v, IMPACT_BITS)
        })?;
        opt(c, "player.action?", &mut self.action, |c, a| {
            c.int("player.action", &mut a.action, ACTION_ANIM_BITS)?;
            c.flag("player.holdAtEnd", &mut a.hold_at_end)?;
            c.flag("player.atEnd", &mut a.at_end)?;
            c.flag("player.firstPerson", &mut a.first_person)?;
            if a.at_end {
                a.position = None;
                return Ok(());
            }
            opt(c, "player.actionPosition?", &mut a.position, |c, p| {
                c.int("player.actionPosition", p, ACTION_POSITION_BITS)
            })
        })?;
        opt(c, "player.arm?", &mut self.arm, |c, v| {
            c.int("player.arm", v, ACTION_ANIM_BITS)
        })?;
        c.flag("player.controlled", &mut self.controlled)?;
        if self.controlled {
            self.motion = None;
            self.energy = 0;
            return Ok(());
        }
        opt(c, "player.move?", &mut self.motion, |c, m| {
            c.flag("player.falling", &mut m.falling)?;
            c.int("player.state", &mut m.state, STATE_BITS)?;
            opt(c, "player.recover?", &mut m.recover_ticks, |c, t| {
                c.int("player.recover", t, RECOVER_DELAY_BITS)
            })?;
            m.position.layout(c)?;
            opt(c, "player.velocity?", &mut m.velocity, |c, v| {
                v.layout(c, VELOCITY_NORMAL_BITS)
            })?;
            c.int("player.yaw", &mut m.yaw, YAW_BITS)?;
            c.int("player.headX", &mut m.head[0], HEAD_BITS)?;
            c.int("player.headZ", &mut m.head[1], HEAD_BITS)?;
            m.mv.layout(c)?;
            c.flag("player.warp", &mut m.warp)
        })?;
        c.int("player.energy", &mut self.energy, ENERGY_LEVEL_BITS)
    }
}

/// Packs a Player: the record and the return mask (the ShapeBase part's).
pub fn pack_player(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (Player, u32) {
    let (shape, ret) = pack_shapebase(view, mask, ctx);
    let initial = mask & INITIAL != 0;
    let impact = (mask & self::mask::IMPACT != 0 && !initial)
        .then(|| mask_bits(int(view, "impact_sound", 0) as u32, IMPACT_BITS));
    let action_id = int(view, "action", 0) as u32;
    let action = (mask & self::mask::ACTION != 0
        && action_id != NULL_ANIMATION
        && action_id >= NUM_TABLE_ACTION_ANIMS)
        .then(|| {
            let at_end = boolean(view, "action_at_end", 0);
            let position = fld(view, "action_position", 0);
            Action {
                action: mask_bits(action_id, ACTION_ANIM_BITS),
                hold_at_end: boolean(view, "action_hold", 0),
                at_end,
                first_person: boolean(view, "action_first_person", 0),
                position: (!at_end && initial && position > 0.0)
                    .then(|| signed_float_bits(position, ACTION_POSITION_BITS)),
            }
        });
    let arm_id = int(view, "arm_action", 0) as u32;
    let arm = (mask & self::mask::ACTION != 0
        && arm_id != NULL_ANIMATION
        && (!initial || arm_id != int(view, "look_action", 0) as u32))
        .then(|| mask_bits(arm_id, ACTION_ANIM_BITS));
    let controlled = boolean(view, "controlled_here", 0) && !initial;
    if controlled {
        return (
            Player {
                shape,
                impact,
                action,
                arm,
                controlled,
                motion: None,
                energy: 0,
            },
            ret,
        );
    }
    let motion = (mask & self::mask::MOVE != 0).then(|| {
        let state = mask_bits(int(view, "state", 0) as u32, STATE_BITS);
        let max_look = fld(view, "max_look_angle", 0);
        let yaw = (f64::from(fld(view, "rotation", 2)) / std::f64::consts::TAU) as f32;
        PlayerMove {
            falling: boolean(view, "falling", 0),
            state,
            recover_ticks: (state == RECOVER_STATE)
                .then(|| mask_bits(int(view, "recover_ticks", 0) as u32, RECOVER_DELAY_BITS)),
            position: CPoint::pack(
                raws::<3>(view, "position", 0).map(|f| f.get()),
                ctx.compression_point,
            ),
            velocity: Velocity::pack(
                raws::<3>(view, "velocity", 0).map(|f| f.get()),
                VELOCITY_NORMAL_BITS,
            ),
            yaw: float_bits(yaw, YAW_BITS),
            head: [
                signed_float_bits(fld(view, "head", 0) / max_look, HEAD_BITS),
                signed_float_bits(fld(view, "head", 2) / max_look, HEAD_BITS),
            ],
            mv: PackedMove::from_view(view),
            warp: mask & NO_WARP == 0,
        }
    });
    let energy = float_bits(
        fld(view, "energy", 0) / fld(view, "max_energy", 0),
        ENERGY_LEVEL_BITS,
    );
    (
        Player {
            shape,
            impact,
            action,
            arm,
            controlled,
            motion,
            energy,
        },
        ret,
    )
}
