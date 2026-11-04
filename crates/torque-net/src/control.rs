//! The control section of a game packet: the server's head, the client's block, the control object's packet data (Torque's
//! Camera by default), the checksum, and the per-connection state behind them.
//!
//! **Server head** (what [`ServerHead::write`] puts first in every data packet, after the rate prefix):
//! 1. `u32` move ack (`lastMoveAck - list size`).
//! 2. `flag(flash != 0 || whiteOut != 0)`; if set `flag(flash != 0)` [+ `writeFloat(flash, 7)`] and `flag(whiteOut != 0)` [+
//!    `writeFloat(f32(whiteOut / 1.5), 7)`]. No control object: `flag(0)`.
//! 3. `flag(ghostIndex(control) != -1)`; if set `flag(checksum != the client's last report)`; differs: `int(GB, index)` and the
//!    object's packet data; equal: the position as three raw floats. Either sets the stream's compression point (the control
//!    object's position). `getGhostIndex` is -1 until the ghost record is acked, so a control object that is not yet ghosted writes
//!    `flag(0)`.
//! 4. `flag(camera object ghosted)` [+ `int(GB)` and the camera object's packet data]: a camera object other than the control
//!    object.
//! 5. `flag(first person update)` [+ `flag(first person)`].
//! 6. `flag(fov update)` [+ `int(8, clamp(trunc(fov), 1, 179))`].
//!
//! **Client block** ([`ClientBlock`]): `flag(camera distance == 0)`, the `u32` control checksum, the move block, then the
//! first-person pair and the fov pair as in steps 5 and 6.
//!
//! **Format.** A game may leave the camera object's packet data out of step 4 (the index alone) and the first-person pair out of
//! the head and the block, and set its moves' form: a [`ControlFormat`], Torque's by default, which both sides must share.
//!
//! **Camera packet data** (`Camera::writePacketData`, a [`ControlData`]): the `ShapeBase` part, `F32` energy and `F32` recharge
//! rate; the position, the stream's compression point being set to it BEFORE it is written; `F32` pitch, `F32` yaw;
//! `ranged(mode, 0, 4)`; for the orbit modes the three `F32` distances, then mode 3 `flag` + `int(GB, target)` and mode 4
//! `writeCompressedPoint(point, 0.01)`. A fly camera is 227 bits. A game's control objects bring their own [`ControlData`] and
//! [`ControlReader`].
//!
//! **Checksum.** The packet data written into a 12,000-bit stream with no string buffer and a zero compression point, `(bits + 7) >> 3`
//! bytes (the unused bits are zero), the reflected CRC-32 (0xEDB88320) from `0xFFFFFFFF` and NO final xor ([`checksum`]).
//!
//! **Degradations** ([`CameraState::resolve`]): mode 3 with an orbit object that is not ghosted is written as mode 4 with the
//! object's world-box centre, as Torque does. **Addition:** mode 3 with no orbit object at all (Torque would read a null object)
//! is written as mode 2.
//!
//! **Hostile input.** The readers take bytes and a charge; a camera mode above 4 is an error.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/gameBase/gameConnection.cpp` (Copyright
//! (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.), `T3D/camera.cpp` (Copyright (c) 2012 GarageGames, LLC) and
//! `T3D/shapeBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this
//! crate's own.

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::NET_MOVE_CREDIT_MAX;
use crate::crc::Crc32;
use crate::events::ChargeFn;
use crate::library;
use crate::moves::{Absorbed, MoveBlock, MoveFormat, MoveList};

/// The scale of mode 4's compressed point.
pub const CAMERA_POINT_SCALE: f32 = 0.01;

/// The checksum stream's capacity in bits: a datagram's 1,500 bytes (`Net::MaxPacketDataSize`).
pub const CHECKSUM_STREAM_BITS: usize = 12_000;

/// The valid range of a field of view on the wire.
pub const FOV_MIN: i32 = 1;
pub const FOV_MAX: i32 = 179;

/// The highest camera mode (`CameraLastMode`, orbit point).
pub const CAMERA_LAST_MODE: u32 = 4;

/// What a game changes about the control section's form (see the module docs): Torque's by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlFormat {
    /// The moves of the client's block.
    pub moves: MoveFormat,
    /// The head and the client's block carry the first-person pair (step 5).
    pub first_person: bool,
    /// Step 4 carries the camera object's packet data after its index.
    pub camera_data: bool,
}

impl Default for ControlFormat {
    fn default() -> ControlFormat {
        ControlFormat {
            moves: MoveFormat::default(),
            first_person: true,
            camera_data: true,
        }
    }
}

// Packet data

/// What a control object writes for `writePacketData` and the equal-checksum branch needs. The Camera ([`CameraPacketData`]) is the
/// implementation in this crate; a game's Player or vehicle plugs in here. The object is reached through its own virtual
/// `writePacketData` and checksum, so the head holds a `Box<dyn ControlData>`.
///
/// The reading side is class-directed (the client knows the control object's class from its ghost): a [`ControlReader`] picks the
/// layout by ghost index; this crate's [`CameraReader`] reads every object as a Camera.
pub trait ControlData: std::fmt::Debug + std::any::Any + Send + Sync {
    /// The position the equal branch sends raw (the object's world translation).
    fn position(&self) -> [f32; 3];
    /// `writePacketData`: sets the writer's compression point itself.
    fn write(&self, w: &mut BitWriter, ghost_bits: u8);
    /// A copy behind the trait.
    fn box_clone(&self) -> Box<dyn ControlData>;
    /// Equal to `other` (same type and same data).
    fn same_as(&self, other: &dyn ControlData) -> bool;
}

/// `ControlData` behind a box is `ControlData` (a head built from [`ControlObject::packet_data`] holds one).
impl ControlData for Box<dyn ControlData> {
    fn position(&self) -> [f32; 3] {
        (**self).position()
    }
    fn write(&self, w: &mut BitWriter, ghost_bits: u8) {
        (**self).write(w, ghost_bits);
    }
    fn box_clone(&self) -> Box<dyn ControlData> {
        (**self).box_clone()
    }
    fn same_as(&self, other: &dyn ControlData) -> bool {
        match (other as &dyn std::any::Any).downcast_ref::<Box<dyn ControlData>>() {
            Some(inner) => (**self).same_as(inner.as_ref()),
            None => (**self).same_as(other),
        }
    }
}

impl Clone for Box<dyn ControlData> {
    fn clone(&self) -> Box<dyn ControlData> {
        (**self).box_clone()
    }
}

impl PartialEq for dyn ControlData {
    fn eq(&self, other: &dyn ControlData) -> bool {
        self.same_as(other)
    }
}

/// The class-directed reading side of `readPacketData` (`GameConnection::readPacket` calls the object's own `readPacketData`): the
/// client knows the class of the object at a ghost index from its ghost table, so the reader is asked by index.
pub trait ControlReader {
    fn read_control(
        &mut self,
        index: u32,
        r: &mut BitReader,
        ghost_bits: u8,
        charge: &mut ChargeFn,
    ) -> Result<Box<dyn ControlData>, NetError>;
}

/// Reads every control object as a Camera.
#[derive(Debug, Clone, Copy, Default)]
pub struct CameraReader;

impl ControlReader for CameraReader {
    fn read_control(
        &mut self,
        _index: u32,
        r: &mut BitReader,
        ghost_bits: u8,
        charge: &mut ChargeFn,
    ) -> Result<Box<dyn ControlData>, NetError> {
        Ok(Box::new(CameraPacketData::read(r, ghost_bits, charge)?))
    }
}

/// The checksum of an object's packet data: see the module docs.
pub fn checksum<D: ControlData + ?Sized>(data: &D, ghost_bits: u8) -> u32 {
    let mut w = BitWriter::with_limit(CHECKSUM_STREAM_BITS);
    data.write(&mut w, ghost_bits);
    let n = w.bits().div_ceil(8);
    let mut crc = Crc32::new();
    crc.update(w.bytes().get(..n).unwrap_or(&[]));
    crc.raw()
}

/// A camera's mode and the data that mode writes, as resolved for the wire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CameraMode {
    /// Modes 0, 1, 2: no data.
    Stationary,
    FreeRotate,
    Fly,
    /// Mode 3: min distance, max distance, current distance, the observing flag and the target's ghost index.
    Orbit {
        distances: [f32; 3],
        flag: bool,
        target: u32,
    },
    /// Mode 4: the three distances and the point, `writeCompressedPoint` at scale 0.01.
    OrbitPoint {
        distances: [f32; 3],
        point: [f32; 3],
    },
}

impl CameraMode {
    /// The number on the wire.
    pub fn number(&self) -> u32 {
        match self {
            CameraMode::Stationary => 0,
            CameraMode::FreeRotate => 1,
            CameraMode::Fly => 2,
            CameraMode::Orbit { .. } => 3,
            CameraMode::OrbitPoint { .. } => 4,
        }
    }
}

/// `Camera::writePacketData` as data.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraPacketData {
    /// The `ShapeBase` part: the energy level and the recharge rate.
    pub energy: f32,
    pub recharge: f32,
    pub position: [f32; 3],
    pub pitch: f32,
    pub yaw: f32,
    pub mode: CameraMode,
}

impl CameraPacketData {
    /// A fly camera at `position`.
    pub fn fly(position: [f32; 3], pitch: f32, yaw: f32) -> CameraPacketData {
        CameraPacketData {
            energy: 0.0,
            recharge: 0.0,
            position,
            pitch,
            yaw,
            mode: CameraMode::Fly,
        }
    }

    fn write_point3(w: &mut BitWriter, p: [f32; 3]) {
        for c in p {
            w.write_f32(c);
        }
    }

    /// Reads packet data; the reader's compression point is set to the position as in Torque. Charges 32 units.
    pub fn read(
        r: &mut BitReader,
        ghost_bits: u8,
        charge: &mut ChargeFn,
    ) -> Result<CameraPacketData, NetError> {
        if !charge(32) {
            return Err(NetError::Invalid("work budget"));
        }
        let read_point3 = |r: &mut BitReader| -> Result<[f32; 3], NetError> {
            Ok([r.read_f32()?, r.read_f32()?, r.read_f32()?])
        };
        let energy = r.read_f32()?;
        let recharge = r.read_f32()?;
        let position = read_point3(r)?;
        r.set_compression_point(position);
        let pitch = r.read_f32()?;
        let yaw = r.read_f32()?;
        let mode = match r.read_ranged_u32(0, CAMERA_LAST_MODE)? {
            0 => CameraMode::Stationary,
            1 => CameraMode::FreeRotate,
            2 => CameraMode::Fly,
            3 => CameraMode::Orbit {
                distances: read_point3(r)?,
                flag: r.read_flag()?,
                target: r.read_bits(ghost_bits)?,
            },
            4 => CameraMode::OrbitPoint {
                distances: read_point3(r)?,
                point: r.read_compressed_point(CAMERA_POINT_SCALE)?,
            },
            _ => return Err(NetError::Invalid("camera mode")),
        };
        Ok(CameraPacketData {
            energy,
            recharge,
            position,
            pitch,
            yaw,
            mode,
        })
    }

    /// The checksum of this data.
    pub fn checksum(&self, ghost_bits: u8) -> u32 {
        checksum(self, ghost_bits)
    }
}

impl ControlData for CameraPacketData {
    fn position(&self) -> [f32; 3] {
        self.position
    }

    fn box_clone(&self) -> Box<dyn ControlData> {
        Box::new(self.clone())
    }

    fn same_as(&self, other: &dyn ControlData) -> bool {
        (other as &dyn std::any::Any)
            .downcast_ref::<CameraPacketData>()
            .is_some_and(|o| o == self)
    }

    fn write(&self, w: &mut BitWriter, ghost_bits: u8) {
        w.write_f32(self.energy);
        w.write_f32(self.recharge);
        w.set_compression_point(self.position);
        CameraPacketData::write_point3(w, self.position);
        w.write_f32(self.pitch);
        w.write_f32(self.yaw);
        w.write_ranged_u32(self.mode.number(), 0, CAMERA_LAST_MODE);
        match &self.mode {
            CameraMode::Stationary | CameraMode::FreeRotate | CameraMode::Fly => {}
            CameraMode::Orbit {
                distances,
                flag,
                target,
            } => {
                CameraPacketData::write_point3(w, *distances);
                w.write_flag(*flag);
                w.write_bits(*target, ghost_bits);
            }
            CameraMode::OrbitPoint { distances, point } => {
                CameraPacketData::write_point3(w, *distances);
                w.write_compressed_point(*point, CAMERA_POINT_SCALE);
            }
        }
    }
}

// The host's view of a camera

/// The object an orbit camera refers to, looked up by id at write time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    /// `getGhostIndex(connection, object)`: `None` until the ghost record is acked.
    pub ghost: Option<u32>,
    /// The object's world box (min and max): the centre stands in for an unghosted orbit target.
    pub box_min: [f32; 3],
    pub box_max: [f32; 3],
}

impl Target {
    /// `Box3F::getCenter` ([`crate::library::MathLibrary::box_centre`]).
    pub fn centre(&self) -> [f32; 3] {
        (library::get().box_centre)(self.box_min, self.box_max)
    }
}

/// A camera's mode as the host holds it, before the degradations of the wire form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModeState {
    Stationary,
    FreeRotate,
    Fly,
    /// Mode 3: `target` is `None` when the camera has no orbit object.
    Orbit {
        distances: [f32; 3],
        flag: bool,
        target: Option<Target>,
    },
    OrbitPoint {
        distances: [f32; 3],
        point: [f32; 3],
    },
}

/// The fields `Camera::writePacketData` reads, as the host holds them.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraState {
    pub energy: f32,
    pub recharge: f32,
    pub position: [f32; 3],
    pub pitch: f32,
    pub yaw: f32,
    pub mode: ModeState,
}

impl CameraState {
    /// The wire form, with the degradations of the module docs applied.
    pub fn resolve(&self) -> CameraPacketData {
        let mode = match self.mode {
            ModeState::Stationary => CameraMode::Stationary,
            ModeState::FreeRotate => CameraMode::FreeRotate,
            ModeState::Fly => CameraMode::Fly,
            ModeState::Orbit {
                distances,
                flag,
                target,
            } => match target {
                None => CameraMode::Fly,
                Some(t) => match t.ghost {
                    Some(index) => CameraMode::Orbit {
                        distances,
                        flag,
                        target: index,
                    },
                    None => CameraMode::OrbitPoint {
                        distances,
                        point: t.centre(),
                    },
                },
            },
            ModeState::OrbitPoint { distances, point } => {
                CameraMode::OrbitPoint { distances, point }
            }
        };
        CameraPacketData {
            energy: self.energy,
            recharge: self.recharge,
            position: self.position,
            pitch: self.pitch,
            yaw: self.yaw,
            mode,
        }
    }
}

/// The clamp applied to a field of view (and to the client's byte): `f > 179` gives 179, `f <= 1` gives 1,
/// else `f`. A NaN is unordered in both compares and so takes the `<= 1` branch: 1.
pub fn clamp_fov(f: f32) -> f32 {
    if f > FOV_MAX as f32 {
        FOV_MAX as f32
    } else if f > FOV_MIN as f32 {
        f
    } else {
        FOV_MIN as f32
    }
}

/// The object at the top of a connection's control chain, as `setControlCameraFov` sees it.
pub trait FovTarget {
    /// `setCameraFov`: the object may clamp to its datablock's range.
    fn set_camera_fov(&mut self, fov: f32);
    /// `getCameraFov`: what the object holds now.
    fn camera_fov(&self) -> f32;
}

/// What the host implements for a connection's control object.
pub trait ControlObject {
    /// `getGhostIndex(connection, object)`: `None` until the object's ghost record is acked.
    fn ghost_index(&self) -> Option<u32>;
    /// The damage flash and white-out (`getDamageFlash`, `getWhiteOut`).
    fn damage(&self) -> Damage;
    /// The object's `writePacketData` as data: a Camera's is [`CameraState::resolve`], a game's own for its classes.
    fn packet_data(&self) -> Box<dyn ControlData>;
}

// The head

/// The damage flash of step 2 as the wire carries it: the 7-bit codes. `None` is a level of zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Damage {
    pub flash: Option<u8>,
    pub white_out: Option<u8>,
}

impl Damage {
    /// From the object's levels: a level that is not 0 is sent (`writeFloat(level, 7)`), the white-out divided by 1.5 first.
    pub fn from_levels(flash: f32, white_out: f32) -> Damage {
        let code = |v: f32| ((library::get().write_float)(v, 7) & 0x7f) as u8;
        Damage {
            flash: (flash != 0.0).then(|| code(flash)),
            white_out: (white_out != 0.0).then(|| code(white_out / 1.5)),
        }
    }
}

/// Step 3.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlSection<D> {
    /// `flag(0)`: no control object, or its ghost is not acked yet.
    None,
    /// The checksum matched: the position, raw.
    Matching { position: [f32; 3] },
    /// The checksum differed: the ghost index and the packet data.
    Resend { index: u32, data: D },
}

/// The server-to-client head of a data packet.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerHead<D> {
    pub move_ack: u32,
    pub damage: Damage,
    pub control: ControlSection<D>,
    /// Step 4: the ghost index of a camera object other than the control object.
    pub camera_object: Option<u32>,
    /// Step 4: the camera object's packet data, under a format that carries it ([`ControlFormat::camera_data`]: there a camera
    /// object without its data is left out).
    pub camera_data: Option<D>,
    /// Step 5: the first-person setting, when it changed.
    pub first_person: Option<bool>,
    /// Step 6: the clamped field of view.
    pub fov: Option<u8>,
}

/// The bits of a head with nothing set: the ack and five flags (four without the first-person pair).
pub const HEAD_MIN_BITS: usize = 37;

fn write_option_flag(w: &mut BitWriter, v: Option<bool>) {
    w.write_flag(v.is_some());
    if let Some(on) = v {
        w.write_flag(on);
    }
}

fn read_option_flag(r: &mut BitReader) -> Result<Option<bool>, NetError> {
    Ok(if r.read_flag()? {
        Some(r.read_flag()?)
    } else {
        None
    })
}

fn write_fov_pair(w: &mut BitWriter, fov: Option<u8>) {
    w.write_flag(fov.is_some());
    if let Some(f) = fov {
        w.write_u8(f);
    }
}

fn read_fov_pair(r: &mut BitReader) -> Result<Option<u8>, NetError> {
    Ok(if r.read_flag()? {
        Some(r.read_u8()?)
    } else {
        None
    })
}

impl<D: ControlData> ServerHead<D> {
    /// Writes the head in Torque's form; leaves the writer's compression point at the control object's position when step 3 sent
    /// one (and at the camera object's when step 4 sent its data).
    pub fn write(&self, w: &mut BitWriter, ghost_bits: u8) {
        self.write_with(w, ghost_bits, ControlFormat::default());
    }

    /// [`ServerHead::write`] in `format`.
    pub fn write_with(&self, w: &mut BitWriter, ghost_bits: u8, format: ControlFormat) {
        w.write_u32(self.move_ack);
        let any = self.damage.flash.is_some() || self.damage.white_out.is_some();
        w.write_flag(any);
        if any {
            for level in [self.damage.flash, self.damage.white_out] {
                w.write_flag(level.is_some());
                if let Some(code) = level {
                    w.write_bits(u32::from(code), 7);
                }
            }
        }
        match &self.control {
            ControlSection::None => w.write_flag(false),
            ControlSection::Matching { position } => {
                w.write_flag(true);
                w.write_flag(false);
                for c in position {
                    w.write_f32(*c);
                }
                w.set_compression_point(*position);
            }
            ControlSection::Resend { index, data } => {
                w.write_flag(true);
                w.write_flag(true);
                w.write_bits(*index, ghost_bits);
                data.write(w, ghost_bits);
            }
        }
        let data = self.camera_data.as_ref().filter(|_| format.camera_data);
        match self.camera_object {
            Some(index) if data.is_some() || !format.camera_data => {
                w.write_flag(true);
                w.write_bits(index, ghost_bits);
                if let Some(data) = data {
                    data.write(w, ghost_bits);
                }
            }
            _ => w.write_flag(false),
        }
        if format.first_person {
            write_option_flag(w, self.first_person);
        }
        write_fov_pair(w, self.fov);
    }
}

impl<D> ServerHead<D> {
    /// Reads a head in `format` whose packet data (steps 3 and 4) is read by `data` (given the ghost index); the reader's
    /// compression point is set as the writer's was.
    pub fn read_in(
        r: &mut BitReader,
        ghost_bits: u8,
        format: ControlFormat,
        charge: &mut ChargeFn,
        data: &mut dyn FnMut(u32, &mut BitReader, &mut ChargeFn) -> Result<D, NetError>,
    ) -> Result<ServerHead<D>, NetError> {
        let move_ack = r.read_u32()?;
        let mut damage = Damage::default();
        if r.read_flag()? {
            if r.read_flag()? {
                damage.flash = Some(r.read_bits(7)? as u8);
            }
            if r.read_flag()? {
                damage.white_out = Some(r.read_bits(7)? as u8);
            }
        }
        let control = if r.read_flag()? {
            if r.read_flag()? {
                let index = r.read_bits(ghost_bits)?;
                let data = data(index, r, charge)?;
                ControlSection::Resend { index, data }
            } else {
                let position = [r.read_f32()?, r.read_f32()?, r.read_f32()?];
                r.set_compression_point(position);
                ControlSection::Matching { position }
            }
        } else {
            ControlSection::None
        };
        let (camera_object, camera_data) = if r.read_flag()? {
            let index = r.read_bits(ghost_bits)?;
            let data = if format.camera_data {
                Some(data(index, r, charge)?)
            } else {
                None
            };
            (Some(index), data)
        } else {
            (None, None)
        };
        let first_person = if format.first_person {
            read_option_flag(r)?
        } else {
            None
        };
        let fov = read_fov_pair(r)?;
        Ok(ServerHead {
            move_ack,
            damage,
            control,
            camera_object,
            camera_data,
            first_person,
            fov,
        })
    }
}

impl ServerHead<CameraPacketData> {
    /// Reads a head whose objects are all Cameras; the reader's compression point is set as the writer's was.
    pub fn read(
        r: &mut BitReader,
        ghost_bits: u8,
        charge: &mut ChargeFn,
    ) -> Result<ServerHead<CameraPacketData>, NetError> {
        let format = ControlFormat::default();
        ServerHead::read_in(r, ghost_bits, format, charge, &mut |_, r, charge| {
            CameraPacketData::read(r, ghost_bits, charge)
        })
    }
}

impl ServerHead<Box<dyn ControlData>> {
    /// Reads a head whose objects are read by `reader`, which picks the layout from the class of the object at the ghost index, as
    /// a client does (`GameConnection::readPacket` calls the object's own `readPacketData`).
    pub fn read_with(
        r: &mut BitReader,
        ghost_bits: u8,
        charge: &mut ChargeFn,
        reader: &mut dyn ControlReader,
    ) -> Result<ServerHead<Box<dyn ControlData>>, NetError> {
        let format = ControlFormat::default();
        ServerHead::read_in(r, ghost_bits, format, charge, &mut |index, r, charge| {
            reader.read_control(index, r, ghost_bits, charge)
        })
    }
}

// The client's block

/// The client-to-server head of a data packet: `flag` camera position, the control checksum, the move block, the first-person
/// pair, the fov pair.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClientBlock {
    /// `flag(camera distance == 0)`; irrelevant to a Camera.
    pub camera_flag: bool,
    pub checksum: u32,
    pub moves: MoveBlock,
    /// `flag` + `flag`: the client's first-person setting, when it changed ([`ControlState::read_client`] applies it).
    pub first_person: Option<bool>,
    /// `flag` + `int(8)`: the client's field of view. The server clamps it to 1..=179 and applies it through
    /// [`ControlState::apply_client_fov`].
    pub fov: Option<u8>,
}

impl ClientBlock {
    /// Writes the block in Torque's format.
    pub fn write(&self, w: &mut BitWriter) {
        self.write_with(w, ControlFormat::default());
    }

    /// [`ClientBlock::write`] in `format` (without the first-person pair when the format has none).
    pub fn write_with(&self, w: &mut BitWriter, format: ControlFormat) {
        w.write_flag(self.camera_flag);
        w.write_u32(self.checksum);
        self.moves.write_with(w, format.moves);
        if format.first_person {
            write_option_flag(w, self.first_person);
        }
        write_fov_pair(w, self.fov);
    }

    /// Reads the block in Torque's format.
    pub fn read(r: &mut BitReader, charge: &mut ChargeFn) -> Result<ClientBlock, NetError> {
        ClientBlock::read_with(r, ControlFormat::default(), charge)
    }

    /// [`ClientBlock::read`] in `format`.
    pub fn read_with(
        r: &mut BitReader,
        format: ControlFormat,
        charge: &mut ChargeFn,
    ) -> Result<ClientBlock, NetError> {
        let camera_flag = r.read_flag()?;
        let checksum = r.read_u32()?;
        let moves = MoveBlock::read_with(r, format.moves, charge)?;
        let first_person = if format.first_person {
            read_option_flag(r)?
        } else {
            None
        };
        let fov = read_fov_pair(r)?;
        Ok(ClientBlock {
            camera_flag,
            checksum,
            moves,
            first_person,
            fov,
        })
    }
}

// A connection's control state

/// How a head's step 3 went, for the per-connection counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step3 {
    None,
    Matching,
    Resend,
}

/// Counters of the control section on a connection (the mismatch rate is what the steady-state check reads).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ControlStats {
    pub none: u64,
    pub matching: u64,
    pub resend: u64,
}

/// The GameConnection fields behind the control section: the move list and its ack (and the move format, the list's), the move
/// credit, the client's last checksum, the first-person setting, the field of view, and their update flags; the section's
/// [`ControlFormat`].
#[derive(Debug, Clone)]
pub struct ControlState {
    pub moves: MoveList,
    format: ControlFormat,
    /// The checksum the client last reported.
    pub client_checksum: u32,
    /// Moves the connection may still process; 30 at the start, +1 a tick, capped at 30 (applied by the host's tick).
    pub credit: u32,
    first_person: bool,
    first_person_update: bool,
    fov_update: bool,
    fov: f32,
    pub stats: ControlStats,
}

impl Default for ControlState {
    fn default() -> ControlState {
        ControlState::new()
    }
}

impl ControlState {
    /// A new connection's state: first person on, nothing to send, Torque's format.
    pub fn new() -> ControlState {
        ControlState::with_format(ControlFormat::default())
    }

    /// [`ControlState::new`] in `format` (the move list takes its moves' form).
    pub fn with_format(format: ControlFormat) -> ControlState {
        ControlState {
            moves: MoveList::with_format(format.moves),
            format,
            client_checksum: 0,
            credit: NET_MOVE_CREDIT_MAX,
            first_person: true,
            first_person_update: false,
            fov_update: false,
            fov: 90.0,
            stats: ControlStats::default(),
        }
    }

    /// The section's format (its moves' form is the move list's).
    pub fn format(&self) -> ControlFormat {
        ControlFormat {
            moves: self.moves.format(),
            ..self.format
        }
    }

    /// `setFirstPerson`: stores the setting and arms its update (step 5 of the next head carries it, changed or not; a format
    /// without the pair never sends it).
    pub fn set_first_person(&mut self, on: bool) {
        self.first_person = on;
        self.first_person_update = true;
    }

    pub fn first_person(&self) -> bool {
        self.first_person
    }

    /// Whether step 5 will carry the first-person setting.
    pub fn first_person_pending(&self) -> bool {
        self.first_person_update
    }

    /// `setControlCameraFov`. Nothing without a control object (`false`, nothing stored or armed). Otherwise the float is
    /// clamped ([`clamp_fov`]), the object at the top of the control chain is told (`setCameraFov`) and
    /// read back (`getCameraFov`; the object may clamp it to its datablock), and the read-back value is stored.
    /// The update flag is armed ONLY when `(int)new != (int)old`: an unchanged fov sends `flag(0)` in step 6, not nine more
    /// bits.
    pub fn set_control_camera_fov(&mut self, fov: f32, target: Option<&mut dyn FovTarget>) -> bool {
        let Some(target) = target else {
            return false;
        };
        target.set_camera_fov(clamp_fov(fov));
        let new = target.camera_fov();
        let to_int = library::get().to_int;
        if to_int(new) != to_int(self.fov) {
            self.fov_update = true;
        }
        self.fov = new;
        true
    }

    /// The fov a client sent in its head (server branch): the byte as a float, clamped like [`clamp_fov`], applied with
    /// [`ControlState::set_control_camera_fov`], read back with `getControlCameraFov`, and the update flag cleared when the
    /// truncated read-back equals the clamped request (the object took exactly what the client asked for, so there is nothing to
    /// correct). The host calls this with [`ClientBlock::fov`].
    pub fn apply_client_fov(&mut self, wire: u8, target: Option<&mut dyn FovTarget>) {
        let f = clamp_fov(f32::from(wire));
        let Some(t) = target else {
            return;
        };
        self.set_control_camera_fov(f, Some(&mut *t));
        if (library::get().to_int)(t.camera_fov()) as f32 == f {
            self.fov_update = false;
        }
    }

    /// Whether step 6 will carry a fov (the update flag).
    pub fn fov_pending(&self) -> bool {
        self.fov_update
    }

    /// The last fov set.
    pub fn fov(&self) -> f32 {
        self.fov
    }

    /// `incMoveCredit`: `ticks` more credit, capped.
    pub fn add_credit(&mut self, ticks: u32) {
        self.credit = self.credit.saturating_add(ticks).min(NET_MOVE_CREDIT_MAX);
    }

    /// Builds steps 1-6 for the next packet. `control` is the connection's control object, if any. The checksum compare is done
    /// here; the first-person and fov update flags are consumed. Step 4 is left empty: a host with a separate camera object sets
    /// [`ServerHead::camera_object`] before writing.
    pub fn head(
        &mut self,
        ghost_bits: u8,
        control: Option<&dyn ControlObject>,
    ) -> (ServerHead<Box<dyn ControlData>>, Step3) {
        let (damage, section) = match control {
            None => (Damage::default(), ControlSection::None),
            Some(obj) => {
                let damage = obj.damage();
                match obj.ghost_index() {
                    None => (damage, ControlSection::None),
                    Some(index) => {
                        let data = obj.packet_data();
                        if checksum(data.as_ref(), ghost_bits) != self.client_checksum {
                            (damage, ControlSection::Resend { index, data })
                        } else {
                            let position = data.position();
                            (damage, ControlSection::Matching { position })
                        }
                    }
                }
            }
        };
        let step = match section {
            ControlSection::None => {
                self.stats.none += 1;
                Step3::None
            }
            ControlSection::Matching { .. } => {
                self.stats.matching += 1;
                Step3::Matching
            }
            ControlSection::Resend { .. } => {
                self.stats.resend += 1;
                Step3::Resend
            }
        };
        let first_person = (self.format.first_person
            && std::mem::take(&mut self.first_person_update))
        .then_some(self.first_person);
        let fov = if self.fov_update {
            self.fov_update = false;
            Some((library::get().to_int)(self.fov).clamp(FOV_MIN, FOV_MAX) as u8)
        } else {
            None
        };
        (
            ServerHead {
                move_ack: self.moves.ack(),
                damage,
                control: section,
                camera_object: None,
                camera_data: None,
                first_person,
                fov,
            },
            step,
        )
    }

    /// Builds the head and writes it.
    pub fn write_head(
        &mut self,
        w: &mut BitWriter,
        ghost_bits: u8,
        control: Option<&dyn ControlObject>,
    ) -> Step3 {
        let (head, step) = self.head(ghost_bits, control);
        head.write_with(w, ghost_bits, self.format());
        step
    }

    /// Reads the client's head: the checksum is stored, the moves absorbed, a first-person setting applied (without arming the
    /// update: the client already has it). The block (with the fov) is returned for the host.
    pub fn read_client(
        &mut self,
        r: &mut BitReader,
        charge: &mut ChargeFn,
    ) -> Result<(ClientBlock, Absorbed), NetError> {
        let block = ClientBlock::read_with(r, self.format(), charge)?;
        self.client_checksum = block.checksum;
        if let Some(on) = block.first_person {
            self.first_person = on;
            self.first_person_update = false;
        }
        let absorbed = self.moves.absorb(&block.moves);
        Ok((block, absorbed))
    }
}

#[cfg(test)]
mod tests;
