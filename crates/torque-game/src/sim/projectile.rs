//! The Projectile host glue (the simulation's host half): the object, its tick over `::sim::projectile`, and the state its ghost
//! view reads. A host that simulates its Projectiles itself names its own ticker ([`super::adapters::GameTickers`]); without one
//! the process list ticks a Projectile with this one.
//!
//! **Where the state lives.** `ObjectExt::projectile` holds a [`ProjectileExt`]: the `::sim` Projectile. Its state is the truth
//! for the position and the velocity; the VM's `position` matrix and the scene's boxes are copies written after every move,
//! then the bins are checked.
//!
//! - `Projectile::onAdd` (Torque: after `GameBase::onAdd`): [`ProjectileClass`]: a Projectile without a ProjectileData is
//!   refused; the state starts at the persist fields `initialPosition` and `initialVelocity`, with `sourceObject` (an object
//!   id, resolved now: the source) and `sourceSlot`; the object enters the container.
//! - `processTick` (Torque: `Projectile::processTick`): [`ProjectileTicker`] -> [`tick`] over [`HostImpl`].
//!
//! **Callbacks** at the step's call points, each `%db.<name>(%obj, ...)`: `onCollision(%obj, %col, %fade, %pos, %normal)` and
//! `onExplode(%obj, %pos, %fade)`, the vectors as `"%g %g %g"`. The script runs with the object's copies written, so its
//! `getPosition` is the hit point. A Projectile that reaches its lifetime is deleted after its tick; one that exploded is
//! deleted [`::sim::projectile::DELETE_WAIT_MS`] later (a scheduled `delete`, as the engine's `ObjectDeleteEvent`).
//!
//! **Listed (engine-only).** The source is not put ahead of the Projectile in the process list (`processAfter`): a Projectile
//! made during its source's tick is ticked after it anyway, being new.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/projectile.cpp`
//! (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use std::rc::Rc;

use ::sim::projectile::{Projectile, ProjectileData, ProjectileHost, ProjectileState};
use ::sim::{Move, Ticking};
use collide::ObjectId;
use collide::math::{Box3, F3, IDENTITY, Matrix};
use ts_vm::classes::EngineClass;
use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::text::format_g;
use ts_vm::vm::{Vm, VmError};

use super::TickCx;
use super::hosted::{call_db, container_ray, switch_collision};
use super::player::tick::SimEnv;
use crate::scene::access::{object_field, value_named};
use crate::scene::camera::is_class;
use crate::scene::container::Container;
use crate::scene::ext;
use crate::scene::ghost as ghosts;
use crate::scene::object as scene;

/// A Projectile's host state.
#[derive(Debug, Clone)]
pub struct ProjectileExt {
    /// The step's Projectile; `None` while its tick runs.
    pub body: Option<Projectile>,
    /// The state as it was when the tick took the body out (what a script in a callback reads).
    pub parked: Option<Box<ProjectileState>>,
}

impl PartialEq for ProjectileExt {
    fn eq(&self, other: &Self) -> bool {
        self.state() == other.state()
    }
}

impl ProjectileExt {
    pub fn state(&self) -> Option<&ProjectileState> {
        self.body.as_ref().map(|p| &*p.s).or(self.parked.as_deref())
    }
}

/// The Projectile's state (its parked copy while a callback of its own tick runs).
pub fn state(vm: &Vm, id: ObjId) -> Option<&ProjectileState> {
    ext::get(vm.objects(), id)?.projectile.as_deref()?.state()
}

// --------------------------------------------------------------------------------------------------------------- the datablock

fn float(vm: &Vm, db: ObjId, name: &[u8], default: f32) -> f32 {
    match value_named(vm, db, name) {
        Some(Value::Float(x)) => x,
        Some(Value::Int(n)) => n as f32,
        _ => default,
    }
}

fn int(vm: &Vm, db: ObjId, name: &[u8]) -> Option<i32> {
    match value_named(vm, db, name) {
        Some(Value::Int(n)) => Some(n),
        Some(Value::Float(x)) => Some(x as i32),
        _ => None,
    }
}

/// `ProjectileData` as the step reads it from datablock `db` (the class's defaults for what it does not set; the times
/// through their validators).
pub fn data_of(vm: &Vm, db: ObjId) -> ProjectileData {
    let mut d = ProjectileData {
        is_ballistic: matches!(value_named(vm, db, b"isBallistic"), Some(Value::Bool(true))),
        ..ProjectileData::default()
    };
    d.vel_inherit_factor = float(vm, db, b"velInheritFactor", d.vel_inherit_factor);
    d.muzzle_velocity = float(vm, db, b"muzzleVelocity", d.muzzle_velocity);
    d.bounce_elasticity = float(vm, db, b"bounceElasticity", d.bounce_elasticity);
    d.bounce_friction = float(vm, db, b"bounceFriction", d.bounce_friction);
    d.gravity_mod = float(vm, db, b"gravityMod", d.gravity_mod);
    if let Some(ms) = int(vm, db, b"lifetime") {
        d.set_lifetime_ms(ms);
    }
    if let Some(ms) = int(vm, db, b"armingDelay") {
        d.set_arming_delay_ms(ms);
    }
    if let Some(ms) = int(vm, db, b"fadeDelay") {
        d.set_fade_delay_ms(ms);
    }
    d
}

/// The ProjectileData of Projectile `id` now.
fn data(vm: &Vm, id: ObjId) -> Option<ProjectileData> {
    let db = object_field(vm, id, b"dataBlock")?;
    is_class(vm, db, "ProjectileData").then(|| data_of(vm, db))
}

fn point(vm: &Vm, id: ObjId, name: &[u8]) -> Option<F3> {
    match value_named(vm, id, name)? {
        Value::Floats3(p) => Some(p),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------------------------- placement

/// Writes the state's transform and world box into the VM's matrix and the scene (the copies), without the bins.
fn write_copies(vm: &mut Vm, id: ObjId, s: &ProjectileState) {
    crate::scene::access::set_matrix(vm, id, &s.scene.obj_to_world);
    if let Some(sc) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.scene.as_mut()) {
        sc.world_min = s.scene.world_box.min;
        sc.world_max = s.scene.world_box.max;
        sc.world_center = s.scene.world_center;
        sc.world_radius = s.scene.world_radius;
    }
}

fn translation(p: F3) -> Matrix {
    let mut m = IDENTITY;
    m[3] = p[0];
    m[7] = p[1];
    m[11] = p[2];
    m
}

// ------------------------------------------------------------------------------------------------------------------- the tick

/// A Projectile's entry in the process list.
pub struct ProjectileTicker {
    pub id: ObjId,
    pub env: Rc<SimEnv>,
}

impl Ticking<TickCx<'_>> for ProjectileTicker {
    fn class(&self) -> &'static str {
        "Projectile"
    }

    fn process_tick(&mut self, cx: &mut TickCx<'_>, _mv: Option<&Move>) {
        tick(cx.vm, &self.env, self.id);
    }
}

/// What a step asked of the object itself, carried out after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// Deleted at the end of the tick (its lifetime ran out).
    Now,
    /// Deleted at this Sim time (it exploded).
    At(u32),
}

/// One tick of Projectile `id` (`Projectile::processTick` through `::sim`).
pub fn tick(vm: &mut Vm, env: &SimEnv, id: ObjId) {
    if !vm.objects().contains(id) || !vm.charge_work(1) {
        return;
    }
    let Some(d) = data(vm, id) else {
        return;
    };
    let Some(mut p) = ext::get_mut(vm.objects_mut(), id)
        .and_then(|e| e.projectile.as_deref_mut())
        .and_then(|e| e.body.take())
    else {
        return;
    };
    p.s.mask_bits = 0;
    vm.hold(id);
    let mut host = HostImpl {
        vm,
        env,
        id,
        ending: None,
        error: None,
    };
    p.process_tick(&d, &mut host);
    let HostImpl { vm, ending, .. } = host;
    write_copies(vm, id, &p.s);
    scene::check_bins(vm, &env.container, id);
    let bits = std::mem::take(&mut p.s.mask_bits);
    if let Some(e) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.projectile.as_deref_mut()) {
        e.body = Some(p);
        e.parked = None;
    }
    if bits != 0 {
        ghosts::mark(vm, id, bits);
    }
    let _ = vm.release(id);
    match ending {
        Some(Ending::Now) => {
            let _ = vm.delete_object(id);
        }
        Some(Ending::At(at)) => {
            let wait = at.saturating_sub(vm.sim_time());
            let _ = vm.eval(format!("{id}.schedule({wait}, \"delete\");").as_bytes());
        }
        None => {}
    }
}

/// The host of one Projectile's step.
pub struct HostImpl<'a> {
    vm: &'a mut Vm,
    env: &'a SimEnv,
    id: ObjId,
    ending: Option<Ending>,
    /// The first error a callback's script or the container returned.
    pub error: Option<VmError>,
}

impl HostImpl<'_> {
    fn note(&mut self, r: Result<(), VmError>) {
        if let Err(e) = r
            && self.error.is_none()
        {
            self.error = Some(e);
        }
    }

    /// Runs script with the step's state where the object's readers find it (its copies written, its state parked).
    fn script(
        &mut self,
        s: &ProjectileState,
        f: impl FnOnce(&mut Vm, ObjId) -> Result<(), VmError>,
    ) {
        let id = self.id;
        write_copies(self.vm, id, s);
        scene::check_bins(self.vm, &self.env.container, id);
        if let Some(e) =
            ext::get_mut(self.vm.objects_mut(), id).and_then(|e| e.projectile.as_deref_mut())
        {
            e.parked = Some(Box::new(s.clone()));
        }
        let r = f(self.vm, id);
        self.note(r);
    }
}

fn text3(v: F3) -> Vec<u8> {
    v.iter()
        .map(|x| format_g(f64::from(*x)))
        .collect::<Vec<_>>()
        .join(" ")
        .into_bytes()
}

impl ProjectileHost for HostImpl<'_> {
    fn sim_time(&self) -> u32 {
        self.vm.sim_time()
    }

    fn type_masks(&self) -> &collide::TypeMasks {
        self.env.masks
    }

    fn cast_ray(&mut self, start: F3, end: F3, mask: u32) -> Option<::sim::RayHit> {
        if self.error.is_some() {
            return None;
        }
        match container_ray(self.vm, self.env, start, end, mask) {
            Ok(h) => h,
            Err(e) => {
                self.note(Err(e));
                None
            }
        }
    }

    fn set_source_collision(&mut self, source: ObjectId, disabled: bool) {
        if self.vm.objects().contains(source.0) {
            switch_collision(self.vm, source.0, disabled);
        }
    }

    fn on_collision(&mut self, s: &mut ProjectileState, col: ObjectId, point: F3, normal: F3) {
        let fade = s.fade;
        self.script(s, |vm, id| {
            call_db(
                vm,
                id,
                b"onCollision",
                &[
                    col.0.to_string().into_bytes(),
                    format_g(f64::from(fade)).into_bytes(),
                    text3(point),
                    text3(normal),
                ],
            )
        });
    }

    fn on_explode(&mut self, s: &mut ProjectileState, pos: F3, fade: f32) {
        self.script(s, |vm, id| {
            call_db(
                vm,
                id,
                b"onExplode",
                &[text3(pos), format_g(f64::from(fade)).into_bytes()],
            )
        });
    }

    fn schedule_delete(&mut self, _id: ObjectId, at: u32) {
        if self.ending.is_none() {
            self.ending = Some(Ending::At(at));
        }
    }

    fn delete_object(&mut self, _id: ObjectId) {
        self.ending = Some(Ending::Now);
    }
}

// ------------------------------------------------------------------------------------------------------------------- the class

/// `Projectile::onAdd` after `GameBase::onAdd` (see the module docs).
struct ProjectileClass {
    container: Rc<Container>,
}

impl EngineClass for ProjectileClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let Some(db) = object_field(vm, id, b"dataBlock") else {
            return Ok(false);
        };
        if !is_class(vm, db, "ProjectileData") {
            return Ok(false);
        }
        let position = point(vm, id, b"initialPosition").unwrap_or([0.0; 3]);
        let velocity = point(vm, id, b"initialVelocity").unwrap_or([0.0, 0.0, 1.0]);
        let source_id = int(vm, id, b"sourceObject").unwrap_or(-1);
        let source = u32::try_from(source_id)
            .ok()
            .filter(|s| *s != 0 && vm.objects().contains(*s));
        let obj_box = ext::get(vm.objects(), id)
            .and_then(|e| e.scene)
            .map(|s| Box3::new(s.object_min, s.object_max))
            .unwrap_or_default();
        let mut s = ProjectileState::new(ObjectId(id), obj_box);
        s.type_mask |= ext::get(vm.objects(), id).map_or(0, |e| e.type_mask);
        s.position = position;
        s.velocity = velocity;
        s.source_id = source.map_or(-1, |s| s as i32);
        s.source_slot = int(vm, id, b"sourceSlot").unwrap_or(-1);
        s.source = source.map(ObjectId);
        s.render_base = position;
        s.scene.set_matrix(&translation(position));
        let mut p = Projectile::new(s);
        p.on_add();
        if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
            e.type_mask = p.s.type_mask;
        }
        write_copies(vm, id, &p.s);
        if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
            e.projectile = Some(Box::new(ProjectileExt {
                body: Some(p),
                parked: None,
            }));
        }
        scene::add_to_scene(vm, &self.container, id);
        Ok(true)
    }
}

/// Gives Projectile its behaviour. After `shapebase::register`.
pub fn register(vm: &mut Vm, container: &Rc<Container>) {
    vm.set_class_behaviour(
        "Projectile",
        Rc::new(ProjectileClass {
            container: Rc::clone(container),
        }),
    );
}
