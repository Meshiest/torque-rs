//! TGE's `ShapeBaseImageData`: the mounted image and its state machine ([`ShapeBaseImageData`], [`ImageState`]).
//!
//! ```text
//! flag(computeCRC): U32 mCRC; flag emap; string shapeName; U32 mountPoint;
//! flag(mountOffset is the identity) [else affine transform]; flag(eyeOffset is the identity) [else affine transform];
//! flag correctMuzzleVector, firstPerson; F32 mass; flag usesEnergy; F32 minEnergy; flag hasFlash; db projectile;
//! flag cloakable; ranged(lightType, 0, NumLightTypes - 1) [when not NoLight: F32 lightRadius, S32 lightTime, float(7) x4 colour];
//! F32 x3 shellExitDir; F32 shellExitVariance, shellVelocity; db casing;
//! per state 0..31: flag(the state has a name): the state
//! state: string name; int(5) x11 transitions (each the target state + 1, so -1 travels as 0);
//!        flag(timeoutValue != 0): F32; flag waitForTimeout, fire, ejectShell, scaleAnimation, direction;
//!        flag(energyDrain != 0): F32; int(3) loaded, spin, recoil;
//!        flag(sequence != -1): signed int(16); flag(sequenceVis != -1): signed int(16); flag flashSequence, ignoreLoadedForReady;
//!        flag(emitter): db emitter, F32 emitterTime, S32 emitterNode; db sound
//! ```
//!
//! **The transitions, in TGE's order:** `loaded[0]` is the transition taken when the image is NOT loaded (`stateTransitionOnNotLoaded`)
//! and `loaded[1]` when it is (`stateTransitionOnLoaded`); likewise `ammo` is `[NoAmmo, Ammo]`, `target` `[NoTarget, Target]`,
//! `wet` `[NotWet, Wet]` and `trigger` `[TriggerUp, TriggerDown]`; then `timeout`. A signed int(16) is `flag(v < 0)` and the
//! magnitude in 15 bits ([`Signed`]).
//!
//! **The light block follows the light type that travels**: a type outside `0..=3` is wrapped into the 2-bit field first, and
//! the block is sent when the wrapped type is not `NoLight`, so the reader always parses what was written.
//!
//! **View fields** of a state are the members of TGE's resolved `StateData` (what preload makes of the `state*` script
//! arrays): `state.name` and the rest at the state's index, `state.transition.loaded` (and `ammo`, `target`, `wet`,
//! `trigger`) at `2 * state + k`, `state.transition.timeout` at the state's index; `-1` is no transition.
//!
//! Credit: this follows TGE 1.4's `game/shapeImage.cc` (Copyright (C) GarageGames.com, Inc.), whose MIT-licensed successor is
//! Torque3D's `T3D/shapeImage.cpp` (Copyright (c) 2012 GarageGames, LLC); Torque3D adds fields, the list here is TGE's. The
//! code is this crate's own.

use super::{db, db_ref, float, float_ints, float_ints_of, opt_float, unless};
use crate::codec::{Codec, F32, Layout, NetWidths, R, Xf, mask_bits, opt, wrap_ranged};
use crate::motion::Signed;
use crate::view::xform;
use crate::{ObjectView, Value, boolean, fld, int, raw, raws, text, word};

/// The states of an image (`MaxStates`).
pub const IMAGE_STATES: usize = 31;

/// The width of a state index (`NumStateBits`).
pub const STATE_BITS: u8 = 5;

/// The light types of an image (`NumLightTypes`): `NoLight`, `ConstantLight`, `PulsingLight`, `WeaponFireLight`.
pub const IMAGE_LIGHT_TYPES: u32 = 4;

/// A state's transitions as they travel (each the target state + 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Transitions {
    /// `[NotLoaded, Loaded]`.
    pub loaded: [u32; 2],
    /// `[NoAmmo, Ammo]`.
    pub ammo: [u32; 2],
    /// `[NoTarget, Target]`.
    pub target: [u32; 2],
    /// `[NotWet, Wet]`.
    pub wet: [u32; 2],
    /// `[TriggerUp, TriggerDown]`.
    pub trigger: [u32; 2],
    pub timeout: u32,
}

/// A state's particle emitter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StateEmitter {
    pub emitter: u32,
    pub time: F32,
    /// An `S32`'s 32 bits.
    pub node: u32,
}

/// One state of the image's state machine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageState {
    pub name: Vec<u8>,
    pub transition: Transitions,
    pub timeout_value: Option<F32>,
    pub wait_for_timeout: bool,
    pub fire: bool,
    pub eject_shell: bool,
    pub scale_animation: bool,
    pub direction: bool,
    pub energy_drain: Option<F32>,
    /// `int(3)`: `IgnoreLoaded`, `Loaded`, `NotLoaded`.
    pub loaded: u32,
    /// `int(3)`: `IgnoreSpin`, `NoSpin`, `SpinUp`, `SpinDown`, `FullSpin`.
    pub spin: u32,
    /// `int(3)`: `NoRecoil`, `LightRecoil`, `MediumRecoil`, `HeavyRecoil`.
    pub recoil: u32,
    pub sequence: Option<Signed>,
    pub sequence_vis: Option<Signed>,
    pub flash_sequence: bool,
    pub ignore_loaded_for_ready: bool,
    pub emitter: Option<StateEmitter>,
    pub sound: Option<u32>,
}

fn signed16<C: Codec>(c: &mut C, name: &'static str, v: &mut Signed) -> R {
    c.flag(name, &mut v.negative)?;
    c.int(name, &mut v.magnitude, 15)
}

impl Layout for ImageState {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.string("state.name", &mut self.name)?;
        let t = &mut self.transition;
        for (name, pair) in [
            ("transition.loaded", &mut t.loaded),
            ("transition.ammo", &mut t.ammo),
            ("transition.target", &mut t.target),
            ("transition.wet", &mut t.wet),
            ("transition.trigger", &mut t.trigger),
        ] {
            for x in pair.iter_mut() {
                c.int(name, x, STATE_BITS)?;
            }
        }
        c.int("transition.timeout", &mut t.timeout, STATE_BITS)?;
        opt_float(c, "timeoutValue", &mut self.timeout_value)?;
        c.flag("waitForTimeout", &mut self.wait_for_timeout)?;
        c.flag("fire", &mut self.fire)?;
        c.flag("ejectShell", &mut self.eject_shell)?;
        c.flag("scaleAnimation", &mut self.scale_animation)?;
        c.flag("direction", &mut self.direction)?;
        opt_float(c, "energyDrain", &mut self.energy_drain)?;
        c.int("loaded", &mut self.loaded, 3)?;
        c.int("spin", &mut self.spin, 3)?;
        c.int("recoil", &mut self.recoil, 3)?;
        opt(c, "sequence", &mut self.sequence, |c, s| {
            signed16(c, "sequence", s)
        })?;
        opt(c, "sequenceVis", &mut self.sequence_vis, |c, s| {
            signed16(c, "sequenceVis", s)
        })?;
        c.flag("flashSequence", &mut self.flash_sequence)?;
        c.flag("ignoreLoadedForReady", &mut self.ignore_loaded_for_ready)?;
        opt(c, "emitter", &mut self.emitter, |c, e| {
            crate::codec::datablock_id(c, "emitter", &mut e.emitter)?;
            float(c, "emitterTime", &mut e.time)?;
            c.raw32("emitterNode", &mut e.node)
        })?;
        db_ref(c, "sound", &mut self.sound)
    }
}

/// The light block of an image (sent when its type is not `NoLight`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageLight {
    pub radius: F32,
    /// An `S32`'s 32 bits.
    pub time: u32,
    /// `writeFloat(channel, 7)` of red, green, blue, alpha.
    pub color: [u32; 4],
}

/// `ShapeBaseImageData::packData`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShapeBaseImageData {
    /// `flag(computeCRC)`: the shape's CRC.
    pub crc: Option<u32>,
    pub emap: bool,
    pub shape_name: Vec<u8>,
    pub mount_point: u32,
    /// `None`: the identity.
    pub mount_offset: Option<Xf>,
    /// `None`: the identity.
    pub eye_offset: Option<Xf>,
    pub correct_muzzle_vector: bool,
    pub first_person: bool,
    pub mass: F32,
    pub uses_energy: bool,
    pub min_energy: F32,
    pub has_flash: bool,
    pub projectile: Option<u32>,
    pub cloakable: bool,
    /// `ranged(lightType, 0, 3)`; the light block follows when it is not 0.
    pub light_type: u32,
    pub light: ImageLight,
    pub shell_exit_dir: [F32; 3],
    pub shell_exit_variance: F32,
    pub shell_velocity: F32,
    pub casing: Option<u32>,
    /// The state table, boxed (31 states make the record large).
    pub states: Box<[Option<ImageState>; IMAGE_STATES]>,
}

/// `flag(is the identity)` and, when clear, the transform.
fn identity_or<C: Codec>(c: &mut C, name: &'static str, v: &mut Option<Xf>) -> R {
    let mut identity = v.is_none();
    c.flag(name, &mut identity)?;
    if identity {
        *v = None;
        return Ok(());
    }
    c.xform(name, v.get_or_insert_with(Xf::default))
}

impl Layout for ShapeBaseImageData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        opt(c, "computeCRC", &mut self.crc, |c, v| c.raw32("mCRC", v))?;
        c.flag("emap", &mut self.emap)?;
        c.string("shapeName", &mut self.shape_name)?;
        c.raw32("mountPoint", &mut self.mount_point)?;
        identity_or(c, "mountOffset", &mut self.mount_offset)?;
        identity_or(c, "eyeOffset", &mut self.eye_offset)?;
        c.flag("correctMuzzleVector", &mut self.correct_muzzle_vector)?;
        c.flag("firstPerson", &mut self.first_person)?;
        float(c, "mass", &mut self.mass)?;
        c.flag("usesEnergy", &mut self.uses_energy)?;
        float(c, "minEnergy", &mut self.min_energy)?;
        c.flag("hasFlash", &mut self.has_flash)?;
        db_ref(c, "projectile", &mut self.projectile)?;
        c.flag("cloakable", &mut self.cloakable)?;
        c.ranged("lightType", &mut self.light_type, 0, IMAGE_LIGHT_TYPES - 1)?;
        if self.light_type == 0 {
            self.light = ImageLight::default();
        } else {
            float(c, "lightRadius", &mut self.light.radius)?;
            c.raw32("lightTime", &mut self.light.time)?;
            float_ints(c, "lightColor", &mut self.light.color, 7)?;
        }
        c.floats("shellExitDir", &mut self.shell_exit_dir)?;
        float(c, "shellExitVariance", &mut self.shell_exit_variance)?;
        float(c, "shellVelocity", &mut self.shell_velocity)?;
        db_ref(c, "casing", &mut self.casing)?;
        for state in self.states.iter_mut() {
            opt(c, "state", state, |c, s| s.layout(c))?;
        }
        Ok(())
    }
}

fn pack_state(v: &dyn ObjectView, i: usize, w: NetWidths) -> Option<ImageState> {
    let name = text(v, "state.name", i);
    if name.is_empty() {
        return None;
    }
    let tr =
        |field: &str, k: usize| mask_bits((int(v, field, k) as u32).wrapping_add(1), STATE_BITS);
    let pair = |field: &str| [tr(field, 2 * i), tr(field, 2 * i + 1)];
    let sequence = |field: &str| {
        let s = int(v, field, i);
        (s != -1).then(|| Signed::of(s as i32, 15))
    };
    Some(ImageState {
        name,
        transition: Transitions {
            loaded: pair("state.transition.loaded"),
            ammo: pair("state.transition.ammo"),
            target: pair("state.transition.target"),
            wet: pair("state.transition.wet"),
            trigger: pair("state.transition.trigger"),
            timeout: tr("state.transition.timeout", i),
        },
        timeout_value: unless(fld(v, "state.timeoutValue", i), 0.0),
        wait_for_timeout: boolean(v, "state.waitForTimeout", i),
        fire: boolean(v, "state.fire", i),
        eject_shell: boolean(v, "state.ejectShell", i),
        scale_animation: boolean(v, "state.scaleAnimation", i),
        direction: boolean(v, "state.direction", i),
        energy_drain: unless(fld(v, "state.energyDrain", i), 0.0),
        loaded: mask_bits(word(v, "state.loaded", i), 3),
        spin: mask_bits(word(v, "state.spin", i), 3),
        recoil: mask_bits(word(v, "state.recoil", i), 3),
        sequence: sequence("state.sequence"),
        sequence_vis: sequence("state.sequenceVis"),
        flash_sequence: boolean(v, "state.flashSequence", i),
        ignore_loaded_for_ready: boolean(v, "state.ignoreLoadedForReady", i),
        emitter: db(v, "state.emitter", i, w).map(|emitter| StateEmitter {
            emitter,
            time: raw(v, "state.emitterTime", i),
            node: word(v, "state.emitterNode", i),
        }),
        sound: db(v, "state.sound", i, w),
    })
}

/// A transform field: `None` when the view has none (the identity).
fn offset(v: &dyn ObjectView, name: &str) -> Option<Xf> {
    match v.get(name, 0) {
        Value::Xform(Some(_)) => Some(xform(v, name)),
        _ => None,
    }
}

pub fn pack_image_data(v: &dyn ObjectView, w: NetWidths) -> ShapeBaseImageData {
    let light_type = wrap_ranged(word(v, "lightType", 0), 0, IMAGE_LIGHT_TYPES - 1);
    ShapeBaseImageData {
        crc: boolean(v, "computeCRC", 0).then(|| word(v, "mCRC", 0)),
        emap: boolean(v, "emap", 0),
        shape_name: text(v, "shapeName", 0),
        mount_point: word(v, "mountPoint", 0),
        mount_offset: offset(v, "mountOffset"),
        eye_offset: offset(v, "eyeOffset"),
        correct_muzzle_vector: boolean(v, "correctMuzzleVector", 0),
        first_person: boolean(v, "firstPerson", 0),
        mass: raw(v, "mass", 0),
        uses_energy: boolean(v, "usesEnergy", 0),
        min_energy: raw(v, "minEnergy", 0),
        has_flash: boolean(v, "hasFlash", 0),
        projectile: db(v, "projectile", 0, w),
        cloakable: boolean(v, "cloakable", 0),
        light_type,
        light: if light_type == 0 {
            ImageLight::default()
        } else {
            ImageLight {
                radius: raw(v, "lightRadius", 0),
                time: word(v, "lightTime", 0),
                color: float_ints_of(v, "lightColor", 0, 7),
            }
        },
        shell_exit_dir: raws(v, "shellExitDir", 0),
        shell_exit_variance: raw(v, "shellExitVariance", 0),
        shell_velocity: raw(v, "shellVelocity", 0),
        casing: db(v, "casing", 0, w),
        states: Box::new(std::array::from_fn(|i| pack_state(v, i, w))),
    }
}
