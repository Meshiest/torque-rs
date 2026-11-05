//! The weapon images on the host: `::sim::image`'s state machine over a `ShapeBase`'s four slots, its datablock view, its
//! script callbacks, the muzzle chain over the object's skeleton and the container, and the 18 `ShapeBase` image methods.
//!
//! **Where the state lives.** The slots are an [`ImageSet`] in the object's ext (`ObjectExt::images`, made by the first call
//! that writes it), reached through [`Images::images`] at every access, so a script that a callback runs may call any image
//! method on the same object, even on the slot being changed (the re-entrancy contract of `::sim::image`). A Player's
//! `PlayerState::images` is a mirror of the occupancy (`sim`'s step reads it for the slot-0 wet byte and the AIPlayer's move
//! triggers), written after every call ([`sync_mirror`]).
//!
//! **The datablock view** ([`view_of`]): the 31 state records as `ShapeBaseImageData::onAdd` built them (`dbvalues::image`:
//! the transitions resolved then, `stateAllowImageChange` and the `stateScript` pointer copied then), the preload's products
//! (`useEyeOffset`, the `muzzlePoint` node and its default-pose matrix, the mount transform), and the
//! members the machine reads live (`usesEnergy`, `minEnergy`, `lightType`, `correctMuzzleVector`, `mountPoint`, `eyeOffset`).
//! It is cached on the datablock's ext by (write count, preload version) and rebuilt after a static-field write or a preload.
//!
//! **Callbacks** are `Con::executef(db, 3, name, "%d" obj, "%d" slot)`, synchronous: the state's `stateScript`,
//! `onMount` (after the first state's entry) and `onUnmount` (before the teardown). Entering the fire state animates the
//! object's shape instance (`animate` at detail 0), which the stale-pose readers (`getSlotTransform`, the muzzle's mount
//! transform) see.
//!
//! **Who steps the slots.** A Player's tick (`sim::player`'s `ShapeBase::processTick` part calls `image_tick` per slot, and the
//! move's triggers 0 and 1 reach `setImageTrigger` in `updateMove`): `tick.rs` runs [`tick_slot`] and [`set_trigger`] with the
//! Player's state parked. **Listed:** the slots of an Item or a vehicle are not stepped (their `processTick`s are not
//! `ShapeBase`'s here); a tagged skin (`mountImage`'s fourth argument) is kept and answered by `getImageSkinTag` but goes to
//! the clients as no skin; the light registry (a `lightType` image) has no server state and is not kept.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/shapeImage.cpp`
//! (Copyright (c) 2012 GarageGames, LLC); `T3D/shapeBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust
//! Logic, Inc.). The code here is this crate's own.

use std::rc::Rc;
use std::sync::Arc;

use ::sim::RayHit;
use ::sim::image::console::{self, Value as ImgValue};
use ::sim::image::muzzle::MuzzleCtx;
use ::sim::image::{
    AimClient, DbId, IMAGE_DT, ImageData, ImageHost, ImageSet, ImageState, RayCaster, SLOTS,
    machine,
};
use ::sim::player::{PlayerData, PlayerState};
use collide::ObjectId;
use collide::math::{Box3, F3, IDENTITY};
use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::text::format_g;
use ts_vm::vm::{Native, NativeCall, Vm, VmError};

use super::hosted::{container_ray, switch_collision};
use super::player::skeleton::HostSkeleton;
use super::player::tick::SimEnv;
use super::player::{PlayerInfo, generation};
use crate::dbvalues::DbValues;
use crate::methods::register_method;
use crate::scene::access::{matrix_of, scale_of, value_named, value_of};
use crate::scene::ext;
use crate::scene::ghost as ghosts;

/// What one build of a datablock view is charged (31 records and the scalars).
const VIEW_WORK: u64 = 64;

/// Engine-only: the call-depth units an image callback costs on top of the script level and the re-entry. The Rust frames
/// of one level of `stateScript` -> `mountImage` on the next object -> `setImage` -> `setImageState` -> the callback are about
/// 7.6 KiB in a debug build, against 3.1 for a unit of a plain script level, so at the usual two units a level 127 levels used
/// 1,014 KiB of a 1 MiB thread (10 KiB left); at three, recursion through the images stops with `CallDepth` at 85 levels.
/// `SIM_IMAGE_DEPTH` is per object and does not bound a chain over many objects.
pub const IMAGE_CALL_DEPTH: usize = 1;

// ------------------------------------------------------------------------------------------------------------------- the slots

/// The object's slots, if it ever had any written.
pub fn set_of(vm: &Vm, id: ObjId) -> Option<&ImageSet> {
    ext::get(vm.objects(), id)?.images.as_deref()
}

/// The datablock in slot `slot`.
pub fn slot_db(vm: &Vm, id: ObjId, slot: usize) -> Option<DbId> {
    set_of(vm, id)?.slots.get(slot)?.db
}

/// The trigger byte of slot `slot`, false for an empty slot.
pub fn trigger(vm: &Vm, id: ObjId, slot: usize) -> bool {
    set_of(vm, id)
        .and_then(|s| s.slots.get(slot))
        .is_some_and(|m| m.db.is_some() && m.trigger_down)
}

/// Which slots hold an image.
pub fn occupied(vm: &Vm, id: ObjId) -> [bool; SLOTS] {
    set_of(vm, id).map_or([false; SLOTS], ImageSet::occupied)
}

/// The Player's mirror of the occupancy (`PlayerState::images`) written from the slots.
pub fn sync_mirror(vm: &mut Vm, id: ObjId) {
    let o = occupied(vm, id);
    if let Some(s) = super::player::state_mut(vm, id) {
        s.images = o;
    }
}

/// The slot-0 wet byte from the Player step's copy (`ShapeBase::processTick` flips it before the image loop).
pub fn load_wet0(vm: &mut Vm, id: ObjId, wet: bool) {
    if let Some(set) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.images.as_deref_mut()) {
        set.load_wet0(wet);
    }
}

/// The view of datablock `db` the ghost record reads (the cached one; `None` before the machine first asked for it).
pub fn cached_view(vm: &Vm, db: DbId) -> Option<&ImageData> {
    ext::get(vm.objects(), db)?
        .image_data
        .as_ref()
        .map(|(_, d)| &**d)
}

// --------------------------------------------------------------------------------------------------------------- the datablock

fn until_nul(t: &[u8]) -> &[u8] {
    t.split(|&b| b == 0).next().unwrap_or(&[])
}

fn text(t: &[u8]) -> String {
    String::from_utf8_lossy(until_nul(t)).into_owned()
}

/// The view built now (see the module docs).
fn build_view(vm: &Vm, values: &DbValues, db: ObjId) -> Option<ImageData> {
    let flag = |name: &[u8]| matches!(value_named(vm, db, name), Some(Value::Bool(true)));
    let int = |name: &[u8]| match value_named(vm, db, name) {
        Some(Value::Int(n)) => n,
        _ => 0,
    };
    let eye_offset = match value_of(vm, db, b"eyeOffset") {
        Some(Value::Matrix(m)) => crate::dbvalues::image::flat(&m),
        _ => IDENTITY,
    };
    let min_energy = match value_of(vm, db, b"minEnergy") {
        Some(Value::Float(x)) => x,
        _ => 0.0,
    };
    let mut d = values.with_image(db, |im| {
        let mut d = ImageData::new(db);
        d.states = im
            .states
            .iter()
            .map(|s| {
                let t = &s.trans;
                ImageState {
                    name: s.name.as_deref().map(text).unwrap_or_default(),
                    loaded: [t[0], t[1]],
                    ammo: [t[2], t[3]],
                    target: [t[4], t[5]],
                    wet: [t[6], t[7]],
                    trigger: [t[8], t[9]],
                    timeout: t[10],
                    fire: s.fire,
                    allow_image_change: s.allow_image_change,
                    wait_for_timeout: s.wait_for_timeout,
                    timeout_value: s.timeout,
                    energy_drain: s.energy_drain,
                    loaded_flag: s.loaded_flag,
                    recoil: s.recoil,
                    script: s.script.as_deref().map(text),
                }
            })
            .collect();
        d.first_person = im.use_eye_offset;
        d.mount_transform = im.mount_transform;
        d.muzzle_node = im.muzzle_node;
        d.muzzle_node_transform = im.muzzle_transform;
        d
    })?;
    d.uses_energy = flag(b"usesEnergy");
    d.min_energy = min_energy;
    d.light_type = int(b"lightType") as u32;
    d.correct_muzzle_vector = flag(b"correctMuzzleVector");
    d.mount_point = int(b"mountPoint") as u32;
    d.eye_offset = eye_offset;
    d.finish();
    Some(d)
}

/// The state machine's view of `ShapeBaseImageData` `db`: cached, or built (charged) and cached. `None` for an object that is
/// not one, or when the budget refused the build.
pub fn view_of(vm: &mut Vm, values: &DbValues, db: ObjId) -> Option<Arc<ImageData>> {
    if !vm.objects().contains(db) {
        return None;
    }
    let version = values.with_image(db, |i| i.version)?;
    let key = (generation(vm, db), version);
    if let Some((k, d)) = ext::get(vm.objects(), db).and_then(|e| e.image_data.as_ref())
        && *k == key
    {
        return Some(Arc::clone(d));
    }
    if !vm.charge_work(VIEW_WORK) {
        return None;
    }
    let d = Arc::new(build_view(vm, values, db)?);
    if let Some(e) = ext::get_mut(vm.objects_mut(), db) {
        e.image_data = Some((key, Arc::clone(&d)));
    }
    Some(d)
}

/// The object named or numbered by `text` if it is a `ShapeBaseImageData` (`Sim::findObject` and the `dynamic_cast` of
/// `mountImage`).
fn find_image(vm: &Vm, text: &str) -> Option<DbId> {
    let objects = vm.objects();
    let id = objects.find(text.as_bytes())?;
    let wanted = objects.classes.find(b"ShapeBaseImageData")?;
    objects
        .classes
        .is_a(objects.get(id)?.class(), wanted)
        .then_some(id)
}

// ------------------------------------------------------------------------------------------------------------------- the host

/// `ImageHost` over the `Vm` for one object (a native's call, or a Player's tick with its state parked).
pub struct Images<'a> {
    vm: &'a mut Vm,
    env: &'a SimEnv,
    id: ObjId,
    /// The slots of an object with no ext (gone): nothing is kept.
    spare: ImageSet,
    /// The first error a callback's script returned.
    pub error: Option<VmError>,
}

impl<'a> Images<'a> {
    pub fn new(vm: &'a mut Vm, env: &'a SimEnv, id: ObjId) -> Images<'a> {
        Images {
            vm,
            env,
            id,
            spare: ImageSet::new(),
            error: None,
        }
    }

    fn note(&mut self, r: Result<(), VmError>) {
        if let Err(e) = r
            && self.error.is_none()
        {
            self.error = Some(e);
        }
    }

    /// `Con::executef(db, 3, name, obj, slot)`, charged [`IMAGE_CALL_DEPTH`] more depth units.
    fn call(&mut self, db: DbId, name: &[u8], slot: usize) {
        let args = [
            self.id.to_string().into_bytes(),
            slot.to_string().into_bytes(),
        ];
        let r = self
            .vm
            .with_extra_depth(IMAGE_CALL_DEPTH, |vm| vm.call_method(db, name, &args))
            .map(|_| ());
        self.note(r);
    }

    /// The mirror written; the first error.
    pub fn finish(self) -> Result<(), VmError> {
        sync_mirror(self.vm, self.id);
        match self.error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl ImageHost for Images<'_> {
    fn images(&mut self) -> &mut ImageSet {
        match ext::get_mut(self.vm.objects_mut(), self.id) {
            Some(e) => e.images.get_or_insert_with(Default::default),
            None => &mut self.spare,
        }
    }

    fn image_data(&mut self, db: DbId) -> Option<Arc<ImageData>> {
        view_of(self.vm, self.env.shapes.values(), db)
    }

    fn raise_mask(&mut self, bits: u32) {
        ghosts::mark(self.vm, self.id, bits);
    }

    fn energy(&mut self) -> f32 {
        super::vitals::energy_level(self.vm, self.id)
    }

    fn set_energy(&mut self, energy: f32) {
        super::vitals::set_energy_level(self.vm, self.id, energy);
    }

    fn state_script(&mut self, db: DbId, script: &str, slot: usize) {
        self.call(db, script.as_bytes(), slot);
    }

    fn on_mount(&mut self, db: DbId, slot: usize) {
        self.call(db, b"onMount", slot);
    }

    fn on_unmount(&mut self, db: DbId, slot: usize) {
        self.call(db, b"onUnmount", slot);
    }

    fn fire_animate(&mut self) {
        let r = crate::scene::shapebase::animate(self.vm, self.id, 0);
        self.note(r);
    }

    fn find_image_data(&mut self, name: &str) -> Option<DbId> {
        find_image(self.vm, name)
    }
}

/// `updateImageState(slot, 0.032f)` for a Player's tick (the state parked by the caller).
pub fn tick_slot(vm: &mut Vm, env: &SimEnv, id: ObjId, slot: usize) -> Result<(), VmError> {
    let mut h = Images::new(vm, env, id);
    machine::update_image_state(&mut h, slot, IMAGE_DT);
    h.finish()
}

/// `setImageTriggerState(slot, down)` from the move, the state parked by the caller.
pub fn set_trigger(
    vm: &mut Vm,
    env: &SimEnv,
    id: ObjId,
    slot: usize,
    down: bool,
) -> Result<(), VmError> {
    let mut h = Images::new(vm, env, id);
    machine::set_image_trigger(&mut h, slot, down);
    h.finish()
}

// ------------------------------------------------------------------------------------------------------------------ the muzzle

/// The slots and the one view the console's muzzle methods read, copied (the ray caster holds the `Vm` meanwhile).
struct Snapshot {
    set: Box<ImageSet>,
    data: Option<Arc<ImageData>>,
}

impl ImageHost for Snapshot {
    fn images(&mut self) -> &mut ImageSet {
        &mut self.set
    }
    fn image_data(&mut self, db: DbId) -> Option<Arc<ImageData>> {
        self.data.clone().filter(|d| d.id == db)
    }
    fn raise_mask(&mut self, _bits: u32) {}
    fn energy(&mut self) -> f32 {
        0.0
    }
    fn set_energy(&mut self, _energy: f32) {}
    fn state_script(&mut self, _db: DbId, _script: &str, _slot: usize) {}
    fn on_mount(&mut self, _db: DbId, _slot: usize) {}
    fn on_unmount(&mut self, _db: DbId, _slot: usize) {}
}

/// `Container::castRay` for the muzzle chain: the object and its riders out, and for the corrected aim the
/// object it is mounted on with its riders too.
struct Rays<'a> {
    vm: &'a mut Vm,
    env: &'a SimEnv,
    id: ObjId,
    error: Option<VmError>,
}

impl Rays<'_> {
    /// `disableCollision`'s walk: the object and everything mounted on it, charged (`SIM_MOUNT_CHAIN` objects at most).
    fn with_riders(&self, root: ObjId, out: &mut Vec<ObjId>) {
        let start = out.len();
        if !out.contains(&root) {
            out.push(root);
        }
        let mut i = start;
        while i < out.len() && out.len() <= super::mount::SIM_MOUNT_CHAIN {
            let riders = super::mount::riders(self.vm, out[i]);
            if !self.vm.charge_work(1 + riders.len() as u64) {
                break;
            }
            for &r in riders {
                if !out.contains(&r) {
                    out.push(r);
                }
            }
            i += 1;
        }
    }
}

impl RayCaster for Rays<'_> {
    fn type_masks(&self) -> &collide::TypeMasks {
        self.env.masks
    }

    fn muzzle_point(
        &mut self,
        s: &sim::player::PlayerState,
        mount: F3,
        muzzle: F3,
        player: bool,
    ) -> F3 {
        let classes = Rc::clone(&self.env.classes);
        classes.muzzle_point(self, s, mount, muzzle, player)
    }

    fn corrected_aim(&mut self, eye: &collide::math::Matrix, muzzle: F3, point: F3) -> Option<F3> {
        let classes = Rc::clone(&self.env.classes);
        classes.corrected_aim(self, eye, muzzle, point)
    }

    fn cast_ray(&mut self, start: F3, end: F3, mask: u32, exclude_mount: bool) -> Option<RayHit> {
        let mut out = Vec::new();
        self.with_riders(self.id, &mut out);
        if exclude_mount && let Some(m) = super::mount::mount_of(self.vm, self.id) {
            self.with_riders(m, &mut out);
        }
        for o in &out {
            switch_collision(self.vm, *o, true);
        }
        let hit = container_ray(self.vm, self.env, start, end, mask);
        for o in &out {
            switch_collision(self.vm, *o, false);
        }
        match hit {
            Ok(h) => h,
            Err(e) => {
                if self.error.is_none() {
                    self.error = Some(e);
                }
                None
            }
        }
    }
}

/// A connection's control object is this object (`isFirstPerson`: the controlling client's control object is `this` and its
/// camera position is 0.0: the `GameConnection` constructor's, then what each client head's first flag sets, `readPacket`).
pub fn first_person(vm: &Vm, id: ObjId) -> bool {
    ghosts::is_control_object(vm, id) && ghosts::controlling_camera_pos(vm, id) == Some(0.0)
}

/// The words the muzzle chain reads of a ShapeBase that is not a Player (its matrix, world box and scale): a stand-in state.
fn stand_in(vm: &Vm, id: ObjId) -> PlayerState {
    let m = matrix_of(vm, id).unwrap_or(IDENTITY);
    let mut s = PlayerState::new(
        ObjectId(id),
        &PlayerData::default(),
        [m[3], m[7], m[11]],
        0.0,
    );
    crate::sim_classes::sim_classes(vm)
        .default_player_kind()
        .update_scaled_box(&mut s);
    s.obj_to_world = m;
    s.scale = scale_of(vm, id);
    if let Some(sc) = ext::get(vm.objects(), id).and_then(|e| e.scene) {
        s.world_box = Box3::new(sc.world_min, sc.world_max);
    }
    s
}

fn stand_in_info(vm: &Vm) -> Rc<PlayerInfo> {
    let classes = crate::sim_classes::sim_classes(vm);
    let data = PlayerData {
        actions: classes.action_table().numbers(),
        ..PlayerData::default()
    };
    Rc::new(PlayerInfo {
        data,
        class: classes.default_player_kind(),
        actions: classes.action_table(),
        motions: Vec::new(),
        look_action: 0,
        max_time_scale: 1.5,
        max_freelook: 0.0,
        walk: None,
    })
}

fn text3(v: F3) -> Vec<u8> {
    v.iter()
        .map(|x| format_g(f64::from(*x)))
        .collect::<Vec<_>>()
        .join(" ")
        .into_bytes()
}

/// `getMuzzleVector` and `getMuzzlePoint`: `"%g %g %g"`, NULL for a client-side call.
#[inline(never)]
fn muzzle_native(vm: &mut Vm, env: &SimEnv, c: &NativeCall, vector: bool) -> Native {
    let Some(id) = c.this_object else {
        return Native::Str(Vec::new());
    };
    let arg = c
        .argv
        .get(2)
        .map(|a| String::from_utf8_lossy(a).into_owned());
    // Boxed: the state is large, and this frame sits under a script's whole depth.
    let player_state = super::player::state(vm, id).map(|s| Box::new(s.clone()));
    let player = player_state.is_some();
    let s = player_state.unwrap_or_else(|| Box::new(stand_in(vm, id)));
    let info = super::player::info(vm, id).unwrap_or_else(|| stand_in_info(vm));
    // The slot's view, read before the casts hold the `Vm`.
    let set = Box::new(set_of(vm, id).cloned().unwrap_or_default());
    let slot = arg
        .as_deref()
        .map(console::atoi)
        .filter(|n| (0..SLOTS as i32).contains(n));
    let db = slot.and_then(|n| set.slots[n as usize].db);
    let data = db.and_then(|d| view_of(vm, env.shapes.values(), d));
    let fp = first_person(vm, id);
    let client = ghosts::controlling_client(vm, id).map(|_| AimClient {
        freelook_value: 0.0,
        ai: false,
    });
    let shape = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take());
    let actions = info.data.actions;
    let mut skel = HostSkeleton::new(shape, info);
    skel.first_person = fp;
    let mut snap = Snapshot { set, data };
    let (out, error) = {
        let mut rays = Rays {
            vm: &mut *vm,
            env,
            id,
            error: None,
        };
        let mut ctx = MuzzleCtx {
            s: &s,
            skel: &mut skel,
            rays: &mut rays,
            first_person: fp,
            client,
            actions,
        };
        let out = if vector {
            Some(console::get_muzzle_vector(
                &mut snap,
                &mut ctx,
                arg.as_deref(),
                player,
            ))
        } else {
            console::get_muzzle_point(&mut snap, &mut ctx, arg.as_deref(), player)
        };
        (out, rays.error)
    };
    let _ = vm.charge_work(skel.work);
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.shape = skel.st;
    }
    if let Some(e) = error {
        return Native::Fail(e);
    }
    Native::Str(out.map_or_else(Vec::new, text3))
}

// ----------------------------------------------------------------------------------------------------------------- the methods

/// One of the state methods through `::sim::image::console::call`.
#[inline(never)]
fn state_native(vm: &mut Vm, env: &SimEnv, c: &NativeCall, name: &str) -> Native {
    let Some(id) = c.this_object else {
        return Native::Str(Vec::new());
    };
    let args: Vec<String> = c
        .argv
        .iter()
        .skip(2)
        .map(|a| String::from_utf8_lossy(until_nul(a)).into_owned())
        .collect();
    let refs: Vec<Option<&str>> = args.iter().map(|a| Some(a.as_str())).collect();
    let mut h = Images::new(vm, env, id);
    let v = console::call(&mut h, name, &refs);
    if let Err(e) = h.finish() {
        return Native::Fail(e);
    }
    match v {
        None => Native::Str(Vec::new()),
        Some(ImgValue::Bool(b)) => Native::Bool(b),
        Some(ImgValue::Int(n)) => Native::Int(n),
        Some(ImgValue::Str(s)) => Native::Str(s.map_or_else(Vec::new, String::into_bytes)),
    }
}

/// The `ShapeBase` image methods (`getSlotTransform` is the host's: the mount transform of the slot, which reads
/// no image).
pub fn register(vm: &mut Vm, env: &Rc<SimEnv>) {
    for spec in console::METHODS.iter() {
        let name = spec.name;
        let env = Rc::clone(env);
        match name {
            "getSlotTransform" => {}
            "getMuzzleVector" | "getMuzzlePoint" => {
                let vector = name == "getMuzzleVector";
                register_method(vm, "ShapeBase", name, move |vm, c| {
                    muzzle_native(vm, &env, c, vector)
                });
            }
            _ => {
                register_method(vm, "ShapeBase", name, move |vm, c| {
                    state_native(vm, &env, c, name)
                });
            }
        }
    }
}
