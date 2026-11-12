//! [`TgeDatablocks`], the engine's default [`DatablockCodecSet`]: TGE 1.4's 35 networked datablock classes
//! ([`TGE_DATABLOCK_CLASSES`](crate::tge::TGE_DATABLOCK_CLASSES)) with their records. Several classes send a parent's record
//! unchanged: `CameraData`, `PathCameraData` and `MissionMarkerData` send `ShapeBaseData`'s, and `SimDataBlock` and
//! `GameBaseData` send nothing.
//!
//! A packer reads the datablock by its members' names; [`TgeDatablockClass::script_field`] names the script field each member
//! is registered as, for a host whose view answers by field.
//!
//! Credit: the field names are the ones TGE 1.4's `initPersistFields` registers (`audio/audioDataBlock.cc`,
//! `game/shapeBase.cc`, `game/shapeImage.cc`, `game/player.cc`, `game/debris.cc`, `game/fx/explosion.cc`, `game/fx/splash.cc`,
//! `game/fx/lightning.cc`, `game/fx/particleEngine.cc`, `game/fx/precipitation.cc`, `game/item.cc`, `game/staticShape.cc`,
//! `game/vehicles/*.cc` and `ts/tsShapeConstruct.cc`, Copyright (C) GarageGames.com, Inc.; `interior/pathedInterior.cc`,
//! which carries no copyright notice; `game/fx/fxLight.cc` by Melvyn May), the ones Torque3D's MIT-licensed release keeps
//! in the same files (for example `T3D/shapeBase.cpp`, Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust
//! Logic, Inc.). The image state table's per-state arrays (`stateTrans`, `stateSequenceVis`, `stateFlashSequence`,
//! `stateScaleAnimation`) are named as the host's value arrays hold them.

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::huffman::Table;

use super::DatablockCodecSet;
use super::audio::{
    AudioDescription, AudioEnvironment, AudioProfile, AudioSampleEnvironment,
    pack_audio_description, pack_audio_environment, pack_audio_profile,
    pack_audio_sample_environment,
};
use super::effects::{
    DebrisData, DecalData, ExplosionData, ParticleData, ParticleEmitterData,
    ParticleEmitterNodeData, ProjectileData, SplashData, pack_debris_data, pack_decal_data,
    pack_explosion_data, pack_particle_data, pack_particle_emitter_data,
    pack_particle_emitter_node_data, pack_projectile_data, pack_splash_data,
};
use super::image::{ShapeBaseImageData, pack_image_data};
use super::misc::{
    NoData, PathedInteriorData, TriggerData, TsShapeConstructor, pack_pathed_interior_data,
    pack_trigger_data, pack_ts_shape_constructor,
};
use super::shapes::{
    ItemData, PlayerData, ShapeBaseData, StaticShapeData, pack_item_data, pack_player_data,
    pack_shapebase_data, pack_static_shape_data,
};
use super::vehicles::{
    FlyingVehicleData, HoverVehicleData, WheeledVehicleData, WheeledVehicleSpring,
    WheeledVehicleTire, pack_flying_vehicle_data, pack_hover_vehicle_data, pack_spring, pack_tire,
    pack_wheeled_vehicle_data,
};
use super::weather::{
    FireballAtmosphereData, FxLightData, LightningData, PrecipitationData, WeatherLightningData,
    pack_fireball_atmosphere_data, pack_fx_light_data, pack_lightning_data,
    pack_precipitation_data, pack_weather_lightning_data,
};
use crate::codec::{Codec, Layout, NetWidths, R, ReadCtx, decode, encode};
use crate::plugin::{PluginClass, Record};
use crate::{ChargeFn, ObjectView, PackCtx};

/// Declares the class enum, the record enum and their dispatch from one table: `Variant = "ClassName" (Record, pack)`.
macro_rules! datablock_classes {
    ($($variant:ident = $name:literal ($rec:ty, $pack:expr)),+ $(,)?) => {
        /// A TGE datablock class.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum TgeDatablockClass {
            $($variant,)+
            /// A plugin's datablock class ([`crate::plugin`]): its class id past TGE's table.
            Plugin(PluginClass),
        }

        impl TgeDatablockClass {
            /// Every TGE class, in the order of TGE's class list (a plugin's classes are its set's).
            pub const ALL: &'static [TgeDatablockClass] = &[$(TgeDatablockClass::$variant),+];

            /// The engine's class name.
            pub fn name(self) -> &'static str {
                match self {
                    $(TgeDatablockClass::$variant => $name,)+
                    TgeDatablockClass::Plugin(class) => class.name(),
                }
            }
        }

        /// A record of any TGE datablock class.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum TgeDatablockRecord {
            $($variant($rec),)+
            /// A plugin class's record ([`crate::plugin`]).
            Plugin(PluginClass, Box<dyn Record>),
        }

        impl TgeDatablockRecord {
            pub fn class(&self) -> TgeDatablockClass {
                match self {
                    $(TgeDatablockRecord::$variant(_) => TgeDatablockClass::$variant,)+
                    TgeDatablockRecord::Plugin(class, _) => TgeDatablockClass::Plugin(*class),
                }
            }

            /// Writes the record in the connection's `widths`.
            pub fn encode(&self, w: &mut BitWriter, table: &Table, widths: NetWidths) -> R {
                match self {
                    $(TgeDatablockRecord::$variant(r) => encode(r, w, table, widths),)+
                    TgeDatablockRecord::Plugin(_, r) => r.encode(w, table, widths),
                }
            }

            /// Runs the record's layout on `c` (on a copy: the layout takes `&mut`). A plugin's record runs nothing: its
            /// fields are its own ([`Record::record_fields`]).
            pub fn visit<C: Codec>(&self, c: &mut C) -> R {
                match self {
                    $(TgeDatablockRecord::$variant(r) => r.clone().layout(c),)+
                    TgeDatablockRecord::Plugin(..) => Ok(()),
                }
            }
        }

        fn pack_class(class: TgeDatablockClass, v: &dyn ObjectView, w: NetWidths) -> TgeDatablockRecord {
            match class {
                $(TgeDatablockClass::$variant => {
                    let pack: fn(&dyn ObjectView, NetWidths) -> $rec = $pack;
                    TgeDatablockRecord::$variant(pack(v, w))
                },)+
                TgeDatablockClass::Plugin(class) => {
                    TgeDatablockRecord::Plugin(class, class.pack(v, !0, &PackCtx::new(w)).0)
                }
            }
        }

        fn decode_class(
            class: TgeDatablockClass,
            r: &mut BitReader,
            table: &Table,
            charge: &mut ChargeFn,
            ctx: &ReadCtx,
        ) -> Result<TgeDatablockRecord, NetError> {
            match class {
                $(TgeDatablockClass::$variant => decode::<$rec>(r, table, charge, ctx).map(TgeDatablockRecord::$variant),)+
                TgeDatablockClass::Plugin(class) => class
                    .decode(r, table, charge, ctx)
                    .map(|rec| TgeDatablockRecord::Plugin(class, rec)),
            }
        }
    };
}

datablock_classes! {
    AudioDescription = "AudioDescription" (AudioDescription, |v, _| pack_audio_description(v)),
    AudioEnvironment = "AudioEnvironment" (AudioEnvironment, |v, _| pack_audio_environment(v)),
    AudioProfile = "AudioProfile" (AudioProfile, pack_audio_profile),
    AudioSampleEnvironment = "AudioSampleEnvironment" (AudioSampleEnvironment, |v, _| pack_audio_sample_environment(v)),
    CameraData = "CameraData" (ShapeBaseData, pack_shapebase_data),
    DebrisData = "DebrisData" (DebrisData, pack_debris_data),
    DecalData = "DecalData" (DecalData, |v, w| pack_decal_data(v).for_widths(w)),
    ExplosionData = "ExplosionData" (ExplosionData, pack_explosion_data),
    FireballAtmosphereData = "FireballAtmosphereData" (FireballAtmosphereData, pack_fireball_atmosphere_data),
    FlyingVehicleData = "FlyingVehicleData" (FlyingVehicleData, pack_flying_vehicle_data),
    GameBaseData = "GameBaseData" (NoData, |_, _| NoData),
    HoverVehicleData = "HoverVehicleData" (HoverVehicleData, pack_hover_vehicle_data),
    ItemData = "ItemData" (ItemData, pack_item_data),
    LightningData = "LightningData" (LightningData, pack_lightning_data),
    MissionMarkerData = "MissionMarkerData" (ShapeBaseData, pack_shapebase_data),
    ParticleData = "ParticleData" (ParticleData, |v, w| pack_particle_data(v).for_widths(w)),
    ParticleEmitterData = "ParticleEmitterData" (ParticleEmitterData, |v, w| pack_particle_emitter_data(v).for_widths(w)),
    ParticleEmitterNodeData = "ParticleEmitterNodeData" (ParticleEmitterNodeData, |v, _| pack_particle_emitter_node_data(v)),
    PathCameraData = "PathCameraData" (ShapeBaseData, pack_shapebase_data),
    PathedInteriorData = "PathedInteriorData" (PathedInteriorData, pack_pathed_interior_data),
    PlayerData = "PlayerData" (PlayerData, pack_player_data),
    PrecipitationData = "PrecipitationData" (PrecipitationData, pack_precipitation_data),
    ProjectileData = "ProjectileData" (ProjectileData, pack_projectile_data),
    ShapeBaseData = "ShapeBaseData" (ShapeBaseData, pack_shapebase_data),
    ShapeBaseImageData = "ShapeBaseImageData" (ShapeBaseImageData, pack_image_data),
    SimDataBlock = "SimDataBlock" (NoData, |_, _| NoData),
    SplashData = "SplashData" (SplashData, pack_splash_data),
    StaticShapeData = "StaticShapeData" (StaticShapeData, pack_static_shape_data),
    TsShapeConstructor = "TSShapeConstructor" (TsShapeConstructor, |v, _| pack_ts_shape_constructor(v)),
    TriggerData = "TriggerData" (TriggerData, |v, _| pack_trigger_data(v)),
    WeatherLightningData = "WeatherLightningData" (WeatherLightningData, pack_weather_lightning_data),
    WheeledVehicleData = "WheeledVehicleData" (WheeledVehicleData, pack_wheeled_vehicle_data),
    WheeledVehicleSpring = "WheeledVehicleSpring" (WheeledVehicleSpring, |v, _| pack_spring(v)),
    WheeledVehicleTire = "WheeledVehicleTire" (WheeledVehicleTire, |v, _| pack_tire(v)),
    FxLightData = "fxLightData" (FxLightData, |v, _| pack_fx_light_data(v)),
}

/// The engine's default set of datablock classes.
#[derive(Debug, Clone, Copy, Default)]
pub struct TgeDatablocks;

impl DatablockCodecSet for TgeDatablocks {
    type Class = TgeDatablockClass;
    type Record = TgeDatablockRecord;

    fn classes() -> &'static [TgeDatablockClass] {
        TgeDatablockClass::ALL
    }

    fn name(class: TgeDatablockClass) -> &'static str {
        class.name()
    }

    fn class_of(record: &TgeDatablockRecord) -> TgeDatablockClass {
        record.class()
    }

    fn pack(
        class: TgeDatablockClass,
        view: &dyn ObjectView,
        widths: NetWidths,
    ) -> TgeDatablockRecord {
        pack_class(class, view, widths)
    }

    fn decode(
        class: TgeDatablockClass,
        r: &mut BitReader,
        table: &Table,
        charge: &mut ChargeFn,
        widths: NetWidths,
    ) -> Result<TgeDatablockRecord, NetError> {
        decode_class(class, r, table, charge, &ReadCtx::new(widths, true))
    }

    fn encode(
        record: &TgeDatablockRecord,
        w: &mut BitWriter,
        table: &Table,
        widths: NetWidths,
    ) -> R {
        record.encode(w, table, widths)
    }
}

/// Where a script keeps the member a packer reads ([`TgeDatablockClass::script_field`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptField {
    /// The field has the member's own name, at the same index.
    Own,
    /// The field and the index in it.
    Field(&'static str, usize),
    /// No field holds that element (an array member with fewer named fields).
    Absent,
}

/// `PlayerData::sound[]`'s fields, in the order of `PlayerData::Sounds`.
const PLAYER_SOUNDS: [&str; 18] = [
    "FootSoftSound",
    "FootHardSound",
    "FootMetalSound",
    "FootSnowSound",
    "FootShallowSound",
    "FootWadingSound",
    "FootUnderwaterSound",
    "FootBubblesSound",
    "movingBubblesSound",
    "waterBreathSound",
    "impactSoftSound",
    "impactHardSound",
    "impactMetalSound",
    "impactSnowSound",
    "impactWaterEasy",
    "impactWaterMedium",
    "impactWaterHard",
    "exitingWater",
];

/// `VehicleData::waterSound[]`'s fields (`ExitWater`, `ImpactSoft`, `ImpactMedium`, `ImpactHard`, `Wake`).
const VEHICLE_WATER_SOUNDS: [&str; 5] = [
    "exitingWater",
    "impactWaterEasy",
    "impactWaterMedium",
    "impactWaterHard",
    "waterWakeSound",
];

impl TgeDatablockClass {
    /// Whether the class is `ShapeBaseData` or one of its subclasses.
    pub fn is_shape_base(self) -> bool {
        use TgeDatablockClass::*;
        matches!(
            self,
            CameraData
                | ItemData
                | MissionMarkerData
                | PathCameraData
                | PlayerData
                | ShapeBaseData
                | StaticShapeData
                | FlyingVehicleData
                | HoverVehicleData
                | WheeledVehicleData
        )
    }

    /// The script field that holds element `index` of the member a packer reads as `member` (TGE 1.4's `initPersistFields`):
    /// most members are registered under their own name; the ones registered otherwise, an enum-indexed array's named fields,
    /// and a `ShapeBaseImageData`'s state members (the `state*` arrays, the transitions eleven to a state in the wire's order)
    /// are listed here.
    pub fn script_field(self, member: &str, index: usize) -> ScriptField {
        use ScriptField::{Absent, Field, Own};
        use TgeDatablockClass::*;
        let field = |name: &'static str| Field(name, index);
        let named = |names: &[&'static str]| names.get(index).map_or(Absent, |&n| Field(n, 0));
        let vehicle = matches!(
            self,
            FlyingVehicleData | HoverVehicleData | WheeledVehicleData
        );
        let splashing = vehicle || self == PlayerData;
        let unprefixed = member.strip_prefix('m').filter(|m| !m.is_empty());
        if let (ShapeBaseImageData, Some(state)) = (self, member.strip_prefix("state.")) {
            return image_state_field(state, index);
        }
        match (self, member) {
            (AudioDescription | AudioEnvironment | AudioSampleEnvironment, _)
                if unprefixed.is_some() =>
            {
                unprefixed
                    .and_then(audio_field)
                    .map_or(Own, |n| Field(n, index))
            }
            (AudioProfile, "mFilename") => field("filename"),
            (AudioProfile, "mDescriptionObject") => field("description"),
            (AudioProfile, "mSampleEnvironment") => field("environment"),
            (FxLightData, "mFlareTextureName") => field("FlareBitmap"),
            (FxLightData, "mUseColour") => field("AnimColour"),
            (FxLightData, "mUseBrightness") => field("AnimBrightness"),
            (FxLightData, "mUseRadius") => field("AnimRadius"),
            (FxLightData, "mUseOffsets") => field("AnimOffsets"),
            (FxLightData, "mUseRotation") => field("AnimRotation"),
            (FxLightData, _) if unprefixed.is_some() => FX_LIGHT_FIELDS
                .iter()
                .find(|n| Some(**n) == unprefixed)
                .map_or(Own, |&n| Field(n, index)),
            (PrecipitationData, "mDropName") => field("dropTexture"),
            (PrecipitationData, "mSplashName") => field("splashTexture"),
            (PrecipitationData, "mDropSize") => field("dropSize"),
            (PrecipitationData, "mSplashSize") => field("splashSize"),
            (PrecipitationData, "mSplashMS") => field("splashMS"),
            (PrecipitationData, "mUseTrueBillboards") => field("useTrueBillboards"),
            (TsShapeConstructor, "mShape") => field("baseShape"),
            (TsShapeConstructor, "mSequence") => field("sequence"),
            (DebrisData | SplashData, "textureName") => field("texture"),
            (DebrisData, "emitterList") => field("emitters"),
            (ExplosionData | SplashData, "emitterList") => field("emitter"),
            (ExplosionData, "dtsFileName") => field("explosionShape"),
            (ExplosionData, "debrisList") => field("debris"),
            (ExplosionData, "explosionList") => field("subExplosion"),
            (LightningData, "strikeTextureNames") => field("strikeTextures"),
            (ParticleData, "textureNameList") => field("animTexName"),
            (ParticleData | ParticleEmitterData, "ext") => Absent,
            (_, "distortionExt") => Absent,
            (ItemData | StaticShapeData, "dynamicTypeField") => field("dynamicType"),
            (DebrisData | ShapeBaseImageData | WheeledVehicleTire, "shapeName") => {
                field("shapeFile")
            }
            (_, "shapeName") if self.is_shape_base() => field("shapeFile"),
            (_, "cloakTexName") if self.is_shape_base() => field("cloakTexture"),
            (ShapeBaseImageData, "mountOffset") => field("offset"),
            (PlayerData, "boxSize") => field("boundingBox"),
            (PlayerData, "sound") => named(&PLAYER_SOUNDS),
            (PlayerData, "footSplashHeight") => field("footstepSplashHeight"),
            (_, "splashEmitterList") if splashing => field("splashEmitter"),
            (_, "exitSplashSoundVel") if splashing => field("exitSplashSoundVelocity"),
            (_, "medSplashSoundVel") if splashing => field("mediumSplashSoundVelocity"),
            (_, "hardSplashSoundVel") if splashing => field("hardSplashSoundVelocity"),
            (_, "softSplashSoundVel") if vehicle => field("softSplashSoundVelocity"),
            (_, "body.restitution") if vehicle => field("bodyRestitution"),
            (_, "body.friction") if vehicle => field("bodyFriction"),
            (_, "body.sound") if vehicle => named(&["softImpactSound", "hardImpactSound"]),
            (_, "damageEmitterList") if vehicle => field("damageEmitter"),
            (_, "waterSound") if vehicle => named(&VEHICLE_WATER_SOUNDS),
            (FlyingVehicleData, "sound") => named(&["jetSound", "engineSound"]),
            (FlyingVehicleData, "jetEmitter") => named(&[
                "forwardJetEmitter",
                "backwardJetEmitter",
                "downJetEmitter",
                "trailEmitter",
            ]),
            (HoverVehicleData, "sound") => named(&["jetSound", "engineSound", "floatSound"]),
            (HoverVehicleData, "jetEmitter") => named(&["forwardJetEmitter"]),
            (WheeledVehicleData, "sound") => {
                named(&["jetSound", "engineSound", "squealSound", "WheelImpactSound"])
            }
            (WheeledVehicleTire, "longitudinalRelaxation") => field("logitudinalRelaxation"),
            (WheeledVehicleSpring, "antiSway") => field("antiSwayForce"),
            (PathedInteriorData, "sound") => named(&["StartSound", "SustainSound", "StopSound"]),
            _ => Own,
        }
    }
}

/// `fxLightData`'s fields that are its members' names without the `m`.
const FX_LIGHT_FIELDS: [&str; 45] = [
    "LightOn",
    "Radius",
    "Brightness",
    "Colour",
    "FlareOn",
    "FlareTP",
    "FlareColour",
    "ConstantSizeOn",
    "ConstantSize",
    "NearSize",
    "FarSize",
    "NearDistance",
    "FarDistance",
    "FadeTime",
    "BlendMode",
    "LinkFlare",
    "LinkFlareSize",
    "MinColour",
    "MaxColour",
    "MinBrightness",
    "MaxBrightness",
    "MinRadius",
    "MaxRadius",
    "StartOffset",
    "EndOffset",
    "MinRotation",
    "MaxRotation",
    "SingleColourKeys",
    "RedKeys",
    "GreenKeys",
    "BlueKeys",
    "BrightnessKeys",
    "RadiusKeys",
    "OffsetKeys",
    "RotationKeys",
    "ColourTime",
    "BrightnessTime",
    "RadiusTime",
    "OffsetTime",
    "RotationTime",
    "LerpColour",
    "LerpBrightness",
    "LerpRadius",
    "LerpOffset",
    "LerpRotation",
];

/// The audio datablocks' fields: their members' names without the `m`, the first letter lower case.
const AUDIO_FIELDS: [&str; 41] = [
    "volume",
    "isLooping",
    "isStreaming",
    "is3D",
    "referenceDistance",
    "maxDistance",
    "coneInsideAngle",
    "coneOutsideAngle",
    "coneOutsideVolume",
    "coneVector",
    "environmentLevel",
    "loopCount",
    "minLoopGap",
    "maxLoopGap",
    "type",
    "useRoom",
    "room",
    "roomHF",
    "reflections",
    "reverb",
    "roomRolloffFactor",
    "decayTime",
    "decayHFRatio",
    "reflectionsDelay",
    "reverbDelay",
    "roomVolume",
    "effectVolume",
    "damping",
    "environmentSize",
    "environmentDiffusion",
    "airAbsorption",
    "flags",
    "direct",
    "directHF",
    "obstruction",
    "obstructionLFRatio",
    "occlusion",
    "occlusionLFRatio",
    "occlusionRoomRatio",
    "roomRolloff",
    "outsideVolumeHF",
];

/// [`AUDIO_FIELDS`]' entry for a member's name without its `m` (`Volume` -> `volume`).
fn audio_field(name: &str) -> Option<&'static str> {
    let mut chars = name.chars();
    let first = chars.next()?.to_ascii_lowercase();
    let rest = chars.as_str();
    AUDIO_FIELDS
        .iter()
        .find(|n| n.starts_with(first) && n.get(1..) == Some(rest))
        .copied()
}

/// A `ShapeBaseImageData` state member (`name`, `fire`, `transition.loaded`, ...) as the host's state arrays hold it: the
/// state's element of `stateName`, `stateFire`, ...; `loaded` and `spin` are `stateLoadedFlag` and `stateSpinThread`; the
/// transitions are `stateTrans`, eleven to a state (`NotLoaded`, `Loaded`, `NoAmmo`, `Ammo`, `NoTarget`, `Target`, `NotWet`,
/// `Wet`, `TriggerUp`, `TriggerDown`, `Timeout`), a pair's element `k` of state `s` read as `2 * s + k`.
fn image_state_field(state: &str, index: usize) -> ScriptField {
    let pair = |at: usize| ScriptField::Field("stateTrans", 11 * (index / 2) + at + index % 2);
    match state {
        "transition.loaded" => pair(0),
        "transition.ammo" => pair(2),
        "transition.target" => pair(4),
        "transition.wet" => pair(6),
        "transition.trigger" => pair(8),
        "transition.timeout" => ScriptField::Field("stateTrans", 11 * index + 10),
        _ => IMAGE_STATE_FIELDS
            .iter()
            .find(|(member, _)| *member == state)
            .map_or(ScriptField::Absent, |&(_, f)| ScriptField::Field(f, index)),
    }
}

/// The image state members and the host's arrays of them.
const IMAGE_STATE_FIELDS: [(&str, &str); 19] = [
    ("name", "stateName"),
    ("timeoutValue", "stateTimeoutValue"),
    ("waitForTimeout", "stateWaitForTimeout"),
    ("fire", "stateFire"),
    ("ejectShell", "stateEjectShell"),
    ("scaleAnimation", "stateScaleAnimation"),
    ("direction", "stateDirection"),
    ("energyDrain", "stateEnergyDrain"),
    ("loaded", "stateLoadedFlag"),
    ("spin", "stateSpinThread"),
    ("recoil", "stateRecoil"),
    ("sequence", "stateSequence"),
    ("sequenceVis", "stateSequenceVis"),
    ("flashSequence", "stateFlashSequence"),
    ("ignoreLoadedForReady", "stateIgnoreLoadedForReady"),
    ("emitter", "stateEmitter"),
    ("emitterTime", "stateEmitterTime"),
    ("emitterNode", "stateEmitterNode"),
    ("sound", "stateSound"),
];
