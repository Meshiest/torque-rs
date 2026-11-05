//! What a frame reads of a client's world of ghosts to draw them: [`ShapeWorld`], over `torque-ghost-world`'s record world
//! ([`ClientWorld`]) and a game's records.
//!
//! The record world already holds most of it: each ghost's class, datablock, scale, mount, the place its record puts it, the
//! mounted images and their machine, the script threads, the control object and the eye, and the client's own render poses
//! (the predicted control object, the vehicles it runs, the projectiles it steps). The trait adds what only a game's records
//! and datablocks can answer (how a class is drawn, a datablock's fields, a WheeledVehicle's wheels) and the points where a game
//! draws other than TGE; each of those has TGE's rule as its default. TGE's own set implements it ([`TgeClientSet`]'s world).
//!
//! Credit: the defaults follow TGE 1.4's `ShapeBase::getRenderMountTransform`, `ShapeBase::getRenderImageTransform`,
//! `ShapeBase::updateImageAnimation` and `ShapeBaseImageData::preload` (`game/shapeImage.cc`), `Player::renderMountedImage`
//! (`game/player.cc`), `WheeledVehicle::renderImage` and the steering thread of `WheeledVehicle::advanceTime`
//! (`game/vehicles/wheeledVehicle.cc`), and a Player's rest pose
//! `Player::onNewDataBlock`, `updateLookAnimation`, `setActionThread` and `unpackUpdate`'s head (`game/player.cc`), each
//! Copyright (C) GarageGames.com, Inc.; as Torque3D carries them under the MIT licence: `T3D/shapeImage.cpp` and
//! `T3D/vehicles/wheeledVehicle.cpp`, Copyright (c) 2012 GarageGames, LLC; `T3D/player.cpp`, Copyright (c) 2012 GarageGames,
//! LLC and Copyright (C) 2015 Faust Logic, Inc. The code here is this crate's own.

use std::collections::BTreeMap;

use ghost_world::set::{ClientRecord, ClientSet};
use ghost_world::tge::TgeClientSet;
use ghost_world::world::{ClientWorld, Ghost, Pose};
use ghosts::{TgeClass, TgeRecord};
use shapes::instance::ShapeInstance;
use shapes::shape::Shape;

pub use ghosts::Value;

/// How a ghost class is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    /// A shape in its default pose with its script threads (a StaticShape).
    Static,
    /// An Item.
    Item,
    /// A Player (or an AIPlayer): posed by its look and its action.
    Player,
    /// A WheeledVehicle: its tires drawn after its body.
    WheeledVehicle,
    /// Another vehicle.
    Vehicle,
    /// A Projectile: its datablock's `projectileShapeName` where the client steps it.
    Projectile,
    /// Not drawn as a shape.
    None,
}

impl ShapeKind {
    /// A Player or an AIPlayer.
    pub fn is_player(self) -> bool {
        self == ShapeKind::Player
    }
}

/// A wheel of a WheeledVehicle as its record sets it up: its tire's and its spring's datablocks and its steering factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelSetup {
    pub tire: u32,
    pub spring: u32,
    pub steering: f32,
}

/// A WheeledVehicle's wheels as its record sent them: the steering (radians, as the client unpacks it) and each wheel's setup in
/// hub order (`None`: no tire or no spring).
#[derive(Debug, Clone, PartialEq)]
pub struct Wheels {
    pub steering: f32,
    pub wheels: Vec<Option<WheelSetup>>,
}

/// The ghost class of a world.
pub type ClassOf<W> = <<W as ShapeWorld>::Set as ClientSet>::Class;

/// What a frame reads of a client's world to draw its ghosts. The methods with a body are where a game may draw other than TGE.
pub trait ShapeWorld: Sync {
    /// A game's classes and records.
    type Set: ClientSet;

    /// The record world.
    fn records(&self) -> &ClientWorld<Self::Set>;

    /// How ghosts of `class` are drawn.
    fn kind(&self, class: ClassOf<Self>) -> ShapeKind;

    /// The classes drawn as shapes, in the order a frame draws them.
    fn shape_classes(&self) -> Vec<ClassOf<Self>>;

    /// Field `name`'s element `index` of datablock `id`, as its event carried it.
    fn field(&self, id: u32, name: &str, index: u32) -> Option<&Value>;

    /// Whether datablock `id` is a `ShapeBaseImageData`.
    fn is_image(&self, id: u32) -> bool;

    /// A WheeledVehicle ghost's wheels, when its record set them up.
    fn wheels(&self, ghost: &Ghost<Self::Set>) -> Option<Wheels>;

    /// How ghost `ghost` is drawn (`None` for a ghost of no class).
    fn kind_of(&self, ghost: &Ghost<Self::Set>) -> ShapeKind {
        ghost.class.map_or(ShapeKind::None, |c| self.kind(c))
    }

    /// A float field (`None` when it is not a float).
    fn float(&self, id: u32, name: &str, index: u32) -> Option<f32> {
        match self.field(id, name, index)? {
            Value::F32(x) => Some(*x),
            _ => None,
        }
    }

    /// A text field's first element.
    fn text(&self, id: u32, name: &str) -> Option<&[u8]> {
        match self.field(id, name, 0)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A ghost's per-object colours, when its records carry them (TGE's do not).
    fn mesh_colors<'a>(&'a self, ghost: &'a Ghost<Self::Set>) -> Option<&'a [Option<[u8; 4]>]> {
        let _ = ghost;
        None
    }

    /// The texture each IFL material slot of a ghost's shape shows (`ghost`: none for a mounted image's), from `ifls` (per IFL
    /// material: its name, its slot and its frames): by default each list's first frame (TGE steps them with the shape's IFL
    /// sequences; this crate draws the first).
    fn ifl_frames(
        &self,
        ghost: Option<&Ghost<Self::Set>>,
        ifls: &[(String, usize, Vec<String>)],
    ) -> BTreeMap<usize, String> {
        let _ = ghost;
        ifls.iter()
            .filter_map(|(_, slot, frames)| Some((*slot, frames.first()?.clone())))
            .collect()
    }

    /// The object names a ghost's records hide, when they carry them (TGE's do not).
    fn hidden_nodes<'a>(&'a self, ghost: &'a Ghost<Self::Set>) -> Option<&'a [u8]> {
        let _ = ghost;
        None
    }

    /// The scale `getRenderMountTransform` scales a mount node of ghost `ghost` (at `scale`) by: TGE's is the object's own.
    fn mount_scale(&self, ghost: &Ghost<Self::Set>, scale: [f32; 3]) -> [f32; 3] {
        let _ = ghost;
        scale
    }

    /// Whether an image drawn at its `eyeOffset` is scaled by its owner's scale (TGE's is not).
    fn scaled_eye_offset(&self) -> bool {
        false
    }

    /// Whether the render transform of a mount point the shape has no node for is scaled by the owner's scale (TGE's is not:
    /// `getRenderMountTransform` gives the render transform).
    fn scaled_bare_mount(&self) -> bool {
        false
    }

    /// Whether a WheeledVehicle's tyres, hubs and spring rays take the vehicle's scale (TGE's do not).
    fn scaled_tires(&self) -> bool {
        false
    }

    /// The node a state's `stateEmitterNode` `node` names in image shape `shape`, whose `muzzlePoint` is node `muzzle` (-1:
    /// none): TGE's maps -1 to the muzzle (`ShapeBaseImageData::preload`) and takes any other as it is.
    fn image_emitter_node(&self, shape: &Shape, node: i32, muzzle: i32) -> i32 {
        let _ = shape;
        if node == -1 { muzzle } else { node }
    }

    /// Whether the state emitters of image datablock `image` emit, in its owner's first-person view or not: TGE's always do
    /// (`ShapeBase::updateImageAnimation`).
    fn image_emits(&self, image: u32, first_person: bool) -> bool {
        let _ = (image, first_person);
        true
    }

    /// The detail a Player's image of `details` details is drawn at, in its owner's first-person view or not: TGE's picks it as any
    /// shape's (`None`).
    fn player_image_detail(&self, details: usize, first_person: bool) -> Option<usize> {
        let _ = (details, first_person);
        None
    }

    /// The colour every object of image datablock `image` is drawn with when a Player holds it (TGE's: none).
    fn player_image_color(&self, image: u32) -> Option<[f32; 4]> {
        let _ = image;
        None
    }

    /// The height of the ground a vehicle's wheels reach for when the client does not run the vehicle itself (TGE's: none, the
    /// wheels hang at their springs' full length).
    fn ground_height(&self) -> Option<f32> {
        None
    }

    /// Poses ghost `index`'s `instance` of `shape` as a frame draws it when the client keeps no live pose of it, the nodes
    /// animated at detail `dl` (`pose`: where the frame draws it). By default a Player (posed by its head pitch) by TGE's root
    /// and look threads ([`crate::player::rest_threads`]) and its script threads (`playThread`) at the world's clock; another by
    /// its script threads, animated when it plays any. A game poses its Player by its own rules here.
    fn pose_at_rest(
        &self,
        index: u32,
        pose: Option<Pose>,
        shape: &Shape,
        instance: &mut ShapeInstance,
        dl: usize,
        notes: &mut Vec<String>,
    ) {
        let _ = notes;
        let look = match pose {
            Some(Pose::Euler { pitch, .. }) => Some((pitch, 0.0)),
            _ => None,
        };
        rest_pose(self, index, look, shape, instance, dl);
    }
}

/// [`ShapeWorld::pose_at_rest`]'s default: a Player with a head `look` (its pitch, and its side angle as a fraction of
/// `maxLookAngle`) by TGE's root and look threads ([`crate::player::rest_threads`]) then its script threads; another (or a
/// Player with no head) by its script threads, animated when it plays any.
pub fn rest_pose<W: ShapeWorld + ?Sized>(
    world: &W,
    index: u32,
    look: Option<(f32, f32)>,
    shape: &Shape,
    instance: &mut ShapeInstance,
    dl: usize,
) {
    let script = crate::player::script_poses(world.records(), index);
    let player = world
        .records()
        .ghosts
        .get(&index)
        .is_some_and(|g| world.kind_of(g) == ShapeKind::Player);
    if let (true, Some((pitch, side))) = (player, look) {
        crate::player::rest_threads(shape, instance, pitch, side);
        crate::player::add_script_threads(shape, instance, &script);
        let mut charge = |_: u64| true;
        let _ = instance.animate(shape, dl, None, &mut charge);
    } else if crate::player::add_script_threads(shape, instance, &script) {
        let mut charge = |_: u64| true;
        let _ = instance.animate(shape, dl, None, &mut charge);
    }
}

/// `PlayerData::maxLookAngle`'s default.
const MAX_LOOK_ANGLE: f32 = 1.4;

/// TGE's own world.
impl ShapeWorld for ClientWorld<TgeClientSet> {
    type Set = TgeClientSet;

    fn records(&self) -> &ClientWorld<TgeClientSet> {
        self
    }

    fn kind(&self, class: TgeClass) -> ShapeKind {
        match class {
            TgeClass::StaticShape | TgeClass::ScopeAlwaysShape => ShapeKind::Static,
            TgeClass::Item => ShapeKind::Item,
            TgeClass::Player | TgeClass::AiPlayer => ShapeKind::Player,
            TgeClass::WheeledVehicle | TgeClass::AiWheeledVehicle => ShapeKind::WheeledVehicle,
            TgeClass::FlyingVehicle | TgeClass::HoverVehicle => ShapeKind::Vehicle,
            TgeClass::Projectile => ShapeKind::Projectile,
            _ => ShapeKind::None,
        }
    }

    fn shape_classes(&self) -> Vec<TgeClass> {
        vec![
            TgeClass::StaticShape,
            TgeClass::ScopeAlwaysShape,
            TgeClass::WheeledVehicle,
            TgeClass::AiWheeledVehicle,
            TgeClass::FlyingVehicle,
            TgeClass::HoverVehicle,
            TgeClass::Item,
            TgeClass::Player,
            TgeClass::AiPlayer,
            TgeClass::Projectile,
        ]
    }

    fn field(&self, id: u32, name: &str, index: u32) -> Option<&Value> {
        self.datablocks.get(&id)?.get(name, index)
    }

    fn is_image(&self, id: u32) -> bool {
        self.datablocks
            .get(&id)
            .is_some_and(|d| d.class == "ShapeBaseImageData")
    }

    /// `Vehicle::unpackUpdate`'s steering (`2 * readFloat(9) * maxSteeringAngle - maxSteeringAngle`; 0 when the update carried
    /// none) and the wheels `WheeledVehicle::unpackUpdate` set up.
    fn wheels(&self, ghost: &Ghost<TgeClientSet>) -> Option<Wheels> {
        let (TgeRecord::WheeledVehicle(r) | TgeRecord::AiWheeledVehicle(r)) =
            ghost.record.as_ref()?
        else {
            return None;
        };
        let setup = r.setup.as_ref()?;
        let max = ghost
            .record
            .as_ref()
            .and_then(ClientRecord::datablock)
            .and_then(|d| self.float(d, "maxSteeringAngle", 0))
            .unwrap_or(0.0);
        let steering = r.vehicle.state.as_ref().map_or(0.0, |s| {
            let yaw = s.steering[0] as f32 / 511.0;
            2.0 * yaw * max - max
        });
        Some(Wheels {
            steering,
            wheels: setup
                .iter()
                .map(|w| {
                    w.as_ref().map(|w| WheelSetup {
                        tire: w.tire,
                        spring: w.spring,
                        steering: w.steering.get(),
                    })
                })
                .collect(),
        })
    }

    /// A Player its record places looks as `Player::unpackUpdate` sets its head: the record's fractions times its datablock's
    /// `maxLookAngle` (the side angle kept as the fraction [`crate::player::rest_threads`] takes); a pose the client renders
    /// (its own prediction) keeps that pose's pitch.
    fn pose_at_rest(
        &self,
        index: u32,
        pose: Option<Pose>,
        shape: &Shape,
        instance: &mut ShapeInstance,
        dl: usize,
        notes: &mut Vec<String>,
    ) {
        let _ = notes;
        let g = self.ghosts.get(&index);
        let head = g
            .filter(|g| pose.is_some() && g.pose() == pose)
            .and_then(|g| g.record.as_ref())
            .and_then(ghost_world::tge::head_fractions);
        let look = match (pose, head) {
            (Some(Pose::Euler { .. }), Some([x, z])) => {
                let max = g
                    .and_then(|g| g.datablock())
                    .and_then(|d| self.float(d, "maxLookAngle", 0))
                    .filter(|m| m.is_finite())
                    .unwrap_or(MAX_LOOK_ANGLE);
                Some((x * max, z))
            }
            (Some(Pose::Euler { pitch, .. }), None) => Some((pitch, 0.0)),
            _ => None,
        };
        let steered = steering_thread(self, index, shape, instance);
        rest_pose(self, index, look, shape, instance, dl);
        if steered {
            let mut charge = |_: u64| true;
            let _ = instance.animate(shape, dl, None, &mut charge);
        }
    }
}

/// Where a WheeledVehicle's `steering` thread sits (`WheeledVehicleData::preload` finds the sequence; `advanceTime` sets
/// its place): `0.5 - t / 2`, `t` the steering times its own size over `maxSteeringAngle`, so 0 is full right and 0.5
/// straight ahead. The steering is the client's own for a vehicle it runs, else the record's. None for a ghost that is not a
/// WheeledVehicle or has no steering lock.
pub fn steering_place(w: &ClientWorld<TgeClientSet>, index: u32) -> Option<f32> {
    let g = w.ghosts.get(&index)?;
    let wheels = w.wheels(g)?;
    let max = g
        .record
        .as_ref()
        .and_then(ClientRecord::datablock)
        .and_then(|d| w.float(d, "maxSteeringAngle", 0))
        .filter(|m| m.is_finite() && *m > 0.0)?;
    let steering = w
        .vehicle_look
        .get(&index)
        .map(|l| l.steering)
        .filter(|s| s.is_finite())
        .unwrap_or(wheels.steering);
    let t = steering * steering.abs() / max;
    Some((0.5 - t * 0.5).clamp(0.0, 1.0))
}

/// The `steering` thread on `instance` at [`steering_place`] when the shape has the sequence (not yet animated).
fn steering_thread(
    w: &ClientWorld<TgeClientSet>,
    index: u32,
    shape: &Shape,
    instance: &mut ShapeInstance,
) -> bool {
    let Some(seq) =
        shapes::instance::find_sequence(shape, b"steering").and_then(|s| i32::try_from(s).ok())
    else {
        return false;
    };
    let Some(place) = steering_place(w, index) else {
        return false;
    };
    let Some(thread) = instance.create_thread(shape) else {
        return false;
    };
    instance.set_sequence(shape, thread, seq, place);
    true
}

#[cfg(test)]
#[path = "world_tests.rs"]
pub(crate) mod tests;
