//! TGE's own set: [`TgeClientSet`], the client's world over `torque-ghosts`' TGE classes and records ([`TgeGhosts`]), and a
//! client ghost table for it ([`TgeCodec`], the record codec torque-net's `GhostClient` reads a connection's ghosts with).
//!
//! A record is kept as the client keeps its object: the creating record, then every update merged into it block by block (a block
//! that was sent replaces the kept one, a block that was not leaves it; the per-slot lists of the ShapeBase part merge slot by slot;
//! the fields every update carries are replaced). Datablocks are kept as their fields ([`TgeDatablock`]).
//!
//! Credit: the client's reading of each record follows TGE 1.4's `unpackUpdate`s, as Torque3D (MIT) carries them in
//! `T3D/shapeBase.cpp`, `T3D/player.cpp`, `T3D/projectile.cpp` (each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015
//! Faust Logic, Inc.), `T3D/camera.cpp`, `T3D/item.cpp` and `T3D/vehicles/vehicle.cpp` (each Copyright (c) 2012 GarageGames,
//! LLC); an image datablock's state table follows `T3D/shapeImage.cpp` (Copyright (c) 2012 GarageGames, LLC) and
//! `T3D/shapeBase.h` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code is this crate's own.

use std::collections::BTreeMap;

use ghosts::audio::AudioEmitter;
use ghosts::fx::{FxLight, ParticleEmitterNode};
use ghosts::interior::InteriorInstance;
use ghosts::light::{ScatterSky, SpotLight, TimeOfDay};
use ghosts::markers::{MissionMarker, SpawnSphere, WayPoint};
use ghosts::parts::GameBase;
use ghosts::player::{HEAD_BITS, Player, VELOCITY_NORMAL_BITS, YAW_BITS};
use ghosts::plugin::PluginClasses;
use ghosts::projectile::Projectile;
use ghosts::shapebase::{Rare, ShapeBody};
use ghosts::shapes::{Camera, Item, StaticShape};
use ghosts::sky::{Sky, Sun};
use ghosts::vehicle::{FlyingVehicle, HoverVehicle, VehiclePart, WheeledVehicle};
use ghosts::weather::{Lightning, Precipitation};
use ghosts::zone::PhysicalZone;
use ghosts::{
    ClientFacts, F32, GhostCodecSet, ObjCtx, ReadCtx, TgeClass, TgeGhosts, TgeRecord, TgeShapeBase,
    Value,
};
use net::NetError;
use net::bits::BitReader;
use net::classes::Kind;
use net::control::{CameraPacketData, ControlData};
use net::ghost::{ClassForm, GhostClient, GhostCodec};
use net::huffman::Table;

use crate::images::{ImageSlot, ImageStates};
use crate::set::{
    ClientDatablock, ClientRecord, ClientSet, ClientTail, Explosion, MoveBlock, SentGhost,
    SentTable, SoundSlot,
};
use crate::threads::Thread;
use crate::world::Pose;

/// TGE's ghost classes and records as the client's world keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TgeClientSet;

impl ClientSet for TgeClientSet {
    type Class = TgeClass;
    type Record = TgeRecord;
    type Datablock = TgeDatablock;
    type Tail = TgeTail;
    type Part = ();
    type Effect = ();

    fn class_name(class: TgeClass) -> &'static str {
        class.name()
    }

    fn is_vehicle(class: TgeClass) -> bool {
        matches!(
            class,
            TgeClass::WheeledVehicle
                | TgeClass::AiWheeledVehicle
                | TgeClass::FlyingVehicle
                | TgeClass::HoverVehicle
        )
    }

    fn tail(data: &dyn ControlData) -> TgeTail {
        let any = data as &dyn std::any::Any;
        if let Some(cam) = any.downcast_ref::<CameraPacketData>() {
            TgeTail::Camera(cam.clone())
        } else if let Some(p) = any.downcast_ref::<ghosts::TgePacketData>() {
            TgeTail::Data(p.clone())
        } else {
            TgeTail::Other(format!("{data:?}"))
        }
    }
}

/// What the last mismatching control section held about the control object.
#[derive(Debug, Clone, PartialEq)]
pub enum TgeTail {
    /// A Camera's packet data.
    Camera(CameraPacketData),
    /// Another TGE class's packet data.
    Data(ghosts::TgePacketData),
    /// Packet data of a type the set does not know (kept as its debug text).
    Other(String),
}

impl ClientTail for TgeTail {
    fn camera(&self) -> Option<([f32; 3], f32, f32)> {
        match self {
            TgeTail::Camera(cam) => Some((cam.position, cam.pitch, cam.yaw)),
            _ => None,
        }
    }
}

/// A datablock as the client holds it: its class name and its fields (name, element, value), as a datablock codec read them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TgeDatablock {
    pub class: String,
    pub fields: Vec<(String, u32, Value)>,
}

impl TgeDatablock {
    /// The first value of field `name`'s element `index`.
    pub fn get(&self, name: &str, index: u32) -> Option<&Value> {
        self.fields
            .iter()
            .find(|(n, i, _)| n == name && *i == index)
            .map(|(_, _, v)| v)
    }
}

impl ClientDatablock for TgeDatablock {
    fn text(&self, field: &str) -> Option<&[u8]> {
        match self.get(field, 0)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// TGE's PlayerData has no jump sound.
    fn jump_sound(&self) -> Option<u32> {
        None
    }

    /// `ProjectileData::explosion`.
    fn explosion(&self) -> Option<u32> {
        match self.get("explosion", 0)? {
            Value::Id(id) => *id,
            _ => None,
        }
    }

    /// `TSShapeConstructor::baseShape`.
    fn base_shape(&self) -> Option<&[u8]> {
        self.text("baseShape")
    }

    /// `TSShapeConstructor`'s `sequence0`, `sequence1`, ... up to the first one not set.
    fn sequences(&self, max: usize) -> Vec<Vec<u8>> {
        (0..max)
            .map_while(|i| self.text(&format!("sequence{i}")).map(<[u8]>::to_vec))
            .collect()
    }

    /// The state table as TGE's record carries it: each state's members after its `state` flag (in the wire's order, a member's
    /// element counted within its state), read as the host's state arrays ([`tge_state_fields`]).
    fn image_states(&self) -> Option<ImageStates> {
        (self.class == "ShapeBaseImageData").then(|| {
            ImageStates::of_fields(tge_state_fields(
                self.fields.iter().map(|(n, i, v)| (n.as_str(), *i, v)),
            ))
        })
    }
}

/// A TGE `ShapeBaseImageData` record's state members (`state`, `transition.loaded`, `timeoutValue`, `sequence`, ...: each
/// state's after its `state` flag, the flag's element the state) as the host's state arrays name them: `stateUsed`, `stateTrans`
/// eleven to a state (`NotLoaded`, `Loaded`, `NoAmmo`, `Ammo`, `NoTarget`, `Target`, `NotWet`, `Wet`, `TriggerUp`, `TriggerDown`,
/// `Timeout`), `stateTimeoutValue`, ... at the state's element. Fields before the first state, or that are no state member,
/// pass as they are.
pub fn tge_state_fields<'a>(
    fields: impl IntoIterator<Item = (&'a str, u32, &'a Value)>,
) -> Vec<(&'a str, u32, &'a Value)> {
    const MEMBERS: [(&str, &str); 14] = [
        ("timeoutValue", "stateTimeoutValue"),
        ("waitForTimeout", "stateWaitForTimeout"),
        ("fire", "stateFire"),
        ("loaded", "stateLoadedFlag"),
        ("spin", "stateSpinThread"),
        ("sequence", "stateSequence"),
        ("direction", "stateDirection"),
        ("scaleAnimation", "stateScaleAnimation"),
        ("flashSequence", "stateFlashSequence"),
        ("sound", "stateSound"),
        ("emitter", "stateEmitter"),
        ("emitterTime", "stateEmitterTime"),
        ("emitterNode", "stateEmitterNode"),
        ("recoil", "stateRecoil"),
    ];
    let mut state: Option<u32> = None;
    // Each transition pair's elements seen in the state so far.
    let mut pairs = [0u32; 5];
    let mut out = Vec::new();
    for (name, index, value) in fields {
        if name == "state" {
            state = Some(index);
            pairs = [0; 5];
            out.push(("stateUsed", index, value));
            continue;
        }
        let Some(s) = state else {
            out.push((name, index, value));
            continue;
        };
        let pair = ["loaded", "ammo", "target", "wet", "trigger"]
            .iter()
            .position(|p| name.strip_prefix("transition.") == Some(p));
        if let Some(p) = pair {
            let k = pairs[p];
            pairs[p] += 1;
            if k < 2 {
                out.push(("stateTrans", 11 * s + 2 * p as u32 + k, value));
            }
        } else if name == "transition.timeout" {
            out.push(("stateTrans", 11 * s + 10, value));
        } else if let Some((_, host)) = MEMBERS.iter().find(|(m, _)| *m == name) {
            out.push((host, s, value));
        } else {
            out.push((name, index, value));
        }
    }
    out
}

fn floats<const N: usize>(v: [F32; N]) -> [f32; N] {
    v.map(F32::get)
}

/// A Player record's head as `Player::unpackUpdate` reads it (`readSignedFloat(6)` each): x (the pitch) and z (the side
/// angle), each a fraction of its datablock's `maxLookAngle` (`None`: another record, or one with no move).
pub fn head_fractions(record: &TgeRecord) -> Option<[f32; 2]> {
    match record {
        TgeRecord::AiPlayer(r) | TgeRecord::Player(r) => r.motion.as_ref().map(|m| {
            m.head
                .map(|h| net::library::plain::read_signed_float(h, HEAD_BITS))
        }),
        _ => None,
    }
}

/// `readFloat(bits)`: an int over `2^bits - 1`.
fn unit_float(v: u32, bits: u8) -> f32 {
    v as f32 / ((1u32 << bits) - 1) as f32
}

/// The ShapeBase part of a record, for the classes that have one.
pub fn shape_part(record: &TgeRecord) -> Option<&TgeShapeBase> {
    use TgeRecord as R;
    Some(match record {
        R::Camera(r) => &r.shape,
        R::StaticShape(r) | R::ScopeAlwaysShape(r) => &r.shape,
        R::MissionMarker(r) => &r.shape,
        R::WayPoint(r) => &r.marker.shape,
        R::SpawnSphere(r) => &r.marker.shape,
        R::AiPlayer(r) | R::Player(r) => &r.shape,
        R::Item(r) => &r.shape,
        R::FlyingVehicle(r) => &r.vehicle.shape,
        R::HoverVehicle(r) => &r.vehicle.shape,
        R::WheeledVehicle(r) | R::AiWheeledVehicle(r) => &r.vehicle.shape,
        _ => return None,
    })
}

fn body(record: &TgeRecord) -> Option<&ShapeBody> {
    shape_part(record)?.body.as_ref()
}

fn vehicle(record: &TgeRecord) -> Option<&VehiclePart> {
    match record {
        TgeRecord::FlyingVehicle(r) => Some(&r.vehicle),
        TgeRecord::HoverVehicle(r) => Some(&r.vehicle),
        TgeRecord::WheeledVehicle(r) | TgeRecord::AiWheeledVehicle(r) => Some(&r.vehicle),
        _ => None,
    }
}

fn affine(transform: Option<(ghosts::Xf, [F32; 3])>) -> Option<Pose> {
    let (xf, scale) = transform?;
    Some(Pose::Affine {
        xf,
        scale: Some(floats(scale)),
    })
}

impl ClientRecord<TgeClientSet> for TgeRecord {
    fn merge_update(&mut self, sent: &TgeRecord) {
        self.merge(sent);
    }

    fn datablock(&self) -> Option<u32> {
        use TgeRecord as R;
        match self {
            R::Projectile(r) => r.game.datablock,
            R::ParticleEmitterNode(r) => r.game.datablock,
            R::FxLight(r) => r.game.datablock,
            R::Lightning(r) => r.game.datablock,
            R::Precipitation(r) => r.game.datablock,
            R::Plugin(_, r) => r.record_datablock(),
            record => shape_part(record)?.game.datablock,
        }
    }

    fn point(&self, cp: [f32; 3]) -> Option<[f32; 3]> {
        use TgeRecord as R;
        match self {
            R::AiPlayer(r) | R::Player(r) => r.motion.as_ref().map(|m| m.position.value(cp)),
            R::Projectile(r) => r.initial.as_ref().map(|i| i.position.value(cp)),
            record => vehicle(record)?
                .state
                .as_ref()?
                .rigid
                .as_ref()
                .map(|s| s.position.value(cp)),
        }
    }

    /// A Player's pitch is the head's fraction of its datablock's `maxLookAngle`, which the record does not carry: 0 here
    /// ([`head_fractions`] gives the record's fractions to scale by the datablock).
    fn pose(&self, point: Option<[f32; 3]>) -> Option<Pose> {
        use TgeRecord as R;
        Some(match self {
            R::StaticShape(r) | R::ScopeAlwaysShape(r) => affine(r.transform)?,
            R::MissionMarker(r) => affine(r.transform)?,
            R::WayPoint(r) => affine(r.marker.transform)?,
            R::SpawnSphere(r) => affine(r.marker.transform)?,
            R::TsStatic(r) => Pose::Matrix {
                m: floats(r.matrix),
                scale: floats(r.scale),
            },
            R::VehicleBlocker(r) => Pose::Matrix {
                m: floats(r.matrix),
                scale: floats(r.scale),
            },
            R::ParticleEmitterNode(r) => Pose::Matrix {
                m: floats(r.matrix),
                scale: floats(r.scale),
            },
            R::PhysicalZone(r) => {
                let i = r.initial.as_ref()?;
                Pose::Matrix {
                    m: floats(i.matrix),
                    scale: floats(i.scale),
                }
            }
            R::FxLight(r) => Pose::Affine {
                xf: r.config.as_ref()?.transform,
                scale: None,
            },
            R::InteriorInstance(r) => {
                let (m, scale) = match (&r.init, &r.transform) {
                    (_, Some((m, s))) => (*m, *s),
                    (Some(i), None) => (i.matrix, i.scale),
                    (None, None) => return None,
                };
                Pose::Matrix {
                    m: floats(m),
                    scale: floats(scale),
                }
            }
            R::AudioEmitter(r) => Pose::Affine {
                xf: r.transform?,
                scale: None,
            },
            R::SpotLight(r) => Pose::Affine {
                xf: r.transform?,
                scale: None,
            },
            // A plugin's record: its first transform.
            R::Plugin(_, r) => Pose::Affine {
                xf: r.record_transform()?,
                scale: None,
            },
            R::Camera(r) => {
                let [x, y, z, pitch, yaw] = floats(r.pose?);
                Pose::Euler {
                    position: [x, y, z],
                    pitch,
                    yaw,
                }
            }
            R::AiPlayer(r) | R::Player(r) => {
                let m = r.motion.as_ref()?;
                Pose::Euler {
                    position: point?,
                    pitch: 0.0,
                    yaw: unit_float(m.yaw, YAW_BITS) * std::f32::consts::TAU,
                }
            }
            R::Item(r) => Pose::Item {
                position: floats(r.position.as_ref()?.position),
                rotation: r.rotation.map(|(f, a)| (f, a.get())),
            },
            record => {
                let rigid = vehicle(record)?.state.as_ref()?.rigid.as_ref()?;
                Pose::Quat {
                    position: point?,
                    rotation: floats(rigid.rotation),
                }
            }
        })
    }

    fn scale(&self) -> Option<[f32; 3]> {
        shape_part(self)?.game.scale.map(floats)
    }

    fn mount(&self) -> Option<(u32, u32)> {
        if let TgeRecord::SpotLight(r) = self {
            return r.mount.flatten().map(|m| (m.object, u32::from(m.node)));
        }
        body(self)?.mount.flatten().map(|m| (m.object, m.node))
    }

    fn images(&self) -> Option<[Option<ImageSlot>; 4]> {
        let sent = body(self)?.images.as_ref()?;
        Some(sent.clone().map(|s| {
            s.map(|s| ImageSlot {
                datablock: s.datablock,
                skin: s.skin,
                flags: [s.wet, s.ammo, s.loaded, s.target, s.trigger_down],
                fire_count: s.fire_count,
                firing: s.firing.unwrap_or(false),
            })
        }))
    }

    fn threads(&self) -> Option<[Option<Thread>; 4]> {
        let sent = body(self)?.threads.as_ref()?;
        Some(sent.map(|t| {
            t.map(|t| Thread {
                sequence: t.sequence,
                state: t.state,
                forward: t.forward,
                at_end: t.at_end,
            })
        }))
    }

    fn sounds(&self) -> Option<[Option<SoundSlot>; 4]> {
        let sent = body(self)?.sounds.as_ref()?;
        Some(sent.map(|s| {
            s.map(|profile| SoundSlot {
                playing: profile.is_some(),
                profile: profile.unwrap_or(0),
            })
        }))
    }

    fn explosion(&self) -> Option<Explosion> {
        let TgeRecord::Projectile(r) = self else {
            return None;
        };
        let x = r.explosion.as_ref()?;
        Some(Explosion {
            position: floats(x.position),
            normal: floats(x.normal),
        })
    }

    /// TGE's move block carries no jump event.
    fn move_block(&self) -> Option<MoveBlock> {
        let (TgeRecord::Player(r) | TgeRecord::AiPlayer(r)) = self else {
            return None;
        };
        let m = r.motion.as_ref()?;
        Some(MoveBlock {
            velocity: m
                .velocity
                .map_or([0.0; 3], |v| v.value(VELOCITY_NORMAL_BITS)),
            warp: m.warp,
            jumped: false,
        })
    }

    /// `Camera::unpackUpdate`: `flag(controlled)` clear, then the position, pitch and yaw it `setPosition`s.
    fn camera_move(&self) -> Option<[f32; 5]> {
        let TgeRecord::Camera(c) = self else {
            return None;
        };
        if c.controlled {
            return None;
        }
        Some(floats(c.pose?))
    }
}

// ---- the merge

/// A block that was sent replaces the kept one.
fn set<T: Clone>(kept: &mut Option<T>, sent: &Option<T>) {
    if sent.is_some() {
        kept.clone_from(sent);
    }
}

/// A list of per-slot blocks: each slot that was sent replaces the kept slot.
fn slots<T: Clone, const N: usize>(
    kept: &mut Option<[Option<T>; N]>,
    sent: &Option<[Option<T>; N]>,
) {
    let Some(sent) = sent else {
        return;
    };
    let kept = kept.get_or_insert_with(|| std::array::from_fn(|_| None));
    for (k, s) in kept.iter_mut().zip(sent) {
        set(k, s);
    }
}

/// A nested block that merges when both were sent.
fn nested<T: Merge + Clone>(kept: &mut Option<T>, sent: &Option<T>) {
    match (kept, sent) {
        (Some(k), Some(s)) => k.merge(s),
        (k, s) => set(k, s),
    }
}

/// Merges an update record into the kept state.
trait Merge {
    fn merge(&mut self, sent: &Self);
}

impl Merge for GameBase {
    fn merge(&mut self, sent: &GameBase) {
        set(&mut self.scale, &sent.scale);
        set(&mut self.datablock, &sent.datablock);
    }
}

impl Merge for Rare {
    fn merge(&mut self, sent: &Rare) {
        set(&mut self.cloak, &sent.cloak);
        set(&mut self.name, &sent.name);
        set(&mut self.shield, &sent.shield);
        set(&mut self.invincible, &sent.invincible);
        set(&mut self.skin, &sent.skin);
    }
}

impl Merge for ShapeBody {
    fn merge(&mut self, sent: &ShapeBody) {
        set(&mut self.damage, &sent.damage);
        slots(&mut self.threads, &sent.threads);
        slots(&mut self.sounds, &sent.sounds);
        slots(&mut self.images, &sent.images);
        nested(&mut self.rare, &sent.rare);
        set(&mut self.mount, &sent.mount);
    }
}

impl Merge for TgeShapeBase {
    fn merge(&mut self, sent: &TgeShapeBase) {
        self.game.merge(&sent.game);
        nested(&mut self.body, &sent.body);
    }
}

impl<P: Merge> Merge for MissionMarker<P> {
    fn merge(&mut self, sent: &MissionMarker<P>) {
        self.shape.merge(&sent.shape);
        set(&mut self.transform, &sent.transform);
    }
}

impl<P: Merge> Merge for WayPoint<P> {
    fn merge(&mut self, sent: &WayPoint<P>) {
        self.marker.merge(&sent.marker);
        set(&mut self.name, &sent.name);
        set(&mut self.team, &sent.team);
        set(&mut self.hidden, &sent.hidden);
    }
}

impl<P: Merge> Merge for SpawnSphere<P> {
    fn merge(&mut self, sent: &SpawnSphere<P>) {
        self.marker.merge(&sent.marker);
        set(&mut self.sphere, &sent.sphere);
    }
}

impl Merge for Camera {
    fn merge(&mut self, sent: &Camera) {
        self.shape.merge(&sent.shape);
        self.controlled = sent.controlled;
        set(&mut self.pose, &sent.pose);
    }
}

impl Merge for StaticShape {
    fn merge(&mut self, sent: &StaticShape) {
        self.shape.merge(&sent.shape);
        set(&mut self.transform, &sent.transform);
        self.powered = sent.powered;
    }
}

impl Merge for Item {
    fn merge(&mut self, sent: &Item) {
        self.shape.merge(&sent.shape);
        set(&mut self.initial, &sent.initial);
        set(&mut self.collision_object, &sent.collision_object);
        set(&mut self.rotation, &sent.rotation);
        set(&mut self.position, &sent.position);
    }
}

impl Merge for Player {
    fn merge(&mut self, sent: &Player) {
        self.shape.merge(&sent.shape);
        set(&mut self.impact, &sent.impact);
        set(&mut self.action, &sent.action);
        set(&mut self.arm, &sent.arm);
        self.controlled = sent.controlled;
        set(&mut self.motion, &sent.motion);
        // The energy travels with every update its connection does not control.
        if !sent.controlled {
            self.energy = sent.energy;
        }
    }
}

impl Merge for Projectile {
    fn merge(&mut self, sent: &Projectile) {
        self.game.merge(&sent.game);
        set(&mut self.initial, &sent.initial);
        set(&mut self.explosion, &sent.explosion);
        set(&mut self.bounce, &sent.bounce);
    }
}

impl Merge for VehiclePart {
    /// The state's steering and move travel with every update to a connection that does not control the vehicle; its rigid
    /// state (`PositionMask`) and energy (`EnergyMask`) only when they changed: an update without them keeps the last ones
    /// (`Vehicle::unpackUpdate` leaves them as they are).
    fn merge(&mut self, sent: &VehiclePart) {
        self.shape.merge(&sent.shape);
        self.jetting = sent.jetting;
        self.controlled = sent.controlled;
        match (&mut self.state, &sent.state) {
            (Some(kept), Some(s)) => {
                kept.steering = s.steering;
                kept.mv = s.mv;
                set(&mut kept.rigid, &s.rigid);
                set(&mut kept.energy, &s.energy);
            }
            (kept, sent) => set(kept, sent),
        }
    }
}

impl Merge for WheeledVehicle {
    fn merge(&mut self, sent: &WheeledVehicle) {
        self.vehicle.merge(&sent.vehicle);
        match (&mut self.setup, &sent.setup) {
            (Some(kept), Some(sent)) if kept.len() == sent.len() => {
                for (k, s) in kept.iter_mut().zip(sent) {
                    set(k, s);
                }
            }
            (kept, sent) => set(kept, sent),
        }
        self.controlled = sent.controlled;
        set(&mut self.motion, &sent.motion);
    }
}

impl Merge for FlyingVehicle {
    fn merge(&mut self, sent: &FlyingVehicle) {
        self.vehicle.merge(&sent.vehicle);
        set(&mut self.tail, &sent.tail);
    }
}

impl Merge for HoverVehicle {
    fn merge(&mut self, sent: &HoverVehicle) {
        self.vehicle.merge(&sent.vehicle);
        self.thrust = sent.thrust;
    }
}

impl Merge for ParticleEmitterNode {
    fn merge(&mut self, sent: &ParticleEmitterNode) {
        self.game.merge(&sent.game);
        // The matrix, the scale and the emitter travel with every update.
        self.matrix = sent.matrix;
        self.scale = sent.scale;
        self.emitter = sent.emitter;
    }
}

impl Merge for FxLight {
    fn merge(&mut self, sent: &FxLight) {
        self.game.merge(&sent.game);
        set(&mut self.attach, &sent.attach);
        set(&mut self.config, &sent.config);
    }
}

/// A later update keeps the first's block; a transform it sends replaces the first's.
impl Merge for InteriorInstance {
    fn merge(&mut self, sent: &InteriorInstance) {
        set(&mut self.init, &sent.init);
        if let (Some(i), Some((m, s))) = (self.init.as_mut(), sent.transform) {
            i.matrix = m;
            i.scale = s;
        }
        if sent.init.is_none()
            && let Some(i) = self.init.as_mut()
        {
            i.alarm = sent.alarm;
        }
        self.transform = None;
    }
}

impl Merge for SpotLight {
    fn merge(&mut self, sent: &SpotLight) {
        set(&mut self.transform, &sent.transform);
        set(&mut self.update, &sent.update);
        set(&mut self.enabled, &sent.enabled);
        set(&mut self.mount, &sent.mount);
    }
}

impl Merge for TimeOfDay {
    /// An animation is a one-off order: kept only from the update that sent it.
    fn merge(&mut self, sent: &TimeOfDay) {
        set(&mut self.orbit, &sent.orbit);
        self.animate = sent.animate;
    }
}

impl Merge for ScatterSky {
    fn merge(&mut self, sent: &ScatterSky) {
        set(&mut self.update, &sent.update);
    }
}

impl Merge for PhysicalZone {
    fn merge(&mut self, sent: &PhysicalZone) {
        set(&mut self.initial, &sent.initial);
        self.active = sent.active;
    }
}

impl Merge for AudioEmitter {
    fn merge(&mut self, sent: &AudioEmitter) {
        self.initial = sent.initial;
        set(&mut self.transform, &sent.transform);
        set(&mut self.profile, &sent.profile);
        set(&mut self.description, &sent.description);
        set(&mut self.file_name, &sent.file_name);
        set(
            &mut self.use_profile_description,
            &sent.use_profile_description,
        );
        set(&mut self.volume, &sent.volume);
        set(&mut self.looping, &sent.looping);
        set(&mut self.is_3d, &sent.is_3d);
        set(&mut self.min_distance, &sent.min_distance);
        set(&mut self.max_distance, &sent.max_distance);
        set(&mut self.cone_inside, &sent.cone_inside);
        set(&mut self.cone_outside, &sent.cone_outside);
        set(&mut self.cone_outside_volume, &sent.cone_outside_volume);
        set(&mut self.cone_vector, &sent.cone_vector);
        set(&mut self.loop_count, &sent.loop_count);
        set(&mut self.min_gap, &sent.min_gap);
        set(&mut self.max_gap, &sent.max_gap);
        set(&mut self.kind, &sent.kind);
        set(&mut self.outside_ambient, &sent.outside_ambient);
        self.enable_visual_feedback = sent.enable_visual_feedback;
    }
}

impl Merge for Sky {
    fn merge(&mut self, sent: &Sky) {
        set(&mut self.init, &sent.init);
        set(&mut self.storm_clouds_on, &sent.storm_clouds_on);
        set(&mut self.storm_fog_on, &sent.storm_fog_on);
        set(&mut self.visibility, &sent.visibility);
        set(&mut self.storm_cloud, &sent.storm_cloud);
        set(&mut self.storm_fog, &sent.storm_fog);
        set(&mut self.real_fog, &sent.real_fog);
        set(&mut self.wind, &sent.wind);
    }
}

impl Merge for Sun {
    fn merge(&mut self, sent: &Sun) {
        set(&mut self.update, &sent.update);
    }
}

impl Merge for Lightning {
    fn merge(&mut self, sent: &Lightning) {
        self.game.merge(&sent.game);
        set(&mut self.body, &sent.body);
    }
}

impl Merge for Precipitation {
    fn merge(&mut self, sent: &Precipitation) {
        self.game.merge(&sent.game);
        set(&mut self.data, &sent.data);
        set(&mut self.percentage, &sent.percentage);
        set(&mut self.storm, &sent.storm);
    }
}

impl Merge for TgeRecord {
    /// A record of another class replaces the kept one; TSStatic and VehicleBlocker send everything every time. A plugin's record
    /// merges as its type does ([`ghosts::plugin::NetRecord::merge`]).
    fn merge(&mut self, sent: &TgeRecord) {
        use TgeRecord as R;
        match (self, sent) {
            // One class is one record type, so the merge takes it.
            (R::Plugin(kc, k), R::Plugin(sc, s)) if kc == sc => {
                k.merge_record(&**s);
            }
            (R::AiPlayer(k), R::AiPlayer(s)) | (R::Player(k), R::Player(s)) => k.merge(s),
            (R::AiWheeledVehicle(k), R::AiWheeledVehicle(s))
            | (R::WheeledVehicle(k), R::WheeledVehicle(s)) => k.merge(s),
            (R::AudioEmitter(k), R::AudioEmitter(s)) => k.merge(s),
            (R::Camera(k), R::Camera(s)) => k.merge(s),
            (R::FlyingVehicle(k), R::FlyingVehicle(s)) => k.merge(s),
            (R::HoverVehicle(k), R::HoverVehicle(s)) => k.merge(s),
            (R::Item(k), R::Item(s)) => k.merge(s),
            (R::Lightning(k), R::Lightning(s)) => k.merge(s),
            (R::MissionMarker(k), R::MissionMarker(s)) => k.merge(s),
            (R::ParticleEmitterNode(k), R::ParticleEmitterNode(s)) => k.merge(s),
            (R::PhysicalZone(k), R::PhysicalZone(s)) => k.merge(s),
            (R::Precipitation(k), R::Precipitation(s)) => k.merge(s),
            (R::Projectile(k), R::Projectile(s)) => k.merge(s),
            (R::ScopeAlwaysShape(k), R::ScopeAlwaysShape(s))
            | (R::StaticShape(k), R::StaticShape(s)) => k.merge(s),
            (R::Sky(k), R::Sky(s)) => k.merge(s),
            (R::SpawnSphere(k), R::SpawnSphere(s)) => k.merge(s),
            (R::Sun(k), R::Sun(s)) => k.merge(s),
            (R::WayPoint(k), R::WayPoint(s)) => k.merge(s),
            (R::FxLight(k), R::FxLight(s)) => k.merge(s),
            (R::InteriorInstance(k), R::InteriorInstance(s)) => k.merge(s),
            (R::SpotLight(k), R::SpotLight(s)) => k.merge(s),
            (R::TimeOfDay(k), R::TimeOfDay(s)) => k.merge(s),
            (R::ScatterSky(k), R::ScatterSky(s)) => k.merge(s),
            (kept, sent) => kept.clone_from(sent),
        }
    }
}

// ---- the client's ghost table

/// A ghost as the client's table holds it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TgeGhost {
    /// `None` for the placeholder of an empty ghost-always event.
    pub class: Option<TgeClass>,
    /// The creating record's number among the client's records (1 first); 0 for a placeholder.
    pub serial: usize,
    /// The last record read for it: the creating one, or an update holding only its mask's blocks.
    pub record: Option<TgeRecord>,
    /// Update records read after the creating one.
    pub updates: u32,
    /// The datablock its records named last (a later update need not carry it; a WheeledVehicle's wheel count comes from it).
    pub datablock: Option<u32>,
}

impl SentGhost<TgeClientSet> for TgeGhost {
    fn class(&self) -> Option<TgeClass> {
        self.class
    }

    fn serial(&self) -> usize {
        self.serial
    }

    fn record(&self) -> Option<&TgeRecord> {
        self.record.as_ref()
    }

    fn updates(&self) -> u32 {
        self.updates
    }
}

impl SentTable<TgeClientSet> for GhostClient<TgeCodec> {
    type Ghost = TgeGhost;

    fn ghost(&self, index: u32) -> Option<&TgeGhost> {
        GhostClient::ghost(self, index)
    }
}

/// The wheel counts of the WheeledVehicleData the client has, by datablock id.
#[derive(Debug, Clone, Default)]
pub struct TgeFacts {
    pub wheels: BTreeMap<u32, usize>,
}

impl ClientFacts for TgeFacts {
    fn wheel_count(&self, datablock: u32) -> Option<usize> {
        self.wheels.get(&datablock).copied()
    }
}

/// The record codec of a client's ghost table over [`TgeGhosts`]: the connection's object class ids (by their names, the net
/// class table's), its id widths and string table, and the client's facts.
///
/// **Ids past the table.** The engine's own classes take the ids right after it ([`GhostCodecSet::class_past_table`]), the
/// plugins' classes ([`plugins`](Self::plugins)) the ids after those, in their set's order ([`PluginClasses::ghost_base`]); any
/// other id is refused as an unknown class.
#[derive(Debug)]
pub struct TgeCodec {
    /// The set's class of each object class id (`None`: a class with no codec here, refused).
    pub classes: Vec<Option<TgeClass>>,
    /// The plugins' ghost classes the client reads (the server's set: both sides number them from the names).
    pub plugins: PluginClasses,
    pub widths: ghosts::NetWidths,
    pub table: Table,
    pub facts: TgeFacts,
    /// The ghost index of the connection's control object, when the control section named one.
    pub control: Option<u32>,
    /// Records read so far.
    pub serial: usize,
    /// The work units the records of the datagram being read may still spend ([`TgeCodec::set_budget`]); `None` sets no
    /// bound.
    budget: std::cell::Cell<Option<u64>>,
}

impl TgeCodec {
    /// Bounds the work the next records read may spend in all, until the next call (a client sets it per datagram:
    /// [`ghosts::datagram_budget`]); a record past it is an error.
    pub fn set_budget(&mut self, units: Option<u64>) {
        self.budget.set(units);
    }

    /// A charge callback on what is left of the budget ([`TgeCodec::set_budget`]), for the datagram's other decoders (its
    /// head, its datablock events): all of one datagram draw on one budget.
    pub fn charge(&self) -> impl FnMut(u64) -> bool + '_ {
        ghosts::cell_charge(&self.budget)
    }

    /// A codec for the object classes `names` in class id order (a connection's derived net class table).
    pub fn new<'a>(
        names: impl IntoIterator<Item = &'a str>,
        widths: ghosts::NetWidths,
        table: Table,
    ) -> TgeCodec {
        TgeCodec {
            classes: names.into_iter().map(TgeGhosts::class_by_name).collect(),
            plugins: PluginClasses::default(),
            widths,
            table,
            facts: TgeFacts::default(),
            control: None,
            serial: 0,
            budget: std::cell::Cell::new(None),
        }
    }

    /// The codec with the plugins' ghost classes `plugins` past the table and the engine's own classes.
    pub fn with_plugins(mut self, plugins: PluginClasses) -> TgeCodec {
        self.plugins = plugins;
        self
    }

    /// The class an object class id names: the table's, the engine's own past it, then the plugins' (`None`: unknown).
    pub fn class_of_id(&self, id: u32) -> Option<TgeClass> {
        let table = self.classes.len();
        self.classes
            .get(id as usize)
            .copied()
            .flatten()
            .or_else(|| TgeGhosts::class_past_table(id, table))
            .or_else(|| {
                let base = PluginClasses::ghost_base(table);
                self.plugins
                    .by_id(Kind::Object, id, base)
                    .map(TgeClass::Plugin)
            })
    }

    fn read(
        &self,
        class: TgeClass,
        initial: bool,
        datablock: Option<u32>,
        r: &mut BitReader,
    ) -> Result<TgeRecord, NetError> {
        let ctx = ReadCtx {
            widths: self.widths,
            initial,
            facts: Some(&self.facts),
            object: ObjCtx {
                controlled_here: false,
                datablock,
            },
        };
        TgeGhosts::decode(class, r, &self.table, &mut self.charge(), &ctx)
    }
}

impl GhostCodec for TgeCodec {
    type Object = TgeGhost;

    fn create(&mut self, class: ClassForm, r: &mut BitReader) -> Result<TgeGhost, NetError> {
        let ClassForm::Id(id) = class else {
            return Err(NetError::Invalid("implied class"));
        };
        // Past the table: the engine's own classes, then the plugins' (a server of the engine's own sends them to its own clients
        // only).
        let class = self
            .class_of_id(id)
            .ok_or(NetError::Invalid("object class"))?;
        let record = self.read(class, true, None, r)?;
        self.serial += 1;
        Ok(TgeGhost {
            class: Some(class),
            serial: self.serial,
            datablock: record.datablock(),
            record: Some(record),
            updates: 0,
        })
    }

    fn placeholder(&mut self) -> TgeGhost {
        TgeGhost::default()
    }

    fn update(&mut self, object: &mut TgeGhost, r: &mut BitReader) -> Result<(), NetError> {
        let class = object
            .class
            .ok_or(NetError::Invalid("update of a placeholder"))?;
        let record = self.read(class, false, object.datablock, r)?;
        self.serial += 1;
        if let Some(d) = record.datablock() {
            object.datablock = Some(d);
        }
        object.record = Some(record);
        object.updates += 1;
        Ok(())
    }
}

#[cfg(test)]
mod vehicle_merge_tests {
    use super::*;
    use ghosts::vehicle::{RigidState, VehicleState};

    #[test]
    fn a_vehicle_update_without_its_position_keeps_the_last_one() {
        let rigid = RigidState {
            at_rest: true,
            ..RigidState::default()
        };
        let mut kept = WheeledVehicle::default();
        kept.vehicle.state = Some(VehicleState {
            steering: [1, 0],
            rigid: Some(rigid),
            energy: Some(7),
            ..VehicleState::default()
        });
        let mut sent = WheeledVehicle::default();
        sent.vehicle.state = Some(VehicleState {
            steering: [300, 2],
            ..VehicleState::default()
        });
        kept.merge(&sent);
        let s = kept.vehicle.state.unwrap();
        assert_eq!(s.steering, [300, 2]);
        assert_eq!((s.rigid, s.energy), (Some(rigid), Some(7)));
    }
}

#[cfg(test)]
mod plugin_tests {
    use super::*;
    use ghosts::codec::{Codec, Layout, R, Xf};
    use ghosts::plugin::{
        DatablockRef, FieldSpec, GhostRecord, NetField, NetRecord, PluginClass, Replicate,
    };
    use ghosts::{ObjectView, PackCtx};

    /// A plugin's record, written as the derive writes it: a pose, a datablock and a field sent only now and then.
    #[derive(Debug, Clone, PartialEq, Default)]
    struct Marker {
        pose: Xf,
        data: DatablockRef,
        glow: Option<u32>,
    }

    impl Layout for Marker {
        fn layout<C: Codec>(&mut self, c: &mut C) -> R {
            self.pose.layout(c, "transform", FieldSpec::DEFAULT)?;
            self.data.layout(c, "dataBlock", FieldSpec::DEFAULT)?;
            self.glow.layout(c, "glow", FieldSpec::DEFAULT)
        }
    }

    impl NetRecord for Marker {
        fn merge(&mut self, sent: &Marker) {
            self.pose.merge_from(&sent.pose);
            self.data.merge_from(&sent.data);
            self.glow.merge_from(&sent.glow);
        }
        fn datablock(&self) -> Option<u32> {
            self.data.datablock()
        }
        fn transform(&self) -> Option<Xf> {
            Some(self.pose)
        }
        fn script_fields(&self) -> Vec<(String, u32, Value)> {
            Vec::new()
        }
    }

    impl GhostRecord for Marker {
        fn pack(_view: &dyn ObjectView, _mask: u32, _ctx: &PackCtx) -> (Marker, u32) {
            (Marker::default(), 0)
        }
    }

    fn codec_with(plugins: PluginClasses) -> TgeCodec {
        let names = ghosts::tge::TGE_OBJECT_CLASSES;
        TgeCodec::new(names, ghosts::NetWidths::engine_default(), Table::torque())
            .with_plugins(plugins)
    }

    #[test]
    fn class_ids_run_through_the_table_the_engines_classes_and_the_plugins_and_no_further() {
        let marker = PluginClass::ghost::<Marker>("Marker2", Replicate::Interpolated);
        let beacon = PluginClass::ghost::<Marker>("Beacon", Replicate::Predicted);
        let codec = codec_with(PluginClasses::new([marker, beacon]).unwrap());
        assert_eq!(codec.class_of_id(21), Some(TgeClass::Player));
        assert_eq!(codec.class_of_id(45), Some(TgeClass::SpotLight));
        assert_eq!(codec.class_of_id(48), Some(TgeClass::Plugin(beacon)));
        assert_eq!(codec.class_of_id(49), Some(TgeClass::Plugin(marker)));
        assert_eq!(codec.class_of_id(50), None);
        assert_eq!(codec.class_of_id(u32::MAX), None);
        // Without the plugins, their ids are as unknown as any other.
        assert_eq!(codec_with(PluginClasses::default()).class_of_id(48), None);
    }

    #[test]
    fn a_plugin_record_poses_names_its_datablock_and_merges_its_updates() {
        let class = PluginClass::ghost::<Marker>("Marker2", Replicate::Interpolated);
        let pose = Xf {
            position: [F32::new(1.0), F32::new(2.0), F32::new(3.0)],
            ..Xf::default()
        };
        let mut kept = TgeRecord::Plugin(
            class,
            Box::new(Marker {
                pose,
                data: DatablockRef(Some(9)),
                glow: Some(4),
            }),
        );
        assert_eq!(ClientRecord::datablock(&kept), Some(9));
        assert_eq!(
            kept.pose(None),
            Some(Pose::Affine {
                xf: pose,
                scale: None
            })
        );
        let sent = TgeRecord::Plugin(
            class,
            Box::new(Marker {
                pose: Xf::default(),
                data: DatablockRef(Some(10)),
                glow: None,
            }),
        );
        kept.merge_update(&sent);
        let marker = kept.plugin::<Marker>().unwrap();
        assert_eq!(
            (marker.data, marker.glow),
            (DatablockRef(Some(10)), Some(4))
        );
        // A record of another class replaces the kept one.
        kept.merge_update(&TgeRecord::Sun(ghosts::sky::Sun::default()));
        assert!(matches!(kept, TgeRecord::Sun(_)));
    }
}
