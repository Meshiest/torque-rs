//! TGE's control tails: the packet data (`writePacketData` / `readPacketData`) a control object sends in the control section of
//! every packet, as `torque_net::control::ControlData`, and [`TgeTails`], the class-directed reader (`ControlReader`).
//!
//! ```text
//! ShapeBase:      F32 energy, F32 recharge rate
//! Camera:         torque_net::control::CameraPacketData (the ShapeBase part, then the camera's own)
//! Player:         ShapeBase; int(3) state; flag(state == Recover): int(7) recover ticks; flag(jump delay != 0): int(7);
//!                 flag(not mounted): F32 x3 position (the stream's compression point is set to it), F32 x3 velocity,
//!                 int(4) ticks since the last jump surface contact (at most 15);
//!                 F32 head x, F32 head z, F32 rotation z;
//!                 flag(control object ghosted): int(GB) index, then that object's own packet data
//! Vehicle:        ShapeBase; F32 x2 steering, F32 x3 position, F32 x4 rotation, F32 x3 linear momentum, F32 x3 angular
//!                 momentum, flag at rest, flag(no contacts), flag disable move; then the stream's compression point is set to
//!                 the position (FlyingVehicle and HoverVehicle send this)
//! WheeledVehicle: Vehicle; flag braking; per wheel (the datablock's count) F32 angular velocity, F32 Dy, F32 Dx, flag slipping
//! ```
//! A Player's nested packet data (a vehicle it drives) is read by class like the head's own; the nesting depth is capped at
//! [`MAX_NESTED_TAILS`].
//!
//! **The frame** (the engine's own protocol with the scene hierarchy's block section on, never Torque's): after the control
//! object's packet data and what is nested in it, `flag(in a parent's frame)`; when set, the parent's ghost index (GB bits)
//! and `flag(node)` with a 16-bit node index. In a parent's frame a Player's position, velocity and yaw are its local pose's
//! ([`TailFrame`]), so a client predicts the child on its parent without knowing where the parent went since.
//!
//! **View fields** of the packers: `energy`, `recharge_rate`; Player: `state`, `recover_ticks`, `jump_delay`, `mounted`,
//! `position` 0..3, `velocity` 0..3, `jump_surface_last_contact`, `head` 0..3, `rotation` 0..3, the link `control_object`;
//! Vehicle: `steering` 0..2, `position` 0..3, `rotation` 0..4, `linear_momentum` 0..3, `angular_momentum` 0..3, `at_rest`,
//! `contact_count`, `disable_move`; WheeledVehicle: `braking`, `wheel_count`, per wheel `wheel_avel`, `wheel_dy`, `wheel_dx`,
//! `wheel_slipping`.
//!
//! Credit: these follow TGE 1.4's `writePacketData` / `readPacketData` in `game/shapeBase.cc`, `game/player.cc`,
//! `game/vehicles/vehicle.cc` and `game/vehicles/wheeledVehicle.cc` (Copyright (C) GarageGames.com, Inc.), whose MIT-licensed
//! successors are Torque3D's `T3D/shapeBase.cpp` and `T3D/player.cpp` (each Copyright (c) 2012 GarageGames, LLC; Copyright
//! (C) 2015 Faust Logic, Inc.), `T3D/vehicles/vehicle.cpp` and `T3D/vehicles/wheeledVehicle.cpp` (each Copyright (c) 2012
//! GarageGames, LLC). The layouts are TGE's; the code is this crate's own.

use std::any::Any;
use std::sync::OnceLock;

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::control::{CameraPacketData, ControlData, ControlReader};
use torque_net::huffman::Table;

use crate::codec::{
    Codec, Enc, F32, Layout, NetWidths, R, ReadCtx, WIRE_WHEELS, decode_into, mask_bits, opt,
};
use crate::player::{RECOVER_DELAY_BITS, RECOVER_STATE, STATE_BITS};
use crate::{ChargeFn, Link, ObjectView, boolean, int, pay, raw, raws};

/// The deepest chain of nested packet data (a control object's control object) a reader follows.
pub const MAX_NESTED_TAILS: usize = 4;
pub const JUMP_DELAY_BITS: u8 = 7;
pub const LAST_CONTACT_BITS: u8 = 4;

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(Table::torque)
}

/// The widths a tail is written with: only the ghost index travels in one.
fn tail_widths(ghost_bits: u8) -> NetWidths {
    NetWidths::new(ghost_bits, NetWidths::engine_default().datablock_bits)
}

/// ShapeBase's packet data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShapeTail {
    pub energy: F32,
    pub recharge_rate: F32,
}

impl ShapeTail {
    pub fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.raw32("tail.energy", &mut self.energy.0)?;
        c.raw32("tail.rechargeRate", &mut self.recharge_rate.0)
    }

    pub fn pack(view: &dyn ObjectView) -> ShapeTail {
        ShapeTail {
            energy: raw(view, "energy", 0),
            recharge_rate: raw(view, "recharge_rate", 0),
        }
    }
}

impl Layout for ShapeTail {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        ShapeTail::layout(self, c)
    }
}

/// A Player's position block (sent when it is not mounted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayerTailMotion {
    pub position: [F32; 3],
    pub velocity: [F32; 3],
    pub last_contact: u32,
}

/// Player's packet data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayerTail {
    pub shape: ShapeTail,
    pub state: u32,
    pub recover_ticks: Option<u32>,
    pub jump_delay: Option<u32>,
    pub motion: Option<PlayerTailMotion>,
    /// Head x, head z.
    pub head: [F32; 2],
    pub rotation_z: F32,
    /// The ghost index of the Player's own control object; its packet data follows.
    pub control: Option<u32>,
}

impl Layout for PlayerTail {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        c.int("tail.state", &mut self.state, STATE_BITS)?;
        opt(c, "tail.recover?", &mut self.recover_ticks, |c, t| {
            c.int("tail.recover", t, RECOVER_DELAY_BITS)
        })?;
        opt(c, "tail.jumpDelay?", &mut self.jump_delay, |c, t| {
            c.int("tail.jumpDelay", t, JUMP_DELAY_BITS)
        })?;
        opt(c, "tail.unmounted?", &mut self.motion, |c, m| {
            c.floats("tail.position", &mut m.position)?;
            c.set_compression_point(m.position.map(F32::get));
            c.floats("tail.velocity", &mut m.velocity)?;
            c.int("tail.lastContact", &mut m.last_contact, LAST_CONTACT_BITS)
        })?;
        c.raw32("tail.headX", &mut self.head[0].0)?;
        c.raw32("tail.headZ", &mut self.head[1].0)?;
        c.raw32("tail.rotationZ", &mut self.rotation_z.0)?;
        let bits = c.ghost_bits();
        opt(c, "tail.control?", &mut self.control, |c, g| {
            c.int("tail.control", g, bits)?;
            *g = mask_bits(*g, bits);
            Ok(())
        })
    }
}

impl PlayerTail {
    pub fn pack(view: &dyn ObjectView, ghost_bits: u8) -> PlayerTail {
        let state = mask_bits(int(view, "state", 0) as u32, STATE_BITS);
        let jump_delay = int(view, "jump_delay", 0);
        let last_contact = int(view, "jump_surface_last_contact", 0);
        PlayerTail {
            shape: ShapeTail::pack(view),
            state,
            recover_ticks: (state == RECOVER_STATE)
                .then(|| mask_bits(int(view, "recover_ticks", 0) as u32, RECOVER_DELAY_BITS)),
            jump_delay: (jump_delay != 0).then(|| mask_bits(jump_delay as u32, JUMP_DELAY_BITS)),
            motion: (!boolean(view, "mounted", 0)).then(|| PlayerTailMotion {
                position: raws::<3>(view, "position", 0),
                velocity: raws::<3>(view, "velocity", 0),
                last_contact: mask_bits(last_contact.min(15) as u32, LAST_CONTACT_BITS),
            }),
            head: [raw(view, "head", 0), raw(view, "head", 2)],
            rotation_z: raw(view, "rotation", 2),
            control: match view.link("control_object") {
                Link::Ghost(g) => Some(mask_bits(g, ghost_bits)),
                _ => None,
            },
        }
    }
}

/// Vehicle's packet data (FlyingVehicle's and HoverVehicle's too).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VehicleTail {
    pub shape: ShapeTail,
    pub steering: [F32; 2],
    pub position: [F32; 3],
    pub rotation: [F32; 4],
    pub linear_momentum: [F32; 3],
    pub angular_momentum: [F32; 3],
    pub at_rest: bool,
    pub no_contacts: bool,
    pub disable_move: bool,
}

impl Layout for VehicleTail {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.shape.layout(c)?;
        c.floats("tail.steering", &mut self.steering)?;
        c.floats("tail.position", &mut self.position)?;
        c.floats("tail.rotation", &mut self.rotation)?;
        c.floats("tail.linearMomentum", &mut self.linear_momentum)?;
        c.floats("tail.angularMomentum", &mut self.angular_momentum)?;
        c.flag("tail.atRest", &mut self.at_rest)?;
        c.flag("tail.noContacts", &mut self.no_contacts)?;
        c.flag("tail.disableMove", &mut self.disable_move)?;
        c.set_compression_point(self.position.map(F32::get));
        Ok(())
    }
}

impl VehicleTail {
    pub fn pack(view: &dyn ObjectView) -> VehicleTail {
        VehicleTail {
            shape: ShapeTail::pack(view),
            steering: raws::<2>(view, "steering", 0),
            position: raws::<3>(view, "position", 0),
            rotation: raws::<4>(view, "rotation", 0),
            linear_momentum: raws::<3>(view, "linear_momentum", 0),
            angular_momentum: raws::<3>(view, "angular_momentum", 0),
            at_rest: boolean(view, "at_rest", 0),
            no_contacts: int(view, "contact_count", 0) == 0,
            disable_move: boolean(view, "disable_move", 0),
        }
    }
}

/// One wheel of a WheeledVehicle's packet data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WheelTail {
    pub avel: F32,
    pub dy: F32,
    pub dx: F32,
    pub slipping: bool,
}

/// WheeledVehicle's packet data. The wheel count is not on the wire: a reader starts from a record with the datablock's count
/// of wheels.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WheeledTail {
    pub vehicle: VehicleTail,
    pub braking: bool,
    pub wheels: Vec<WheelTail>,
}

impl Layout for WheeledTail {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.vehicle.layout(c)?;
        c.flag("tail.braking", &mut self.braking)?;
        self.wheels.truncate(WIRE_WHEELS);
        for w in self.wheels.iter_mut() {
            c.raw32("tail.wheel.avel", &mut w.avel.0)?;
            c.raw32("tail.wheel.dy", &mut w.dy.0)?;
            c.raw32("tail.wheel.dx", &mut w.dx.0)?;
            c.flag("tail.wheel.slipping", &mut w.slipping)?;
        }
        Ok(())
    }
}

impl WheeledTail {
    pub fn pack(view: &dyn ObjectView) -> WheeledTail {
        let count = (int(view, "wheel_count", 0).max(0) as usize).min(WIRE_WHEELS);
        WheeledTail {
            vehicle: VehicleTail::pack(view),
            braking: boolean(view, "braking", 0),
            wheels: (0..count)
                .map(|i| WheelTail {
                    avel: raw(view, "wheel_avel", i),
                    dy: raw(view, "wheel_dy", i),
                    dx: raw(view, "wheel_dx", i),
                    slipping: boolean(view, "wheel_slipping", i),
                })
                .collect(),
        }
    }
}

/// The packet data of a TGE ShapeBase class other than the Camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailData {
    Shape(ShapeTail),
    Player(PlayerTail),
    Vehicle(VehicleTail),
    Wheeled(WheeledTail),
}

impl TailData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        match self {
            TailData::Shape(t) => t.layout(c),
            TailData::Player(t) => t.layout(c),
            TailData::Vehicle(t) => t.layout(c),
            TailData::Wheeled(t) => t.layout(c),
        }
    }

    /// The object's position as its data carries it (a mounted Player's and a plain ShapeBase's are not on the wire: zero).
    fn position(&self) -> [f32; 3] {
        match self {
            TailData::Shape(_) => [0.0; 3],
            TailData::Player(p) => p.motion.map_or([0.0; 3], |m| m.position.map(F32::get)),
            TailData::Vehicle(v) => v.position.map(F32::get),
            TailData::Wheeled(w) => w.vehicle.position.map(F32::get),
        }
    }
}

/// The frame a control object's packet data is in (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TailFrame {
    /// Torque's protocol: no frame on the wire, the values are the world's.
    #[default]
    Absent,
    /// The world's, on a connection that carries frames.
    World,
    /// The parent's whose ghost index is `ghost`, at its shape's node `node` (`None`: its origin): a Player's position,
    /// velocity and yaw are its local pose's.
    Parent { ghost: u32, node: Option<u16> },
}

impl TailFrame {
    fn write(self, w: &mut BitWriter, ghost_bits: u8) {
        match self {
            TailFrame::Absent => {}
            TailFrame::World => w.write_flag(false),
            TailFrame::Parent { ghost, node } => {
                w.write_flag(true);
                w.write_bits(ghost, ghost_bits);
                w.write_flag(node.is_some());
                if let Some(node) = node {
                    w.write_bits(u32::from(node), crate::block::NODE_BITS);
                }
            }
        }
    }

    /// Reads a frame, charging its bytes to `charge` as a record's are charged.
    fn read(
        r: &mut BitReader,
        ghost_bits: u8,
        charge: &mut ChargeFn,
    ) -> Result<TailFrame, NetError> {
        let start = r.position();
        let frame = TailFrame::read_bits(r, ghost_bits)?;
        pay(charge, (r.position() - start).div_ceil(8).max(1) as u64)?;
        Ok(frame)
    }

    fn read_bits(r: &mut BitReader, ghost_bits: u8) -> Result<TailFrame, NetError> {
        if !r.read_flag()? {
            return Ok(TailFrame::World);
        }
        let ghost = r.read_bits(ghost_bits)?;
        let node = if r.read_flag()? {
            // Read in 16 bits: the cast keeps every bit.
            Some(r.read_bits(crate::block::NODE_BITS)? as u16)
        } else {
            None
        };
        Ok(TailFrame::Parent { ghost, node })
    }
}

/// A TGE control object's packet data, with the nested packet data of a Player's own control object.
#[derive(Debug, Clone)]
pub struct TgePacketData {
    pub data: TailData,
    /// The object's world translation, which the control section sends raw when the client's checksum matches.
    pub position: [f32; 3],
    /// After a Player's control index: that object's packet data. The host fills it whenever the Player's data names a control
    /// object (`writePacketData` follows the index with the object's own data); without it only the index is written.
    pub nested: Option<Box<dyn ControlData>>,
    /// The frame its values are in, written after everything else (module docs).
    pub frame: TailFrame,
}

impl TgePacketData {
    /// Packet data with the position its data carries, nothing nested and no frame.
    pub fn new(data: TailData) -> TgePacketData {
        let position = data.position();
        TgePacketData {
            data,
            position,
            nested: None,
            frame: TailFrame::Absent,
        }
    }
}

impl PartialEq for TgePacketData {
    fn eq(&self, other: &TgePacketData) -> bool {
        self.data == other.data
            && self.position.map(f32::to_bits) == other.position.map(f32::to_bits)
            && self.frame == other.frame
            && match (&self.nested, &other.nested) {
                (None, None) => true,
                (Some(a), Some(b)) => a.same_as(b.as_ref()),
                _ => false,
            }
    }
}

impl ControlData for TgePacketData {
    fn position(&self) -> [f32; 3] {
        self.position
    }

    fn write(&self, w: &mut BitWriter, ghost_bits: u8) {
        let mut data = self.data.clone();
        let mut c = Enc::new(w, table(), tail_widths(ghost_bits));
        // The encoder is total.
        let _ = data.layout(&mut c);
        if let (
            TailData::Player(PlayerTail {
                control: Some(_), ..
            }),
            Some(n),
        ) = (&self.data, &self.nested)
        {
            n.write(w, ghost_bits);
        }
        self.frame.write(w, ghost_bits);
    }

    fn box_clone(&self) -> Box<dyn ControlData> {
        Box::new(self.clone())
    }

    fn same_as(&self, other: &dyn ControlData) -> bool {
        (other as &dyn Any)
            .downcast_ref::<TgePacketData>()
            .is_some_and(|o| o == self)
    }
}

/// The class of a control object's packet data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailClass {
    /// A ShapeBase class with no packet data of its own (StaticShape, Item, ...).
    Shape,
    Camera,
    Player,
    /// FlyingVehicle and HoverVehicle.
    Vehicle,
    /// A WheeledVehicle with its datablock's wheel count.
    Wheeled {
        wheels: usize,
    },
}

/// The class-directed reader: the class of the object at a ghost index comes from `class_of` (the client's ghost table).
pub struct TgeTails<'a> {
    pub class_of: &'a dyn Fn(u32) -> Option<TailClass>,
    /// The packet data ends with its frame ([`TailFrame`]): the engine's own protocol with the scene hierarchy's section on.
    pub frames: bool,
}

impl<'a> TgeTails<'a> {
    pub fn new(class_of: &'a dyn Fn(u32) -> Option<TailClass>) -> TgeTails<'a> {
        TgeTails {
            class_of,
            frames: false,
        }
    }

    /// The reader of a connection whose packet data ends with its frame when `frames`.
    #[must_use]
    pub fn with_frames(mut self, frames: bool) -> TgeTails<'a> {
        self.frames = frames;
        self
    }

    /// Reads the packet data of the object at `index`, following a Player's nested data up to [`MAX_NESTED_TAILS`] deep.
    pub fn read(
        &self,
        index: u32,
        r: &mut BitReader,
        ghost_bits: u8,
        charge: &mut ChargeFn,
        depth: usize,
    ) -> Result<Box<dyn ControlData>, NetError> {
        if depth >= MAX_NESTED_TAILS {
            return Err(NetError::Invalid("control objects nested too deep"));
        }
        let class =
            (self.class_of)(index).ok_or(NetError::Invalid("control object of unknown class"))?;
        let start = match class {
            TailClass::Camera => {
                return Ok(Box::new(CameraPacketData::read(r, ghost_bits, charge)?));
            }
            TailClass::Shape => TailData::Shape(ShapeTail::default()),
            TailClass::Player => TailData::Player(PlayerTail::default()),
            TailClass::Vehicle => TailData::Vehicle(VehicleTail::default()),
            TailClass::Wheeled { wheels } => {
                let wheels =
                    torque_net::cap::count("wheels", wheels as u64, WIRE_WHEELS, usize::MAX, 0)?;
                TailData::Wheeled(WheeledTail {
                    wheels: vec![WheelTail::default(); wheels],
                    ..WheeledTail::default()
                })
            }
        };
        let ctx = ReadCtx::new(tail_widths(ghost_bits), false);
        let data = decode_into(TailWrap(start), r, table(), charge, &ctx)?.0;
        let nested = match &data {
            TailData::Player(PlayerTail {
                control: Some(i), ..
            }) => Some(self.read(*i, r, ghost_bits, charge, depth + 1)?),
            _ => None,
        };
        let position = data.position();
        let frame = if self.frames && depth == 0 {
            TailFrame::read(r, ghost_bits, charge)?
        } else {
            TailFrame::Absent
        };
        Ok(Box::new(TgePacketData {
            data,
            position,
            nested,
            frame,
        }))
    }
}

impl ControlReader for TgeTails<'_> {
    fn read_control(
        &mut self,
        index: u32,
        r: &mut BitReader,
        ghost_bits: u8,
        charge: &mut ChargeFn,
    ) -> Result<Box<dyn ControlData>, NetError> {
        self.read(index, r, ghost_bits, charge, 0)
    }
}

/// [`TailData`] as a [`Layout`] (its default is a ShapeBase tail; a reader starts from the class's own).
#[derive(Debug, Clone)]
struct TailWrap(TailData);

impl Default for TailWrap {
    fn default() -> TailWrap {
        TailWrap(TailData::Shape(ShapeTail::default()))
    }
}

impl Layout for TailWrap {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.0.layout(c)
    }
}
