//! What the engine keeps for an object beside the VM's own state: the one `SimObject::ext` slot.
//!
//! `ext` holds a single `Box<dyn Any>` per object, so every engine class that keeps state in Rust shares one type, here
//! [`ObjectExt`], and each owner has a field in it: the type mask `getType()` answers and the scene object's boxes; the
//! simulation adds the Player's and the vehicles' state. One downcast reaches all of it, and the object's death frees it
//! without any table to keep in step. The host's own state for an object is one value in the
//! [`ObjectExt::game`] slot, of the host's type.
//!
//! The scripts' own fields are not here: they are the VM's static fields, and stay the one place a script reads and
//! writes them (the design note in [`super`]).

use std::any::Any;

use ts_vm::sim::{ObjId, Objects};

/// The boxes and the sphere of a scene object: its object box, which the class decides (a shape class's
/// is its datablock's, a plain scene object's is the empty box at the origin), and the world box, the object box scaled
/// and moved by the transform, which `setTransform`, `setScale` and `onAdd` recompute and a script's write to the
/// `position` field does not.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SceneState {
    pub object_min: [f32; 3],
    pub object_max: [f32; 3],
    pub world_min: [f32; 3],
    pub world_max: [f32; 3],
    /// The world sphere `resetWorldBox` makes from the world box: a spatial index may place an object by it
    /// and a ray's first test is against it.
    pub world_center: [f32; 3],
    pub world_radius: f32,
}

/// A `Camera`'s own state (with the ShapeBase words it reads): the mode and what each mode
/// keeps, the rotation, the field of view and the control references. The position is the transform matrix in the `position`
/// field.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraExt {
    /// The camera mode (TGE's five are 0 to 4; a camera class may add more); 2 (fly) from the constructor.
    pub mode: u32,
    /// The pitch and yaw of the last `setTransform` or `setPosition`.
    pub pitch: f32,
    pub yaw: f32,
    /// `Camera::setControlObject`'s object.
    pub controlled: Option<ObjId>,
    /// The orbit object and its distances: min, max and current, the point of the point mode
    /// and `setOrbitMode`'s `ownClientObject`.
    pub orbit: Option<ObjId>,
    pub orbit_distances: [f32; 3],
    pub orbit_point: [f32; 3],
    pub orbit_flag: bool,
    /// The trigger states `ShapeBase::processTick` compared the moves with: a change calls `onTrigger`.
    pub triggers: [bool; 5],
    /// The camera that has this object as its control object, when it is a camera that holds it.
    pub controlling_object: Option<ObjId>,
    /// ShapeBase's field of view (90.0 from the constructor), the energy word `writePacketData` sends (0.0), the damage
    /// flash and white-out levels.
    pub fov: f32,
    pub energy: f32,
    pub flash: f32,
    pub white_out: f32,
    /// ShapeBase's datablock has been set once (`onNewDataBlock` initialises the energy words only the first time).
    pub had_datablock: bool,
}

impl Default for CameraExt {
    fn default() -> CameraExt {
        CameraExt {
            mode: 2,
            pitch: 0.0,
            yaw: 0.0,
            controlled: None,
            orbit: None,
            orbit_distances: [0.0; 3],
            orbit_point: [0.0; 3],
            orbit_flag: false,
            triggers: [false; 5],
            controlling_object: None,
            fov: 90.0,
            energy: 0.0,
            flash: 0.0,
            white_out: 0.0,
            had_datablock: false,
        }
    }
}

/// The per-object state the engine's engine classes keep (see the module documentation): the Torque classes' fields, and one
/// slot for the host's own ([`ObjectExt::game`]).
#[derive(Debug, Default)]
pub struct ObjectExt {
    /// What `getType()` answers (`SimObject`'s type mask): 0 for everything that does not set it.
    pub type_mask: u32,
    /// The scene object's boxes, set by `SceneObject`'s `on_add`.
    pub scene: Option<SceneState>,
    /// The collision-disable count (`SceneObject`): `disableCollision` adds one, `enableCollision` takes one off a count
    /// that is not zero. While it is not zero the container's queries skip the object (`containerRayCast`'s exemptions, a
    /// mover's own working-set query).
    pub collision_off: u32,
    /// `ShapeBase::mHidden`: `setHidden(true)` took the object out of the container ([`super::shapebase::set_hidden`]).
    pub hidden: bool,
    /// The ghost hub the object was registered with: the hooks that mark the object dirty reach it from here.
    pub ghost: Option<std::rc::Rc<dyn crate::ghost::GhostLink>>,
    /// The Sim time the object was added (the ghost layer's time-keeping packers count from it).
    pub created_ms: u32,
    /// A `PhysicalZone` that `deactivate` switched off (a new zone is active).
    pub zone_inactive: bool,
    /// The object an `fxLight` follows (`attachToObject`).
    pub attached_object: Option<ObjId>,
    /// A `Camera`'s state, made by the class's `on_add`.
    pub camera: Option<CameraExt>,
    /// The server's process list, set by `GameBase`'s `on_add`: the hooks that change an object's place in it reach it from
    /// here.
    pub process: Option<crate::sim::ProcessHandle>,
    /// A `ShapeBase`'s shape instance and script threads, made by `onNewDataBlock`.
    pub shape: Option<Box<super::shapebase::ShapeBaseState>>,
    /// A `ShapeBaseData`'s shape resource, the one its preload loaded: what an object's `onNewDataBlock` builds its instance
    /// from.
    pub shape_resource: Option<crate::shapes::ShapeResource>,
    /// The shape a shape-carrying datablock's preload loaded, or the one an object's datablock gave it at `onNewDataBlock` (the
    /// datablock's resource member, the instance's lock), held until it goes: `purgeResources` frees only what no one holds.
    pub shape_hold: Option<std::rc::Rc<crate::shapes::KeptShape>>,
    /// A Player's or AIPlayer's simulation state, made by its first PlayerData.
    pub player: Option<Box<crate::sim::player::PlayerExt>>,
    /// An Item's simulation state, made by its first ItemData.
    pub item: Option<Box<crate::sim::item::ItemExt>>,
    /// A Projectile's simulation state, made by its `onAdd`.
    pub projectile: Option<Box<crate::sim::projectile::ProjectileExt>>,
    /// A WheeledVehicle's or FlyingVehicle's simulation state, made by its `onAdd`.
    pub vehicle: Option<Box<crate::sim::vehicle::VehicleExt>>,
    /// A ShapeBase's mount links.
    pub mount: crate::sim::mount::MountState,
    /// A ShapeBase's damage and energy words when it is not a Player (a Player's are its state), and the datablock they were
    /// set for.
    pub vitals: Option<crate::sim::vitals::Vitals>,
    pub vitals_db: Option<ObjId>,
    /// The object that controls this one when it is not a Camera (a Camera keeps its own in [`CameraExt`]).
    pub controlled_by: Option<ObjId>,
    /// `ShapeBase::mCameraFov` of an object that is not a Camera, once `setCameraFov` set it (none: the constructor's 90).
    pub camera_fov: Option<f32>,
    /// The cameras whose orbit took a delete notify on this object (`deleteNotify`): its removal
    /// runs their `onDeleteNotify` ([`super::camera::orbit_removed`]). Only cameras that still orbit it are kept.
    pub orbit_notify: Vec<ObjId>,
    /// A datablock's count of static-field writes (the `onStaticModified` hook point): its Players rebuild their products when
    /// it moves.
    pub db_generation: u32,
    /// A PlayerData's products for its Players, with the write count they were built at (one rebuild per write, shared by every
    /// Player).
    pub player_info: Option<(u32, std::rc::Rc<crate::sim::player::PlayerInfo>)>,
    /// A ShapeBase's four mounted-image slots, made by the first image call that writes them.
    pub images: Option<Box<::sim::image::ImageSet>>,
    /// A ShapeBaseImageData's view for the state machine, with the (write count, preload version) it was built at.
    pub image_data: Option<((u32, u32), std::sync::Arc<::sim::image::ImageData>)>,
    /// A map object's collision (a terrain, an interior, a water block): its container view and shape, made by its `onAdd`.
    pub map: Option<MapObject>,
    /// The host's state for the object, one value of the host's own type, made by the
    /// first write of it.
    pub game: Option<Box<dyn Any>>,
}

impl ObjectExt {
    /// The host's state, if it is a `T`.
    pub fn game<T: 'static>(&self) -> Option<&T> {
        self.game.as_deref()?.downcast_ref::<T>()
    }

    /// The host's state, mutably, if the object has one of type `T` (none is made).
    pub fn game_existing_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.game.as_deref_mut()?.downcast_mut::<T>()
    }

    /// The host's state, made as `T::default()` first if the object has none. None when the slot holds another type.
    pub fn game_mut<T: Default + 'static>(&mut self) -> Option<&mut T> {
        self.game
            .get_or_insert_with(|| Box::new(T::default()))
            .downcast_mut::<T>()
    }
}

/// The object's [`ObjectExt`], if it has one.
pub fn get(objects: &Objects, id: ObjId) -> Option<&ObjectExt> {
    objects.get(id)?.ext.as_ref()?.downcast_ref::<ObjectExt>()
}

/// The object's [`ObjectExt`], made empty first if it has none. None for an object that is gone, or whose slot holds
/// something that is not an [`ObjectExt`] (nothing else puts anything there).
pub fn get_mut(objects: &mut Objects, id: ObjId) -> Option<&mut ObjectExt> {
    let object = objects.get_mut(id)?;
    object
        .ext
        .get_or_insert_with(|| Box::new(ObjectExt::default()))
        .downcast_mut::<ObjectExt>()
}

/// A map object's collision (a terrain, an interior, a water block): its container view and its shape (none for a water block
/// or a file that does not read). The host that builds map objects fills [`ObjectExt::map`]; the scene's rays and convexes
/// answer from it.
#[derive(Debug, Clone, PartialEq)]
pub struct MapObject {
    pub view: collide::convex::ObjectView,
    pub shape: Option<std::rc::Rc<collide::map::MapShape>>,
}

/// The object's map collision, if it has one.
pub fn map_of(vm: &ts_vm::vm::Vm, id: ts_vm::sim::ObjId) -> Option<&MapObject> {
    get(vm.objects(), id)?.map.as_ref()
}
