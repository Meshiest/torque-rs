//! [`TgeGhosts`], the engine's default [`GhostCodecSet`]: TGE 1.4's object classes with their record codecs, and TGE's
//! networked class list ([`TGE_OBJECT_CLASSES`], [`TGE_DATABLOCK_CLASSES`], [`tge_reps`]) that a connection's class ids are
//! derived from (`torque_net::classes::NetClasses::derive`). A class of the list with no codec here (TerrainBlock,
//! InteriorInstance, WaterBlock, ...) stays unsupported, by name.
//!
//! The class ids are the derivation's, so they are only meaningful between two ends that register the same list. The engine's
//! own classes ([`crate::light`]: not TGE's) take the ids past the end of the table ([`GhostCodecSet::class_past_table`]).
//!
//! **Widths.** Every layout reads its id widths from the connection's [`NetWidths`]: TGE 1.4 itself sends 12-bit ghost
//! indices and 10-bit datablock ids (`NetWidths::new(12, 10)`), while torque-net's default profile uses Torque3D's wider
//! ones ([`NetWidths::engine_default`]). The layouts are the same either way.
//!
//! Credit: the class list is the set of networked classes TGE 1.4 registers (`IMPLEMENT_CO_NETOBJECT_V1`,
//! `IMPLEMENT_CO_DATABLOCK_V1`; most of those files Copyright (C) GarageGames.com, Inc., the fx*.cc ones written by Melvyn
//! May), the registration Torque3D carries in
//! `console/consoleObject.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). Each class's net
//! flags ([`TgeClass::net_flags`]) are the ones those files' constructors set; Torque3D keeps the flags in `sim/netObject.h`
//! (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.) and the two constructors most classes inherit in
//! `T3D/gameBase/gameBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.) and
//! `T3D/camera.cpp` (Copyright (c) 2012 GarageGames, LLC).

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::classes::{Direction, Kind, NetRep};
use torque_net::events::EventRegistry;
use torque_net::huffman::Table;

use crate::audio::{self, AudioEmitter};
use crate::codec::{Layout, NetWidths, R, ReadCtx, decode, encode};
use crate::fx::{FxLight, ParticleEmitterNode, pack_fx_light, pack_particle_emitter_node};
use crate::light::{
    ENGINE_OBJECT_CLASSES, ScatterSky, SpotLight, TimeOfDay, pack_scatter_sky, pack_spot_light,
    pack_time_of_day,
};
use crate::markers::{
    MissionMarker, SpawnSphere, WayPoint, pack_mission_marker, pack_spawn_sphere, pack_waypoint,
};
use crate::player::{Player, pack_player};
use crate::projectile::{Projectile, pack_projectile};
use crate::props::{TsStatic, VehicleBlocker, pack_ts_static, pack_vehicle_blocker};
use crate::set::GhostCodecSet;
use crate::shapebase::TgeShapeBase;
use crate::shapes::{Camera, Item, StaticShape, pack_camera, pack_item, pack_static_shape};
use crate::sky::{Sky, Sun, pack_sky, pack_sun};
use crate::tails::TailClass;
use crate::vehicle::{
    FlyingVehicle, HoverVehicle, WheeledVehicle, pack_flying_vehicle, pack_hover_vehicle,
    pack_wheeled_vehicle,
};
use crate::weather::{Lightning, Precipitation, pack_lightning, pack_precipitation};
use crate::zone::{PhysicalZone, pack_physical_zone};
use crate::{ChargeFn, ObjectView, PackCtx};

/// TGE 1.4's networked object classes.
pub const TGE_OBJECT_CLASSES: [&str; 45] = [
    "AIPlayer",
    "AIWheeledVehicle",
    "AudioEmitter",
    "Camera",
    "ClientAudioEmitter",
    "Debris",
    "FireballAtmosphere",
    "FlyingVehicle",
    "GameBase",
    "HoverVehicle",
    "InteriorInstance",
    "InteriorMap",
    "Item",
    "Lightning",
    "Marker",
    "MissionArea",
    "MissionMarker",
    "ParticleEmitterNode",
    "PathCamera",
    "PathedInterior",
    "PhysicalZone",
    "Player",
    "Precipitation",
    "Projectile",
    "ScopeAlwaysShape",
    "ShapeBase",
    "SimpleNetObject",
    "Sky",
    "SpawnSphere",
    "Splash",
    "StaticShape",
    "Sun",
    "TSStatic",
    "TerrainBlock",
    "Trigger",
    "VehicleBlocker",
    "WaterBlock",
    "WayPoint",
    "WeatherLightning",
    "WheeledVehicle",
    "fxFoliageReplicator",
    "fxLight",
    "fxShapeReplicatedStatic",
    "fxShapeReplicator",
    "fxSunLight",
];

/// TGE 1.4's networked datablock classes.
pub const TGE_DATABLOCK_CLASSES: [&str; 35] = [
    "AudioDescription",
    "AudioEnvironment",
    "AudioProfile",
    "AudioSampleEnvironment",
    "CameraData",
    "DebrisData",
    "DecalData",
    "ExplosionData",
    "FireballAtmosphereData",
    "FlyingVehicleData",
    "GameBaseData",
    "HoverVehicleData",
    "ItemData",
    "LightningData",
    "MissionMarkerData",
    "ParticleData",
    "ParticleEmitterData",
    "ParticleEmitterNodeData",
    "PathCameraData",
    "PathedInteriorData",
    "PlayerData",
    "PrecipitationData",
    "ProjectileData",
    "ShapeBaseData",
    "ShapeBaseImageData",
    "SimDataBlock",
    "SplashData",
    "StaticShapeData",
    "TSShapeConstructor",
    "TriggerData",
    "WeatherLightningData",
    "WheeledVehicleData",
    "WheeledVehicleSpring",
    "WheeledVehicleTire",
    "fxLightData",
];

/// The class representatives of TGE's networked classes: the objects, the datablocks and `events`' event classes.
pub fn tge_reps(events: &EventRegistry) -> Vec<NetRep> {
    let rep = |name: &str, kind| NetRep {
        name: name.to_string(),
        kind,
        direction: Direction::Any,
    };
    let mut reps: Vec<NetRep> = TGE_OBJECT_CLASSES
        .iter()
        .map(|n| rep(n, Kind::Object))
        .chain(
            TGE_DATABLOCK_CLASSES
                .iter()
                .map(|n| rep(n, Kind::DataBlock)),
        )
        .collect();
    reps.extend(events.classes().iter().map(|(name, direction)| NetRep {
        name: name.clone(),
        kind: Kind::Event,
        direction: *direction,
    }));
    reps
}

/// A TGE object class with a record codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TgeClass {
    AiPlayer,
    AiWheeledVehicle,
    AudioEmitter,
    Camera,
    FlyingVehicle,
    HoverVehicle,
    Item,
    Lightning,
    MissionMarker,
    ParticleEmitterNode,
    PhysicalZone,
    Player,
    Precipitation,
    Projectile,
    ScopeAlwaysShape,
    Sky,
    SpawnSphere,
    StaticShape,
    Sun,
    TsStatic,
    VehicleBlocker,
    WayPoint,
    WheeledVehicle,
    FxLight,
    /// The engine's own classes ([`crate::light`]): ghosted to the engine's own clients only, their class ids past TGE's.
    SpotLight,
    TimeOfDay,
    ScatterSky,
}

impl TgeClass {
    pub const ALL: [TgeClass; 27] = [
        TgeClass::AiPlayer,
        TgeClass::AiWheeledVehicle,
        TgeClass::AudioEmitter,
        TgeClass::Camera,
        TgeClass::FlyingVehicle,
        TgeClass::HoverVehicle,
        TgeClass::Item,
        TgeClass::Lightning,
        TgeClass::MissionMarker,
        TgeClass::ParticleEmitterNode,
        TgeClass::PhysicalZone,
        TgeClass::Player,
        TgeClass::Precipitation,
        TgeClass::Projectile,
        TgeClass::ScopeAlwaysShape,
        TgeClass::Sky,
        TgeClass::SpawnSphere,
        TgeClass::StaticShape,
        TgeClass::Sun,
        TgeClass::TsStatic,
        TgeClass::VehicleBlocker,
        TgeClass::WayPoint,
        TgeClass::WheeledVehicle,
        TgeClass::FxLight,
        TgeClass::SpotLight,
        TgeClass::TimeOfDay,
        TgeClass::ScatterSky,
    ];

    /// The engine's class name.
    pub fn name(self) -> &'static str {
        match self {
            TgeClass::AiPlayer => "AIPlayer",
            TgeClass::AiWheeledVehicle => "AIWheeledVehicle",
            TgeClass::AudioEmitter => "AudioEmitter",
            TgeClass::Camera => "Camera",
            TgeClass::FlyingVehicle => "FlyingVehicle",
            TgeClass::HoverVehicle => "HoverVehicle",
            TgeClass::Item => "Item",
            TgeClass::Lightning => "Lightning",
            TgeClass::MissionMarker => "MissionMarker",
            TgeClass::ParticleEmitterNode => "ParticleEmitterNode",
            TgeClass::PhysicalZone => "PhysicalZone",
            TgeClass::Player => "Player",
            TgeClass::Precipitation => "Precipitation",
            TgeClass::Projectile => "Projectile",
            TgeClass::ScopeAlwaysShape => "ScopeAlwaysShape",
            TgeClass::Sky => "Sky",
            TgeClass::SpawnSphere => "SpawnSphere",
            TgeClass::StaticShape => "StaticShape",
            TgeClass::Sun => "Sun",
            TgeClass::TsStatic => "TSStatic",
            TgeClass::VehicleBlocker => "VehicleBlocker",
            TgeClass::WayPoint => "WayPoint",
            TgeClass::WheeledVehicle => "WheeledVehicle",
            TgeClass::FxLight => "fxLight",
            TgeClass::SpotLight => "SpotLight",
            TgeClass::TimeOfDay => "TimeOfDay",
            TgeClass::ScatterSky => "ScatterSky",
        }
    }

    /// The class's place in [`TgeClass::ALL`] (a ghost registry's number for it).
    pub fn code(self) -> u16 {
        TgeClass::ALL.iter().position(|&c| c == self).unwrap_or(0) as u16
    }

    /// The class of a [`code`](Self::code).
    pub fn of_code(code: u16) -> Option<TgeClass> {
        TgeClass::ALL.get(usize::from(code)).copied()
    }

    /// `mNetFlags` as a server object of the class has it after its constructor chain: `GameBase` sets Ghostable (Camera's
    /// constructor clears it again; only the editor sets it back), and the scenery, markers, zones, weather, the sky, the sun,
    /// lights and audio emitters add ScopeAlways in their own constructors. No TGE class sets ScopeLocal, and no `onAdd`
    /// changes the flags.
    pub fn net_flags(self) -> u32 {
        use torque_net::ghost::net_flags::{GHOSTABLE, SCOPE_ALWAYS};
        match self {
            TgeClass::Camera => 0,
            TgeClass::AiPlayer
            | TgeClass::AiWheeledVehicle
            | TgeClass::FlyingVehicle
            | TgeClass::HoverVehicle
            | TgeClass::Item
            | TgeClass::ParticleEmitterNode
            | TgeClass::Player
            | TgeClass::Projectile
            | TgeClass::StaticShape
            | TgeClass::WheeledVehicle => GHOSTABLE,
            TgeClass::AudioEmitter
            | TgeClass::Lightning
            | TgeClass::MissionMarker
            | TgeClass::PhysicalZone
            | TgeClass::Precipitation
            | TgeClass::ScopeAlwaysShape
            | TgeClass::Sky
            | TgeClass::SpawnSphere
            | TgeClass::Sun
            | TgeClass::TsStatic
            | TgeClass::VehicleBlocker
            | TgeClass::WayPoint
            | TgeClass::FxLight
            | TgeClass::SpotLight
            | TgeClass::TimeOfDay
            | TgeClass::ScatterSky => GHOSTABLE | SCOPE_ALWAYS,
        }
    }

    /// The class of the packet data an object of this class sends as a control object; a WheeledVehicle needs its datablock's
    /// wheel count (`None` then).
    pub fn tail_class(self, wheels: Option<usize>) -> Option<TailClass> {
        match self {
            TgeClass::Camera => Some(TailClass::Camera),
            TgeClass::Player | TgeClass::AiPlayer => Some(TailClass::Player),
            TgeClass::FlyingVehicle | TgeClass::HoverVehicle => Some(TailClass::Vehicle),
            TgeClass::WheeledVehicle | TgeClass::AiWheeledVehicle => {
                wheels.map(|wheels| TailClass::Wheeled { wheels })
            }
            TgeClass::Item | TgeClass::StaticShape | TgeClass::ScopeAlwaysShape => {
                Some(TailClass::Shape)
            }
            _ => None,
        }
    }
}

/// A record of any TGE class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TgeRecord {
    /// AIPlayer sends Player's record.
    AiPlayer(Player),
    /// AIWheeledVehicle sends WheeledVehicle's record.
    AiWheeledVehicle(WheeledVehicle),
    AudioEmitter(AudioEmitter),
    Camera(Camera),
    FlyingVehicle(FlyingVehicle),
    HoverVehicle(HoverVehicle),
    Item(Item),
    Lightning(Lightning),
    MissionMarker(MissionMarker<TgeShapeBase>),
    ParticleEmitterNode(ParticleEmitterNode),
    PhysicalZone(PhysicalZone),
    Player(Player),
    Precipitation(Precipitation),
    Projectile(Projectile),
    /// ScopeAlwaysShape sends StaticShape's record.
    ScopeAlwaysShape(StaticShape),
    Sky(Sky),
    SpawnSphere(SpawnSphere<TgeShapeBase>),
    StaticShape(StaticShape),
    Sun(Sun),
    TsStatic(TsStatic),
    VehicleBlocker(VehicleBlocker),
    WayPoint(WayPoint<TgeShapeBase>),
    WheeledVehicle(WheeledVehicle),
    FxLight(FxLight),
    SpotLight(SpotLight),
    TimeOfDay(TimeOfDay),
    ScatterSky(ScatterSky),
}

/// Runs `$body` with `$r` bound to the record inside any variant.
macro_rules! each_record {
    ($rec:expr, $r:ident => $body:expr) => {
        match $rec {
            TgeRecord::AiPlayer($r) => $body,
            TgeRecord::AiWheeledVehicle($r) => $body,
            TgeRecord::AudioEmitter($r) => $body,
            TgeRecord::Camera($r) => $body,
            TgeRecord::FlyingVehicle($r) => $body,
            TgeRecord::HoverVehicle($r) => $body,
            TgeRecord::Item($r) => $body,
            TgeRecord::Lightning($r) => $body,
            TgeRecord::MissionMarker($r) => $body,
            TgeRecord::ParticleEmitterNode($r) => $body,
            TgeRecord::PhysicalZone($r) => $body,
            TgeRecord::Player($r) => $body,
            TgeRecord::Precipitation($r) => $body,
            TgeRecord::Projectile($r) => $body,
            TgeRecord::ScopeAlwaysShape($r) => $body,
            TgeRecord::Sky($r) => $body,
            TgeRecord::SpawnSphere($r) => $body,
            TgeRecord::StaticShape($r) => $body,
            TgeRecord::Sun($r) => $body,
            TgeRecord::TsStatic($r) => $body,
            TgeRecord::VehicleBlocker($r) => $body,
            TgeRecord::WayPoint($r) => $body,
            TgeRecord::WheeledVehicle($r) => $body,
            TgeRecord::FxLight($r) => $body,
            TgeRecord::SpotLight($r) => $body,
            TgeRecord::TimeOfDay($r) => $body,
            TgeRecord::ScatterSky($r) => $body,
        }
    };
}

impl TgeRecord {
    pub fn class(&self) -> TgeClass {
        match self {
            TgeRecord::AiPlayer(_) => TgeClass::AiPlayer,
            TgeRecord::AiWheeledVehicle(_) => TgeClass::AiWheeledVehicle,
            TgeRecord::AudioEmitter(_) => TgeClass::AudioEmitter,
            TgeRecord::Camera(_) => TgeClass::Camera,
            TgeRecord::FlyingVehicle(_) => TgeClass::FlyingVehicle,
            TgeRecord::HoverVehicle(_) => TgeClass::HoverVehicle,
            TgeRecord::Item(_) => TgeClass::Item,
            TgeRecord::Lightning(_) => TgeClass::Lightning,
            TgeRecord::MissionMarker(_) => TgeClass::MissionMarker,
            TgeRecord::ParticleEmitterNode(_) => TgeClass::ParticleEmitterNode,
            TgeRecord::PhysicalZone(_) => TgeClass::PhysicalZone,
            TgeRecord::Player(_) => TgeClass::Player,
            TgeRecord::Precipitation(_) => TgeClass::Precipitation,
            TgeRecord::Projectile(_) => TgeClass::Projectile,
            TgeRecord::ScopeAlwaysShape(_) => TgeClass::ScopeAlwaysShape,
            TgeRecord::Sky(_) => TgeClass::Sky,
            TgeRecord::SpawnSphere(_) => TgeClass::SpawnSphere,
            TgeRecord::StaticShape(_) => TgeClass::StaticShape,
            TgeRecord::Sun(_) => TgeClass::Sun,
            TgeRecord::TsStatic(_) => TgeClass::TsStatic,
            TgeRecord::VehicleBlocker(_) => TgeClass::VehicleBlocker,
            TgeRecord::WayPoint(_) => TgeClass::WayPoint,
            TgeRecord::WheeledVehicle(_) => TgeClass::WheeledVehicle,
            TgeRecord::FxLight(_) => TgeClass::FxLight,
            TgeRecord::SpotLight(_) => TgeClass::SpotLight,
            TgeRecord::TimeOfDay(_) => TgeClass::TimeOfDay,
            TgeRecord::ScatterSky(_) => TgeClass::ScatterSky,
        }
    }

    /// Writes the record in the connection's `widths`.
    pub fn encode(&self, w: &mut BitWriter, table: &Table, widths: NetWidths) -> R {
        each_record!(self, r => encode(r, w, table, widths))
    }
}

/// The engine's default set of ghost classes.
#[derive(Debug, Clone, Copy, Default)]
pub struct TgeGhosts;

fn read<L: Layout>(
    r: &mut BitReader,
    table: &Table,
    charge: &mut ChargeFn,
    ctx: &ReadCtx,
    wrap: fn(L) -> TgeRecord,
) -> Result<TgeRecord, NetError> {
    decode::<L>(r, table, charge, ctx).map(wrap)
}

impl GhostCodecSet for TgeGhosts {
    type Class = TgeClass;
    type Record = TgeRecord;

    fn classes() -> &'static [TgeClass] {
        &TgeClass::ALL
    }

    fn name(class: TgeClass) -> &'static str {
        class.name()
    }

    fn class_of(record: &TgeRecord) -> TgeClass {
        record.class()
    }

    /// The engine's own classes ([`ENGINE_OBJECT_CLASSES`]) at the ids past the TGE table, in order.
    fn class_past_table(id: u32, table_len: usize) -> Option<TgeClass> {
        let k = usize::try_from(id).ok()?.checked_sub(table_len)?;
        ENGINE_OBJECT_CLASSES
            .get(k)
            .and_then(|n| TgeGhosts::class_by_name(n))
    }

    /// Packs an object of `class`. An AudioEmitter's after-pack effects are [`audio::pack`]'s; a host that needs them calls it.
    fn pack(class: TgeClass, view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (TgeRecord, u32) {
        match class {
            TgeClass::AiPlayer => {
                let (r, m) = pack_player(view, mask, ctx);
                (TgeRecord::AiPlayer(r), m)
            }
            TgeClass::Player => {
                let (r, m) = pack_player(view, mask, ctx);
                (TgeRecord::Player(r), m)
            }
            TgeClass::AiWheeledVehicle => {
                let (r, m) = pack_wheeled_vehicle(view, mask, ctx);
                (TgeRecord::AiWheeledVehicle(r), m)
            }
            TgeClass::WheeledVehicle => {
                let (r, m) = pack_wheeled_vehicle(view, mask, ctx);
                (TgeRecord::WheeledVehicle(r), m)
            }
            TgeClass::FlyingVehicle => {
                let (r, m) = pack_flying_vehicle(view, mask, ctx);
                (TgeRecord::FlyingVehicle(r), m)
            }
            TgeClass::HoverVehicle => {
                let (r, m) = pack_hover_vehicle(view, mask, ctx);
                (TgeRecord::HoverVehicle(r), m)
            }
            TgeClass::AudioEmitter => (TgeRecord::AudioEmitter(audio::pack(view, mask, ctx).0), 0),
            TgeClass::Camera => (TgeRecord::Camera(pack_camera(view, mask, ctx)), 0),
            TgeClass::Item => {
                let (r, m) = pack_item(view, mask, ctx);
                (TgeRecord::Item(r), m)
            }
            TgeClass::Lightning => (TgeRecord::Lightning(pack_lightning(view, mask, ctx)), 0),
            TgeClass::MissionMarker => {
                let (r, m) = pack_mission_marker(view, mask, ctx);
                (TgeRecord::MissionMarker(r), m)
            }
            TgeClass::SpawnSphere => {
                let (r, m) = pack_spawn_sphere(view, mask, ctx);
                (TgeRecord::SpawnSphere(r), m)
            }
            TgeClass::WayPoint => {
                let (r, m) = pack_waypoint(view, mask, ctx);
                (TgeRecord::WayPoint(r), m)
            }
            TgeClass::ParticleEmitterNode => (
                TgeRecord::ParticleEmitterNode(pack_particle_emitter_node(view, mask, ctx)),
                0,
            ),
            TgeClass::PhysicalZone => (TgeRecord::PhysicalZone(pack_physical_zone(view, mask)), 0),
            TgeClass::Precipitation => (
                TgeRecord::Precipitation(pack_precipitation(view, mask, ctx)),
                0,
            ),
            TgeClass::Projectile => {
                let (r, m) = pack_projectile(view, mask, ctx);
                (TgeRecord::Projectile(r), m)
            }
            TgeClass::ScopeAlwaysShape => {
                let (r, m) = pack_static_shape(view, mask, ctx);
                (TgeRecord::ScopeAlwaysShape(r), m)
            }
            TgeClass::StaticShape => {
                let (r, m) = pack_static_shape(view, mask, ctx);
                (TgeRecord::StaticShape(r), m)
            }
            TgeClass::Sky => (TgeRecord::Sky(pack_sky(view, mask, ctx)), 0),
            TgeClass::Sun => (TgeRecord::Sun(pack_sun(view, mask)), 0),
            TgeClass::TsStatic => (TgeRecord::TsStatic(pack_ts_static(view)), 0),
            TgeClass::VehicleBlocker => (TgeRecord::VehicleBlocker(pack_vehicle_blocker(view)), 0),
            TgeClass::FxLight => (TgeRecord::FxLight(pack_fx_light(view, mask, ctx)), 0),
            TgeClass::SpotLight => (TgeRecord::SpotLight(pack_spot_light(view, mask, ctx)), 0),
            TgeClass::TimeOfDay => (TgeRecord::TimeOfDay(pack_time_of_day(view, mask)), 0),
            TgeClass::ScatterSky => (TgeRecord::ScatterSky(pack_scatter_sky(view, mask)), 0),
        }
    }

    fn decode(
        class: TgeClass,
        r: &mut BitReader,
        table: &Table,
        charge: &mut ChargeFn,
        ctx: &ReadCtx,
    ) -> Result<TgeRecord, NetError> {
        match class {
            TgeClass::AiPlayer => read(r, table, charge, ctx, TgeRecord::AiPlayer),
            TgeClass::AiWheeledVehicle => read(r, table, charge, ctx, TgeRecord::AiWheeledVehicle),
            TgeClass::AudioEmitter => read(r, table, charge, ctx, TgeRecord::AudioEmitter),
            TgeClass::Camera => read(r, table, charge, ctx, TgeRecord::Camera),
            TgeClass::FlyingVehicle => read(r, table, charge, ctx, TgeRecord::FlyingVehicle),
            TgeClass::HoverVehicle => read(r, table, charge, ctx, TgeRecord::HoverVehicle),
            TgeClass::Item => read(r, table, charge, ctx, TgeRecord::Item),
            TgeClass::Lightning => read(r, table, charge, ctx, TgeRecord::Lightning),
            TgeClass::MissionMarker => read(r, table, charge, ctx, TgeRecord::MissionMarker),
            TgeClass::ParticleEmitterNode => {
                read(r, table, charge, ctx, TgeRecord::ParticleEmitterNode)
            }
            TgeClass::PhysicalZone => read(r, table, charge, ctx, TgeRecord::PhysicalZone),
            TgeClass::Player => read(r, table, charge, ctx, TgeRecord::Player),
            TgeClass::Precipitation => read(r, table, charge, ctx, TgeRecord::Precipitation),
            TgeClass::Projectile => read(r, table, charge, ctx, TgeRecord::Projectile),
            TgeClass::ScopeAlwaysShape => read(r, table, charge, ctx, TgeRecord::ScopeAlwaysShape),
            TgeClass::Sky => read(r, table, charge, ctx, TgeRecord::Sky),
            TgeClass::SpawnSphere => read(r, table, charge, ctx, TgeRecord::SpawnSphere),
            TgeClass::StaticShape => read(r, table, charge, ctx, TgeRecord::StaticShape),
            TgeClass::Sun => read(r, table, charge, ctx, TgeRecord::Sun),
            TgeClass::TsStatic => read(r, table, charge, ctx, TgeRecord::TsStatic),
            TgeClass::VehicleBlocker => read(r, table, charge, ctx, TgeRecord::VehicleBlocker),
            TgeClass::WayPoint => read(r, table, charge, ctx, TgeRecord::WayPoint),
            TgeClass::WheeledVehicle => read(r, table, charge, ctx, TgeRecord::WheeledVehicle),
            TgeClass::FxLight => read(r, table, charge, ctx, TgeRecord::FxLight),
            TgeClass::SpotLight => read(r, table, charge, ctx, TgeRecord::SpotLight),
            TgeClass::TimeOfDay => read(r, table, charge, ctx, TgeRecord::TimeOfDay),
            TgeClass::ScatterSky => read(r, table, charge, ctx, TgeRecord::ScatterSky),
        }
    }

    fn encode(record: &TgeRecord, w: &mut BitWriter, table: &Table, widths: NetWidths) -> R {
        record.encode(w, table, widths)
    }
}
