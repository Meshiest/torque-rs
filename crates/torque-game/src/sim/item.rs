//! The Item host glue (the simulation's host half): the object, its tick over `::sim::item`, its console methods and its ghost
//! view's state.
//!
//! **Where the state lives** (as the Player's, see [`super::player`]). `ObjectExt::item` holds an [`ItemExt`]: the `::sim`
//! Item (its state, its working set). The `ItemState` is the truth for every field the step reads, the transform included; the
//! VM's `position` matrix and the scene's boxes are copies written at every placement ([`mirror`]), then the bins are checked.
//! While the Item's own tick runs, its `Item` is out of the ext and, around each script callback, its state is *parked* in
//! [`ItemExt::parked`]: [`state`]/[`state_mut`] find it in either place. The persist fields `static`, `rotate` and
//! `collideable` are the VM's slots: the state takes them before every use, as the members are the fields themselves.
//!
//! **The datablock** is read live (`ItemData`'s fields, the class table's defaults for unset ones): the step reads its
//! members through the datablock every tick.
//!
//! - `Item::onNewDataBlock` (Torque: `ShapeBase`'s, then the Item's): [`new_datablock`]: the first ItemData makes the state at
//!   the transform the fields gave
//! - `Item::onAdd` (Torque: `addToScene` and the script): `ItemClass`: the dynamic type bit, a static item at rest,
//!   `addToScene`, `%db.onAdd(%obj)`
//! - `processTick` (Torque: `Item::processTick`): [`ItemTicker`] -> [`tick`] over [`HostImpl`]
//! - `setTransform` (Torque: `Item::setTransform`): [`set_transform`] (from `SceneObject::setTransform`'s dispatch)
//! - `setVelocity`, `applyImpulse`, `getVelocity` (Torque: the Item's): [`set_velocity`], [`apply_impulse`], [`velocity`]
//! - the 7 console methods (Torque: the Item's): [`register`]
//!
//! **Callbacks** at the Item's call sites, each `%db.<name>(%obj, ...)`: `notifyCollision` (`onCollision` on the Item, then on
//! the other object with the same vector), `onEnterLiquid`, `onLeaveLiquid`.
//!
//! **Listed (engine-only).** `updateContainer` for an Item is the no-zone form (`::sim::item::no_zones`): an Item in a
//! PhysicalZone gets no drag, buoyancy or gravity factor from it yet. `setHidden` is ShapeBase's
//! ([`crate::scene::shapebase::set_hidden`]): a hidden Item is out of the container.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/item.cpp`
//! (Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.

use std::rc::Rc;

use ::sim::Ticking;
use ::sim::item::{Item, ItemData, ItemHost, ItemState, no_zones};
use collide::math::{Box3, F3, Matrix};
use collide::{ObjectId, World};
use ts_vm::classes::EngineClass;
use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::text::format_g;
use ts_vm::vm::{Native, NativeCall, Vm, VmError};

use super::TickCx;
use super::hosted::{Slot, call_db, container_ray, switch_collision, vector_args};
use super::player::tick::SimEnv;
use crate::methods::register_method;
use crate::scene::access::{matrix_of, object_field, scale_of, value_named, value_of};
use crate::scene::camera::is_class;
use crate::scene::container::Container;
use crate::scene::ext;
use crate::scene::ghost as ghosts;
use crate::scene::object as scene;
use ::sim::Move;

/// An Item's host state.
#[derive(Debug, Clone)]
pub struct ItemExt {
    /// The step's Item; `None` while its tick runs.
    pub body: Option<Item>,
    /// The state while a callback of its own tick runs script.
    pub parked: Option<Box<ItemState>>,
}

impl PartialEq for ItemExt {
    fn eq(&self, other: &Self) -> bool {
        self.state() == other.state()
    }
}

impl ItemExt {
    pub fn state(&self) -> Option<&ItemState> {
        self.body.as_ref().map(|i| &*i.s).or(self.parked.as_deref())
    }

    pub fn state_mut(&mut self) -> Option<&mut ItemState> {
        match self.body.as_mut() {
            Some(i) => Some(&mut *i.s),
            None => self.parked.as_deref_mut(),
        }
    }
}

/// The Item's state (its own tick's parked copy while a callback runs).
pub fn state(vm: &Vm, id: ObjId) -> Option<&ItemState> {
    ext::get(vm.objects(), id)?.item.as_deref()?.state()
}

pub fn state_mut(vm: &mut Vm, id: ObjId) -> Option<&mut ItemState> {
    ext::get_mut(vm.objects_mut(), id)?
        .item
        .as_deref_mut()?
        .state_mut()
}

/// Whether the object is an `Item` with its state (its `setTransform` and velocity are its own).
pub fn is_item(vm: &Vm, id: ObjId) -> bool {
    state(vm, id).is_some()
}

// --------------------------------------------------------------------------------------------------------------- the datablock

fn float(vm: &Vm, db: ObjId, name: &[u8], default: f32) -> f32 {
    match value_named(vm, db, name) {
        Some(Value::Float(x)) => x,
        Some(Value::Int(n)) => n as f32,
        _ => default,
    }
}

fn boolean(vm: &Vm, db: ObjId, name: &[u8], default: bool) -> bool {
    match value_named(vm, db, name) {
        Some(Value::Bool(b)) => b,
        Some(Value::Int(n)) => n != 0,
        _ => default,
    }
}

/// `ItemData` as the step reads it from datablock `db` (the class's defaults for what it does not set).
pub fn data_of(vm: &Vm, db: ObjId) -> ItemData {
    let mut d = ItemData::default();
    d.mass = float(vm, db, b"mass", d.mass);
    d.drag = float(vm, db, b"drag", d.drag);
    d.density = float(vm, db, b"density", d.density);
    d.friction = float(vm, db, b"friction", d.friction);
    d.elasticity = float(vm, db, b"elasticity", d.elasticity);
    d.sticky = boolean(vm, db, b"sticky", d.sticky);
    d.gravity_mod = float(vm, db, b"gravityMod", d.gravity_mod);
    d.max_velocity = float(vm, db, b"maxVelocity", d.max_velocity);
    d.dynamic_type = match value_of(vm, db, b"dynamicType") {
        Some(Value::Int(n)) => n as u32,
        _ => d.dynamic_type,
    };
    d.preload(
        ext::get(vm.objects(), db)
            .and_then(|e| e.shape_resource.as_ref())
            .is_some_and(|r| r.current().shape.is_some()),
    );
    d
}

/// The ItemData of Item `id` now.
fn data(vm: &Vm, id: ObjId) -> Option<ItemData> {
    let db = object_field(vm, id, b"dataBlock")?;
    is_class(vm, db, "ItemData").then(|| data_of(vm, db))
}

/// The object box the scene holds for `id` (the shape's bounds, `ShapeBase::onNewDataBlock`).
fn scene_box(vm: &Vm, id: ObjId) -> Option<Box3> {
    let s = ext::get(vm.objects(), id)?.scene?;
    Some(Box3::new(s.object_min, s.object_max))
}

/// The persist fields and the scene's scale, box and type mask into the state (the members are those fields).
fn take_fields(vm: &Vm, id: ObjId, s: &mut ItemState) {
    let flag = |name: &[u8]| matches!(value_named(vm, id, name), Some(Value::Bool(true)));
    s.is_static = flag(b"static");
    s.rotate = flag(b"rotate");
    s.collideable = flag(b"collideable");
    s.type_mask = ext::get(vm.objects(), id).map_or(s.type_mask, |e| e.type_mask);
    let scale = scale_of(vm, id);
    let obj_box = scene_box(vm, id);
    if s.scene.scale != scale || obj_box.is_some_and(|b| b != s.scene.obj_box) {
        s.scene.scale = scale;
        if let Some(b) = obj_box {
            s.scene.obj_box = b;
        }
        s.scene.reset_world_box();
    }
}

fn sync_fields(vm: &mut Vm, id: ObjId) {
    let Some(mut s) = state(vm, id).cloned() else {
        return;
    };
    take_fields(vm, id, &mut s);
    if let Some(t) = state_mut(vm, id) {
        *t = s;
    }
}

/// `Item::onNewDataBlock` after the shape part: the first ItemData makes the state at the transform the fields gave (raw,
/// as the `position`/`rotation` fields store it), with the scene's box and scale; a later one keeps it (the step reads the
/// datablock live).
pub fn new_datablock(vm: &mut Vm, id: ObjId, db: ObjId) {
    if !is_class(vm, id, "Item") || !is_class(vm, db, "ItemData") {
        return;
    }
    if ext::get(vm.objects(), id).is_some_and(|e| e.item.is_some()) {
        return;
    }
    let d = data_of(vm, db);
    let m = matrix_of(vm, id).unwrap_or(collide::math::IDENTITY);
    let obj_box = scene_box(vm, id).unwrap_or_default();
    let mut s = ItemState::new(ObjectId(id), &d, obj_box, m, crate::masks::type_masks(vm));
    take_fields(vm, id, &mut s);
    s.scene.reset_world_box();
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.item = Some(Box::new(ItemExt {
            body: Some(Item::new(s)),
            parked: None,
        }));
    }
}

// ------------------------------------------------------------------------------------------------------------------- placement

/// Writes the state's transform and world box into the VM's matrix and the scene (the copies), without the bins.
fn write_copies(vm: &mut Vm, id: ObjId, s: &ItemState) {
    crate::scene::access::set_matrix(vm, id, &s.scene.obj_to_world);
    if let Some(sc) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.scene.as_mut()) {
        sc.world_min = s.scene.world_box.min;
        sc.world_max = s.scene.world_box.max;
        sc.world_center = s.scene.world_center;
        sc.world_radius = s.scene.world_radius;
    }
}

/// The copies of the state now, then `checkBins`.
pub fn mirror(vm: &mut Vm, container: &Rc<Container>, id: ObjId) {
    if let Some(s) = state(vm, id).cloned() {
        write_copies(vm, id, &s);
    }
    scene::check_bins(vm, container, id);
}

/// The mask bits the state set, into the ghost layer's dirty masks (`setMaskBits`).
fn flush_mask(vm: &mut Vm, id: ObjId) {
    let bits = state_mut(vm, id).map_or(0, |s| std::mem::take(&mut s.mask_bits));
    if bits != 0 {
        ghosts::mark(vm, id, bits);
    }
}

/// The world a placement outside the tick re-bins in (the process list's environment).
fn env_of(vm: &Vm, id: ObjId) -> Option<Rc<SimEnv>> {
    ext::get(vm.objects(), id)?.process.as_ref()?.env()
}

/// Runs `f` on Item `id` outside its tick (a script's call): the Item out of its ext (a parked state is lent as an Item for the
/// call), a host over the plain `Vm`, the copies and the mask bits after.
pub fn with_item<R>(
    vm: &mut Vm,
    id: ObjId,
    f: impl FnOnce(&mut Item, &mut dyn ItemHost, &ItemData) -> R,
) -> Option<R> {
    sync_fields(vm, id);
    let d = data(vm, id).unwrap_or_default();
    let env = env_of(vm, id);
    let masks = crate::masks::type_masks(vm);
    let e = ext::get_mut(vm.objects_mut(), id)?.item.as_deref_mut()?;
    let (mut item, parked) = match e.body.take() {
        Some(i) => (i, false),
        None => (Item::new(*e.parked.take()?), true),
    };
    let r = match env.as_deref() {
        Some(env) => {
            let mut host = HostImpl::new(vm, env, id);
            let r = f(&mut item, &mut host, &d);
            host.slot.into_vm();
            r
        }
        None => f(&mut item, &mut Detached(NoWorld, masks), &d),
    };
    if let Some(e) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.item.as_deref_mut()) {
        if parked {
            e.parked = Some(item.s);
        } else {
            e.body = Some(item);
        }
    }
    match env {
        Some(env) => mirror(vm, &env.container, id),
        None => {
            if let Some(s) = state(vm, id).cloned() {
                write_copies(vm, id, &s);
            }
        }
    }
    flush_mask(vm, id);
    Some(r)
}

/// `Item::setTransform` for a script's `setTransform` (the caller checks the bins).
pub fn set_transform(vm: &mut Vm, id: ObjId, m: &Matrix) {
    let m = *m;
    with_item(vm, id, |item, host, _| item.set_transform(&m, host));
}

/// `Item::setVelocity`.
pub fn set_velocity(vm: &mut Vm, id: ObjId, v: F3) {
    with_item(vm, id, |item, _, _| item.set_velocity(v));
}

/// `Item::applyImpulse`: it SETS the velocity to `impulse / mass`.
pub fn apply_impulse(vm: &mut Vm, id: ObjId, j: F3) {
    with_item(vm, id, |item, _, d| item.apply_impulse(d, j));
}

/// `Item::getVelocity`.
pub fn velocity(vm: &Vm, id: ObjId) -> Option<F3> {
    state(vm, id).map(|s| s.velocity)
}

/// The host of a call with no process list (an Item that is not registered with one): no world, no clock.
struct Detached(NoWorld, &'static collide::TypeMasks);

struct NoWorld;

impl World for NoWorld {
    fn object(&self, _id: ObjectId) -> Option<collide::ObjectView> {
        None
    }

    fn find_objects(
        &mut self,
        _query: &Box3,
        _mask: u32,
        _exclude: Option<ObjectId>,
        _limit: usize,
        _out: &mut Vec<ObjectId>,
    ) -> Result<(), collide::Exhausted> {
        Ok(())
    }

    fn convex_source(&self, _id: ObjectId) -> collide::convex::ConvexSource {
        collide::convex::ConvexSource::None
    }

    fn hull(&self, _id: ObjectId, _slot: u32) -> Option<Rc<collide::mesh::Hull>> {
        None
    }

    fn charge(&mut self, _units: u64) -> bool {
        true
    }
}

impl ItemHost for Detached {
    fn type_masks(&self) -> &collide::TypeMasks {
        self.1
    }

    fn world(&mut self) -> &mut dyn World {
        &mut self.0
    }

    fn sim_time(&self) -> u32 {
        0
    }

    fn cast_ray(&mut self, _start: F3, _end: F3, _mask: u32) -> Option<::sim::RayHit> {
        None
    }
}

// ------------------------------------------------------------------------------------------------------------------- the tick

/// An Item's entry in the process list.
pub struct ItemTicker {
    pub id: ObjId,
    pub env: Rc<SimEnv>,
}

impl Ticking<TickCx<'_>> for ItemTicker {
    fn class(&self) -> &'static str {
        "Item"
    }

    fn process_tick(&mut self, cx: &mut TickCx<'_>, _mv: Option<&Move>) {
        tick(cx.vm, &self.env, self.id);
    }
}

/// One tick of Item `id` (`Item::processTick` through `::sim`).
pub fn tick(vm: &mut Vm, env: &SimEnv, id: ObjId) {
    if !vm.objects().contains(id) {
        return;
    }
    // One unit a tick; a refused budget skips the step (the container's own charges stop it mid-way: `Exhausted`).
    if !vm.charge_work(1) {
        return;
    }
    sync_fields(vm, id);
    let Some(d) = data(vm, id) else {
        return;
    };
    let Some(mut item) = ext::get_mut(vm.objects_mut(), id)
        .and_then(|e| e.item.as_deref_mut())
        .and_then(|e| e.body.take())
    else {
        return;
    };
    item.s.mask_bits = 0;
    vm.hold(id);
    let mut host = HostImpl::new(vm, env, id);
    let _ = item.process_tick(&d, &mut host);
    if host.moved {
        host.flush_move(&item.s);
    }
    let vm = host.slot.into_vm();
    if let Some(e) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.item.as_deref_mut()) {
        e.body = Some(item);
    }
    flush_mask(vm, id);
    let _ = vm.release(id);
}

/// The host of one Item's step (or of a script's call into it).
pub struct HostImpl<'a> {
    slot: Slot<'a>,
    env: &'a SimEnv,
    id: ObjId,
    /// The box the state swaps with while it is parked.
    spare: Option<Box<ItemState>>,
    /// The first error a callback's script returned.
    pub error: Option<VmError>,
    /// The transform changed since the copies were last written.
    moved: bool,
}

impl<'a> HostImpl<'a> {
    fn new(vm: &'a mut Vm, env: &'a SimEnv, id: ObjId) -> HostImpl<'a> {
        HostImpl {
            slot: Slot::Vm(vm),
            env,
            id,
            spare: None,
            error: None,
            moved: false,
        }
    }

    fn note(&mut self, r: Result<(), VmError>) {
        if let Err(e) = r
            && self.error.is_none()
        {
            self.error = Some(e);
        }
    }

    /// Writes the Item's copies (matrix, world box, bins) from `s`.
    fn flush_move(&mut self, s: &ItemState) {
        let id = self.id;
        let container = Rc::clone(&self.env.container);
        let vm = self.slot.vm();
        write_copies(vm, id, s);
        scene::check_bins(vm, &container, id);
        self.moved = false;
    }

    /// Runs script with the Item's state parked where its methods find it.
    fn script(&mut self, s: &mut ItemState, f: impl FnOnce(&mut Vm, ObjId) -> Result<(), VmError>) {
        if self.moved {
            self.flush_move(s);
        }
        let id = self.id;
        let mut parked = self.spare.take().unwrap_or_else(|| Box::new(s.clone()));
        std::mem::swap(&mut *parked, s);
        let vm = self.slot.vm();
        let mut kept = None;
        match ext::get_mut(vm.objects_mut(), id).and_then(|e| e.item.as_deref_mut()) {
            Some(e) => e.parked = Some(parked),
            None => kept = Some(parked),
        }
        let r = f(vm, id);
        let back = kept.or_else(|| {
            ext::get_mut(vm.objects_mut(), id)
                .and_then(|e| e.item.as_deref_mut())
                .and_then(|e| e.parked.take())
        });
        if let Some(mut b) = back {
            std::mem::swap(&mut *b, s);
            self.spare = Some(b);
        }
        // The callback may have moved it (`setTransform`): the copies were written by that call.
        take_fields(vm, id, s);
        self.note(r);
    }
}

impl<'a> ItemHost for HostImpl<'a> {
    fn type_masks(&self) -> &collide::TypeMasks {
        self.env.masks
    }

    fn world(&mut self) -> &mut dyn World {
        self.slot.world(self.env)
    }

    fn sim_time(&self) -> u32 {
        self.slot.read(|vm| vm.sim_time())
    }

    fn cast_ray(&mut self, start: F3, end: F3, mask: u32) -> Option<::sim::RayHit> {
        // After a refused charge every later cast of the step misses (deterministic: the step ends as far as it got).
        if self.error.is_some() {
            return None;
        }
        let env = self.env;
        match container_ray(self.slot.vm(), env, start, end, mask) {
            Ok(h) => h,
            Err(e) => {
                self.note(Err(e));
                None
            }
        }
    }

    fn disable_collision(&mut self, id: ObjectId) {
        switch_collision(self.slot.vm(), id.0, true);
    }

    fn enable_collision(&mut self, id: ObjectId) {
        switch_collision(self.slot.vm(), id.0, false);
    }

    fn object_velocity(&mut self, id: ObjectId) -> F3 {
        self.slot.read(|vm| object_velocity(vm, id.0))
    }

    fn update_container(&mut self, s: &mut ItemState, db: &ItemData) {
        no_zones(s, db);
    }

    fn moved(&mut self, _s: &mut ItemState) {
        self.moved = true;
    }

    fn on_collision(&mut self, s: &mut ItemState, other: ObjectId, vec: F3) -> bool {
        let mut alive = true;
        self.script(s, |vm, id| {
            let [v, len] = vector_args(vec);
            call_db(
                vm,
                id,
                b"onCollision",
                &[other.0.to_string().into_bytes(), v.clone(), len.clone()],
            )?;
            alive = vm.objects().contains(id);
            if alive && vm.objects().contains(other.0) && is_class(vm, other.0, "ShapeBase") {
                call_db(
                    vm,
                    other.0,
                    b"onCollision",
                    &[id.to_string().into_bytes(), v, len],
                )?;
            }
            Ok(())
        });
        alive
    }

    fn on_enter_liquid(&mut self, s: &mut ItemState, coverage: f32, liquid_type: u32) {
        self.script(s, |vm, id| {
            call_db(
                vm,
                id,
                b"onEnterLiquid",
                &[
                    format_g(f64::from(coverage)).into_bytes(),
                    liquid_type.to_string().into_bytes(),
                ],
            )
        });
    }

    fn on_leave_liquid(&mut self, s: &mut ItemState, liquid_type: u32) {
        self.script(s, |vm, id| {
            call_db(
                vm,
                id,
                b"onLeaveLiquid",
                &[liquid_type.to_string().into_bytes()],
            )
        });
    }
}

/// An object's velocity (`getVelocity`): a Player's, a vehicle's, an Item's, zero for anything else.
pub fn object_velocity(vm: &Vm, id: ObjId) -> F3 {
    if let Some(s) = super::player::state(vm, id) {
        return s.velocity;
    }
    if let Some(v) = super::vehicle::velocity(vm, id) {
        return v;
    }
    velocity(vm, id).unwrap_or([0.0; 3])
}

// ------------------------------------------------------------------------------------------------------------------- the class

/// `Item::onAdd` after `ShapeBase::onAdd`: with its ItemData, the datablock's `dynamicType` bit joins the type mask, a
/// static item is at rest, the render base is the position, the cache is unset; the object enters the container and the
/// datablock's script `onAdd(%obj)` runs. Without one, only the container (as before the Item was simulated).
struct ItemClass {
    container: Rc<Container>,
}

impl EngineClass for ItemClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let Some(d) = data(vm, id).filter(|_| is_item(vm, id)) else {
            scene::add_to_scene(vm, &self.container, id);
            return Ok(true);
        };
        let masks = crate::masks::type_masks(vm);
        let bit = d.dynamic_type & masks.item_dynamic_allowed;
        if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
            e.type_mask |= bit;
        }
        sync_fields(vm, id);
        if let Some(e) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.item.as_deref_mut())
            && let Some(item) = e.body.as_mut()
        {
            item.on_add(&d, masks);
        }
        if let Some(s) = state(vm, id).cloned() {
            write_copies(vm, id, &s);
        }
        scene::add_to_scene(vm, &self.container, id);
        if let Some(db) = object_field(vm, id, b"dataBlock") {
            vm.call_method(db, b"onAdd", &[id.to_string().into_bytes()])?;
        }
        Ok(true)
    }

    /// `Item::onRemove`: the datablock's script `onRemove(%obj)` (`scriptOnRemove`; the cache is unset just before,
    /// which no script sees), then the cache unset and the working set let go.
    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        let script = crate::scene::shapebase::script_on_remove(vm, id);
        if let Some(e) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.item.as_deref_mut())
            && let Some(item) = e.body.as_mut()
        {
            item.on_remove();
        }
        script
    }
}

pub fn arg(c: &NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&b""[..], Vec::as_slice)
}

/// `%g %g %g`.
fn text3_g(v: F3) -> Vec<u8> {
    v.iter()
        .map(|x| format_g(f64::from(*x)))
        .collect::<Vec<_>>()
        .join(" ")
        .into_bytes()
}

/// Gives Item its behaviour (replacing the scene entry) and its seven console methods. After `shapebase::register`.
pub fn register(vm: &mut Vm, container: &Rc<Container>) {
    vm.set_class_behaviour(
        "Item",
        Rc::new(ItemClass {
            container: Rc::clone(container),
        }),
    );
    // The persist fields' bytes.
    register_method(vm, "Item", "isStatic", |vm, c| {
        Native::Bool(
            c.this_object
                .is_some_and(|id| matches!(value_of(vm, id, b"static"), Some(Value::Bool(true)))),
        )
    });
    register_method(vm, "Item", "isRotating", |vm, c| {
        Native::Bool(
            c.this_object
                .is_some_and(|id| matches!(value_of(vm, id, b"rotate"), Some(Value::Bool(true)))),
        )
    });
    // A ShapeBase found by the argument becomes the exempt object for 15 ticks; true on success.
    register_method(vm, "Item", "setCollisionTimeout", |vm, c| {
        let Some(id) = c.this_object else {
            return Native::Bool(false);
        };
        let Some(other) = vm
            .objects()
            .find(arg(c, 2))
            .filter(|o| is_class(vm, *o, "ShapeBase"))
        else {
            return Native::Bool(false);
        };
        let done = with_item(vm, id, |item, _, _| {
            item.set_collision_timeout(ObjectId(other))
        });
        Native::Bool(done.is_some())
    });
    // The sticky position and normal, which nothing writes: zeros.
    register_method(vm, "Item", "getLastStickyPos", |_, _| {
        Native::Str(text3_g([0.0; 3]))
    });
    register_method(vm, "Item", "getLastStickyNormal", |_, _| {
        Native::Str(text3_g([0.0; 3]))
    });
}
