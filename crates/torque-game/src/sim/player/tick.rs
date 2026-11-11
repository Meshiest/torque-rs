//! The Player's tick on the host: [`PlayerTicker`] (the process list's adapter) and [`HostImpl`], the
//! `::sim::player::PlayerHost` over the `Vm`, the container, the skeleton and the script callbacks.
//!
//! **The borrow rule** (the `ContainerWorld` contract). The host holds the `Vm` in one of two forms: plain, or inside a
//! `ContainerWorld` built the first time the step asks for the world. Every script callback first takes the `Vm` back (the
//! world is dropped), *parks* the Player's state and shape in its ext so that the methods the script calls find them, runs the
//! script as a call charged like any other, then takes them back and refreshes the mount copy; the next world query builds a
//! new container.
//!
//! **Deletes.** The Player is held for its whole tick (an object's own delete waits until its tick returns); any other object a
//! script deletes is gone at once, and the step finds it gone (`World::object` answers `None`).
//!
//! **Callbacks** at the Player's call sites, each `%db.<name>(%obj, ...)` unless noted: `onTrigger` (through the tick's trigger
//! calls, so the network's limits apply), the touch callback a host's [`SimClasses`](crate::sim_classes::SimClasses) makes,
//! `doDismount`, `onEnterLiquid(%obj, coverage, type)`, `onLeaveLiquid(%obj, type)`, `onImpact(%obj, %hit, "%f %f %f", "%f")`,
//! `onCollision` both ways (`ShapeBase::onCollision`: the same vector for both), the `%obj.<name>()` a class names at the end
//! of its step, the AIPlayer's six, and the damage step's level update and `onDamage`. Not wired yet:
//! `Trigger::potentialEnterObject` (no Trigger class is ticked yet), an item's thrower, a vehicle mount or control object (a
//! Camera control object is ticked). The image slots ([`crate::sim::image`]): `image_tick` and the move's `setImageTrigger` run
//! the state machine with the state parked (its `stateScript`, `onMount` and `onUnmount` are script), and the AIPlayer's move
//! reads the trigger bytes.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/player.cpp` and
//! `T3D/shapeBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this
//! crate's own.

use std::cell::RefCell;
use std::rc::Rc;

use ::sim::Move;
use ::sim::Ticking;
use ::sim::player::host::{
    ControlledInfo, EnergyLink, LiquidStatics, MAX_LIQUID_DENSITY, MAX_LIQUID_VISCOSITY,
    SimGlobals, ZoneView, liquid_value,
};
use ::sim::player::{PlayerHost, PlayerState, Skeleton, eye_transform};
use collide::math::{Box3, F3, Matrix, Plane, PlaneTransformer, matrix_product, sqrt_f32};
use collide::{ObjectId, World};
use collide::{ObjectView, TypeMasks};
use ts_vm::fields::Value;
use ts_vm::sim::ObjId;
use ts_vm::text::{atof, format_fixed, format_g, to_f32};
use ts_vm::vm::{Vm, VmError};

use super::skeleton::HostSkeleton;
use super::{PlayerInfo, mirror_from, scene_box, state, sync_mount};
use crate::scene::access::{matrix_of, object_field, scale_of, value_named, value_of};
use crate::scene::camera::is_class;
use crate::scene::camera_tick::TriggerCalls;
use crate::scene::container_world::ContainerWorld;
use crate::scene::ghost as ghosts;
use crate::scene::{ext, shapebase};
use crate::shapes::Shapes;
use crate::sim::TickCx;

/// What every Player tick shares: the container, the shapes, and `updateContainer`'s statics.
pub struct SimEnv {
    /// The server container the movers' queries run through.
    pub container: Rc<crate::scene::container::Container>,
    pub shapes: Rc<Shapes>,
    pub statics: RefCell<LiquidStatics>,
    /// The type masks the movers query and test with (the host's, [`crate::masks`]).
    pub masks: &'static TypeMasks,
    /// The host's subclasses of the simulated classes ([`crate::sim_classes`]).
    pub classes: Rc<dyn crate::sim_classes::SimClasses>,
}

impl SimEnv {
    /// The environment of the movers over the `container`.
    pub fn new(
        container: Rc<crate::scene::container::Container>,
        shapes: Rc<Shapes>,
        masks: &'static TypeMasks,
        classes: Rc<dyn crate::sim_classes::SimClasses>,
    ) -> SimEnv {
        SimEnv {
            container,
            shapes,
            masks,
            classes,
            statics: RefCell::new(LiquidStatics::default()),
        }
    }
}

/// A Player's or AIPlayer's entry in the process list.
pub struct PlayerTicker {
    pub id: ObjId,
    pub env: Rc<SimEnv>,
    pub class: &'static str,
}

impl Ticking<TickCx<'_>> for PlayerTicker {
    fn class(&self) -> &'static str {
        self.class
    }

    fn process_tick(&mut self, cx: &mut TickCx<'_>, mv: Option<&Move>) {
        // The process-tick byte is clear while something controls the Player (a rider on a turret): its controller ticks
        // it with the rider's move ([`HostImpl::tick_controlled`]), not the list with its AI's.
        if mv.is_none() && crate::sim::mount::controlling_object(cx.vm, self.id).is_some() {
            return;
        }
        tick(cx.vm, cx.calls, &self.env, self.id, mv);
    }
}

/// `PlayerExt`'s pieces out for a tick.
fn take_body(vm: &mut Vm, id: ObjId) -> Option<(::sim::player::Player, Rc<PlayerInfo>)> {
    let p = ext::get_mut(vm.objects_mut(), id)?.player.as_deref_mut()?;
    let body = p.body.take()?;
    Some((body, Rc::clone(&p.info)))
}

/// How deep the ticks of control objects nest inside one another (a Player's control object ticked by the Player's tick, its
/// own control object by that tick, ...).
pub const CONTROL_DEPTH: usize = 8;

thread_local! {
    static CONTROL_NESTING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// One level of [`CONTROL_DEPTH`], given back when dropped.
struct ControlDepth;

impl ControlDepth {
    fn enter() -> Option<ControlDepth> {
        CONTROL_NESTING.with(|n| {
            (n.get() < CONTROL_DEPTH).then(|| {
                n.set(n.get() + 1);
                ControlDepth
            })
        })
    }
}

impl Drop for ControlDepth {
    fn drop(&mut self) {
        CONTROL_NESTING.with(|n| n.set(n.get().saturating_sub(1)));
    }
}

/// One tick of Player `id` (`Player::processTick` through `::sim`), with `mv` for a control object's move.
pub fn tick(vm: &mut Vm, calls: &mut dyn TriggerCalls, env: &SimEnv, id: ObjId, mv: Option<&Move>) {
    if !vm.objects().contains(id) {
        return;
    }
    super::refresh_info(vm, id);
    sync_mount(vm, id);
    let (scale, obj_box) = (scale_of(vm, id), scene_box(vm, id));
    let type_mask = ext::get(vm.objects(), id).map_or(0, |e| e.type_mask);
    // The controlling client's camera position (set by every client head: `readPacket`).
    let client = ghosts::controlling_client(vm, id).map(|client| {
        (
            client,
            ghosts::controlling_camera_pos(vm, id).unwrap_or(0.0),
        )
    });
    let Some((mut player, info)) = take_body(vm, id) else {
        return;
    };
    {
        let s = &mut player.s;
        if s.scale != scale || obj_box.is_some_and(|b| b != s.obj_box) {
            s.scale = scale;
            if let Some(b) = obj_box {
                s.obj_box = b;
            }
            info.class.update_scaled_box(s);
        }
        s.type_mask = type_mask;
        s.mask_bits = 0;
        // The controlling client as the step reads it: the camera position a head set; the class reads what else it needs.
        s.client = client.map(|(_, camera_pos)| ::sim::player::ClientView {
            freelook_value: camera_pos,
        });
        info.class
            .before_step(vm, id, client.map(|(client, _)| client), s);
    }
    let synced = crate::scene::hitboxes::wanted(vm, id);
    let shape = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take());
    vm.hold(id);
    let statics = *env.statics.borrow();
    let damage_before = player.s.damage;
    let mut host = HostImpl {
        slot: Slot::Vm(vm),
        env,
        calls,
        id,
        skel: HostSkeleton::new(shape, Rc::clone(&info)).synced(synced),
        statics,
        spare: None,
        error: None,
        moved: false,
        damage_before,
        tether: None,
    };
    let _ = player.process_tick(&info.data, mv, &mut host);
    // The class's end-of-step method, after the skeleton's advance (nothing after it in the step reads what the callback may
    // change).
    if let Some(name) = info.class.after_step(&mut player.s) {
        host.call_own_method(&mut player.s, name);
    }
    // A change of pose changes the box shots reach it through, and its end shrinks that back (the engine's poses): the scene's
    // copies follow it, not only a move.
    let s = &player.s;
    if host.moved
        || (s.pose_from != s.pose
            && s.pose_ticks <= ::sim::player::pose::moving_ticks(s.pose_from, s.pose))
    {
        host.flush_move(&player.s);
    }
    let HostImpl {
        slot,
        skel,
        statics,
        ..
    } = host;
    let vm = slot.into_vm();
    let _ = vm.charge_work(skel.work);
    *env.statics.borrow_mut() = statics;
    let bits = std::mem::take(&mut player.s.mask_bits);
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        // The object box the step left (the crouch switch rewrites max z) is the scene's from now on: the next
        // tick starts from it rather than taking the scene's old box back.
        if let Some(sc) = e.scene.as_mut() {
            sc.object_min = player.s.obj_box.min;
            sc.object_max = player.s.obj_box.max;
        }
        e.shape = skel.st;
        if let Some(p) = e.player.as_deref_mut() {
            p.body = Some(player);
            p.net_move = mv.is_some();
        }
    }
    if bits != 0 {
        ghosts::mark(vm, id, bits);
    }
    let _ = vm.release(id);
}

/// The `Vm` as the tick holds it.
enum Slot<'a> {
    Vm(&'a mut Vm),
    World(ContainerWorld<'a>),
    Moving,
}

impl<'a> Slot<'a> {
    fn into_vm(self) -> &'a mut Vm {
        match self {
            Slot::Vm(vm) => vm,
            Slot::World(w) => w.into_vm(),
            Slot::Moving => unreachable!("the slot is refilled before it is read"),
        }
    }
}

/// The host of one Player's tick.
pub struct HostImpl<'a> {
    slot: Slot<'a>,
    env: &'a SimEnv,
    calls: &'a mut dyn TriggerCalls,
    id: ObjId,
    skel: HostSkeleton,
    statics: LiquidStatics,
    /// The box the state swaps with while it is parked.
    spare: Option<Box<PlayerState>>,
    /// The first error a callback's script returned (the run's budget: the step goes on).
    pub error: Option<VmError>,
    /// `setPosition` moved the Player since its copies were last written.
    moved: bool,
    /// The damage before the step's damage update (`onDamage`'s difference).
    damage_before: f32,
    /// The tether sources of the script engine and the one whose tether the step took ([`crate::player_tether`]).
    tether: Option<(Rc<crate::player_tether::TetherSources>, usize)>,
}

impl<'a> HostImpl<'a> {
    fn vm(&mut self) -> &mut Vm {
        if matches!(self.slot, Slot::World(_)) {
            let w = std::mem::replace(&mut self.slot, Slot::Moving);
            self.slot = Slot::Vm(w.into_vm());
        }
        match &mut self.slot {
            Slot::Vm(vm) => vm,
            _ => unreachable!("made plain just above"),
        }
    }

    fn read<R>(&self, f: impl FnOnce(&Vm) -> R) -> R {
        match &self.slot {
            Slot::Vm(vm) => f(vm),
            Slot::World(w) => w.with_vm(f),
            Slot::Moving => unreachable!("the slot is refilled before it is read"),
        }
    }

    fn note(&mut self, r: Result<(), VmError>) {
        if let Err(e) = r
            && self.error.is_none()
        {
            self.error = Some(e);
        }
    }

    /// Writes the Player's copies (matrix, world box, bins) from `s`.
    fn flush_move(&mut self, s: &PlayerState) {
        let id = self.id;
        let container = Rc::clone(&self.env.container);
        let vm = self.vm();
        mirror_from(vm, id, s);
        crate::scene::object::check_bins(vm, &container, id);
        self.moved = false;
    }

    /// Runs script with the Player's state and shape parked where its methods find them (see the module docs).
    fn script(
        &mut self,
        s: &mut PlayerState,
        f: impl FnOnce(&mut Vm, &mut dyn TriggerCalls, ObjId) -> Result<(), VmError>,
    ) {
        if self.moved {
            self.flush_move(s);
        }
        let id = self.id;
        let mut parked = self.spare.take().unwrap_or_else(|| Box::new(s.clone()));
        std::mem::swap(&mut *parked, s);
        let shape = self.skel.st.take();
        self.vm();
        let Slot::Vm(vm) = &mut self.slot else {
            unreachable!("made plain just above");
        };
        let mut kept = None;
        match ext::get_mut(vm.objects_mut(), id) {
            Some(e) => {
                e.shape = shape;
                match e.player.as_deref_mut() {
                    Some(p) => p.parked = Some(parked),
                    None => kept = Some(parked),
                }
            }
            None => {
                self.skel.st = shape;
                kept = Some(parked);
            }
        }
        // `updateContainer`'s statics are globals: a tick the script runs (another Player's, nested) reads what this tick wrote
        // and this tick reads what it wrote (the copy was written back over the inner tick's at the end).
        *self.env.statics.borrow_mut() = self.statics;
        let r = f(vm, &mut *self.calls, id);
        self.statics = *self.env.statics.borrow();
        let back = kept.or_else(|| {
            ext::get_mut(vm.objects_mut(), id)
                .and_then(|e| e.player.as_deref_mut())
                .and_then(|p| p.parked.take())
        });
        if let Some(e) = ext::get_mut(vm.objects_mut(), id)
            && let Some(sh) = e.shape.take()
        {
            self.skel.st = Some(sh);
        }
        if let Some(mut b) = back {
            std::mem::swap(&mut *b, s);
            self.spare = Some(b);
        }
        let (on, node) = crate::sim::mount::link(vm, id);
        s.mount = on.map(ObjectId);
        s.mount_node = node;
        self.damage_before = s.damage;
        self.note(r);
    }
}

/// `%db.<name>(%obj, args...)` on the object's datablock.
fn call_db(vm: &mut Vm, id: ObjId, name: &[u8], args: &[Vec<u8>]) -> Result<(), VmError> {
    let Some(db) = object_field(vm, id, b"dataBlock") else {
        return Ok(());
    };
    let mut argv = vec![id.to_string().into_bytes()];
    argv.extend_from_slice(args);
    vm.call_method(db, name, &argv)?;
    Ok(())
}

/// `"%f %f %f"` and `"%f"` of the length `mSqrt(f32((x*x + y*y) + z*z))`, as `onImpact` and `onCollision` pass a vector.
fn vector_args(v: F3) -> [Vec<u8>; 2] {
    let text = v
        .iter()
        .map(|x| format_fixed(f64::from(*x), 6))
        .collect::<Vec<_>>()
        .join(" ");
    let len = sqrt_f32(collide::math::dot(v, v));
    [
        text.into_bytes(),
        format_fixed(f64::from(len), 6).into_bytes(),
    ]
}

fn type_mask(vm: &Vm, id: ObjId) -> u32 {
    ext::get(vm.objects(), id).map_or(0, |e| e.type_mask)
}

/// `ShapeBase::getMountTransform(node)` of any ShapeBase: the node's matrix with its translation scaled
/// by the object's scale, after the object's transform; the transform alone without that node.
pub fn mount_transform(vm: &Vm, id: ObjId, point: u32) -> Matrix {
    let object = super::state(vm, id).map_or_else(
        || matrix_of(vm, id).unwrap_or(collide::math::IDENTITY),
        |s| s.obj_to_world,
    );
    let node = (point < 32)
        .then(|| shapebase::state(vm, id))
        .flatten()
        .and_then(|st| {
            let body = st.body.as_ref()?;
            let n = body
                .kept
                .facts
                .as_ref()?
                .mount_nodes
                .get(point as usize)
                .copied()?;
            body.instance
                .node_transform(usize::try_from(n).ok()?)
                .copied()
        });
    let Some(mut n) = node else {
        return object;
    };
    let s = scale_of(vm, id);
    n[3] *= s[0];
    n[7] *= s[1];
    n[11] *= s[2];
    matrix_product(&object, &n)
}

/// A Player's eye transform (`getEyeTransform`, its nodes animated), through a skeleton over its own shape state.
pub fn eye_of(vm: &mut Vm, id: ObjId) -> Option<Matrix> {
    let info = super::info(vm, id)?;
    let shape = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take());
    let mut skel = HostSkeleton::new(shape, info);
    let m = super::state(vm, id).map(|s| eye_transform(s, &mut skel));
    let _ = vm.charge_work(skel.work);
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.shape = skel.st;
    }
    m
}

/// A Player's steady eye (engine addition, `getSteadyEyePoint`): `getEyeTransform` with the eye node at the shape's rest pose
/// (no thread played) instead of as the walk or run animates it, so it does not bob. A client with a steady first-person view
/// (`torque::app::ghosts::GhostOptions::steady_eye`) draws its view and crosshair from this point: a shot fired from it goes
/// where the crosshair is. Crouched or prone (the engine's poses, [`super::poses`]) the eye is the one of the pose's root
/// (`crouch_root`, `prone_root`) held at its start, as that client lowers its view, eased from the last pose's over the
/// body's blend ([`super::poses::pose_blend`]) the same way. The server's own state alone; nothing a client sends. `None` for
/// an object that is not a Player or whose shape has no eye node.
pub fn steady_eye_of(vm: &mut Vm, id: ObjId) -> Option<Matrix> {
    let s = super::state(vm, id)?;
    let (obj_to_world, head, scale) = (s.obj_to_world, s.head, s.scale);
    let (pose, from, ticks) = (s.pose, s.pose_from, s.pose_ticks);
    let body = ext::get(vm.objects(), id)?.shape.as_ref()?.body.as_ref()?;
    let kept = Rc::clone(&body.kept);
    let shape = kept.shape.as_ref()?;
    let eye = usize::try_from(kept.facts.as_ref()?.eye_node).ok()?;
    let mut work = 0u64;
    let mut eye_in = |pose: super::poses::Pose| -> Option<F3> {
        let mut charge = |n: u64| {
            work += n;
            true
        };
        let mut instance = shapes::instance::ShapeInstance::new(shape, &mut charge).ok()?;
        let root = pose.action_prefix().map(|p| format!("{p}_root"));
        if let Some(seq) = root.and_then(|n| shapes::instance::find_sequence(shape, n.as_bytes()))
            && let Some(t) = instance.create_thread(shape)
        {
            instance.set_sequence(shape, t, seq as i32, 0.0);
        }
        let dl =
            (shape.smallest_visible_dl.max(0) as usize).min(shape.details.len().saturating_sub(1));
        instance.animate(shape, dl, None, &mut charge).ok()?;
        let n = instance.node_transform(eye)?;
        Some([n[3], n[7], n[11]])
    };
    let mut p = eye_in(pose)?;
    if let Some(seconds) = super::poses::pose_blend(from, pose)
        && seconds > 0.0
        && let Some(old) = eye_in(from)
    {
        // The eye eases from the last pose's to this one's (smoothstep), as the client's view does.
        let u = (ticks as f32 * ::sim::player::DT / seconds).clamp(0.0, 1.0);
        let w = 1.0 - u * u * (3.0 - 2.0 * u);
        p = std::array::from_fn(|k| p[k] + (old[k] - p[k]) * w);
    }
    let _ = vm.charge_work(work);
    let mut t = matrix_product(
        &collide::math::set_euler([0.0, 0.0, head[2]]),
        &collide::math::set_euler([head[0], 0.0, 0.0]),
    );
    t[3] = p[0] * scale[0];
    t[7] = p[1] * scale[1];
    t[11] = p[2] * scale[2];
    Some(matrix_product(&obj_to_world, &t))
}

/// A zone's view (the object box's six planes moved to the world: a polyhedron that is not a box is taken as its bounds,
/// listed).
fn zone_view(vm: &Vm, id: ObjId) -> Option<ZoneView> {
    let e = ext::get(vm.objects(), id)?;
    let sc = e.scene?;
    let f = |name: &[u8], default: f32| match value_named(vm, id, name) {
        Some(Value::Float(x)) => x,
        _ => default,
    };
    let f3 = |name: &[u8]| match value_named(vm, id, name) {
        Some(Value::Floats3(v)) => v,
        _ => [0.0; 3],
    };
    let color = match value_of(vm, id, b"waterColor") {
        Some(Value::ColorF(c)) => c,
        _ => [0.0; 4],
    };
    let m = matrix_of(vm, id)?;
    let pt = PlaneTransformer::new(&m, scale_of(vm, id));
    let (lo, hi) = (sc.object_min, sc.object_max);
    let planes = [
        Plane::new(-1.0, 0.0, 0.0, lo[0]),
        Plane::new(1.0, 0.0, 0.0, -hi[0]),
        Plane::new(0.0, -1.0, 0.0, lo[1]),
        Plane::new(0.0, 1.0, 0.0, -hi[1]),
        Plane::new(0.0, 0.0, -1.0, lo[2]),
        Plane::new(0.0, 0.0, 1.0, -hi[2]),
    ]
    .iter()
    .map(|p| pt.transform(p))
    .collect();
    Some(ZoneView {
        id: ObjectId(id),
        active: true,
        world_box: Box3::new(sc.world_min, sc.world_max),
        planes,
        enabled: !e.zone_inactive,
        velocity_mod: f(b"velocityMod", 1.0),
        gravity_mod: f(b"gravityMod", 1.0),
        drag: f(b"extraDrag", 1.0),
        applied_force: f3(b"appliedForce"),
        water: matches!(value_of(vm, id, b"isWater"), Some(Value::Bool(true))),
        viscosity: liquid_value(f(b"waterViscosity", 40.0), MAX_LIQUID_VISCOSITY),
        density: liquid_value(f(b"waterDensity", 1.0), MAX_LIQUID_DENSITY),
        color,
        liquid_type: match value_of(vm, id, b"waterType") {
            Some(Value::Int(n)) => n as u32,
            _ => 0,
        },
    })
}

impl HostImpl<'_> {
    /// `%obj.<name>()`: the method the Player's class names at the end of its step ([`PlayerKind::after_step`]).
    ///
    /// [`PlayerKind::after_step`]: crate::sim_classes::PlayerKind::after_step
    fn call_own_method(&mut self, s: &mut PlayerState, name: &'static [u8]) {
        self.script(s, |vm, _, id| vm.call_method(id, name, &[]).map(|_| ()));
    }
}

impl crate::sim_classes::VmAccess for HostImpl<'_> {
    fn read_vm(&self, f: &mut dyn FnMut(&Vm)) {
        self.read(|vm| f(vm));
    }

    fn write_vm(&mut self, f: &mut dyn FnMut(&mut Vm)) {
        f(self.vm());
    }
}

impl<'a> PlayerHost for HostImpl<'a> {
    fn player_class(&self) -> Option<Rc<dyn ::sim::player::PlayerClass>> {
        Some(self.skel.info.class.class())
    }

    /// The first tether a plugin's source puts on this Player ([`crate::player_tether`]).
    fn tether(&mut self, _s: &PlayerState) -> Option<::sim::player::tether::Tether> {
        let sources = self.read(crate::player_tether::of)?;
        let (index, tether) = sources.tether(self.id)?;
        self.tether = Some((sources, index));
        Some(tether)
    }

    fn tethered(&mut self, s: &PlayerState, change: F3) {
        if let Some((sources, index)) = self.tether.take() {
            sources.pulled(index, self.id, change, s.mass);
        }
    }

    fn world(&mut self) -> &mut dyn World {
        if matches!(self.slot, Slot::Vm(_)) {
            let Slot::Vm(vm) = std::mem::replace(&mut self.slot, Slot::Moving) else {
                unreachable!("tested just above");
            };
            self.slot = Slot::World(ContainerWorld::new(
                vm,
                &self.env.container,
                &self.env.shapes,
            ));
        }
        match &mut self.slot {
            Slot::World(w) => w,
            _ => unreachable!("made a world just above"),
        }
    }

    fn skeleton(&mut self) -> &mut dyn Skeleton {
        &mut self.skel
    }

    fn sim_time(&self) -> u32 {
        self.read(|vm| vm.sim_time())
    }

    fn type_masks(&self) -> &TypeMasks {
        self.env.masks
    }

    fn globals(&self) -> SimGlobals {
        self.read(|vm| {
            let g = |name: &str| vm.global(name).map_or(0.0, |v| to_f32(atof(&v)));
            SimGlobals {
                damage_flash_dec: g("$SB::DFDec"),
                white_out_dec: g("$SB::WODec"),
            }
        })
    }

    fn object_velocity(&mut self, id: ObjectId) -> F3 {
        self.read(|vm| state(vm, id.0).map_or([0.0; 3], |s| s.velocity))
    }

    fn mount_transform(&mut self, mount: ObjectId, node: u32) -> Option<Matrix> {
        self.read(|vm| {
            vm.objects()
                .contains(mount.0)
                .then(|| mount_transform(vm, mount.0, node))
        })
    }

    fn energy_link(&mut self, id: ObjectId) -> Option<EnergyLink> {
        self.read(|vm| {
            vm.objects().contains(id.0).then(|| EnergyLink {
                energy: crate::sim::vitals::get(vm, id.0).energy,
                inherit: crate::sim::vitals::limits(vm, id.0).inherit,
                mount: crate::sim::mount::mount_of(vm, id.0).map(ObjectId),
            })
        })
    }

    /// `findContact`'s touch: the host's ([`crate::sim_classes::SimClasses::contact_touch`]).
    fn contact_touch(
        &mut self,
        s: &PlayerState,
        obj: ObjectId,
        view: &ObjectView,
        query: &Box3,
        queue_full: bool,
    ) -> bool {
        let classes = Rc::clone(&self.env.classes);
        classes.contact_touch(self, s, obj, view, query, queue_full)
    }

    fn zones(&mut self, query: &Box3, out: &mut Vec<ZoneView>) {
        let me = self.id;
        let mut ids = Vec::new();
        if self
            .world()
            .find_objects(
                query,
                collide::types::PHYSICAL_ZONE | collide::types::WATER,
                Some(ObjectId(me)),
                256,
                &mut ids,
            )
            .is_err()
        {
            return;
        }
        self.read(|vm| {
            for id in ids {
                // A water block's water, as its host made it ([`ext::MapObject::water`]).
                if let Some(z) = ext::map_of(vm, id.0).and_then(|m| m.water.as_ref()) {
                    out.push(ZoneView { id, ..z.clone() });
                } else if is_class(vm, id.0, "PhysicalZone")
                    && let Some(z) = zone_view(vm, id.0)
                {
                    out.push(z);
                }
            }
        });
    }

    fn liquid_statics(&mut self) -> &mut LiquidStatics {
        &mut self.statics
    }

    fn zone_velocity_mod(&mut self, id: ObjectId) -> Option<f32> {
        self.read(|vm| {
            let e = ext::get(vm.objects(), id.0)?;
            if e.zone_inactive || !is_class(vm, id.0, "PhysicalZone") {
                return None;
            }
            match value_of(vm, id.0, b"velocityMod") {
                Some(Value::Float(x)) => Some(x),
                _ => Some(1.0),
            }
        })
    }

    fn controlled_info(&mut self, id: ObjectId) -> ControlledInfo {
        self.read(|vm| {
            let m = type_mask(vm, id.0);
            ControlledInfo {
                vehicle: m & 0x1_0000 != 0,
                player: m & 0x4000 != 0,
            }
        })
    }

    fn cast_ray(&mut self, start: F3, end: F3, mask: u32) -> Option<F3> {
        let me = self.id;
        let container = Rc::clone(&self.env.container);
        let shapes = Rc::clone(&self.env.shapes);
        let vm = self.vm();
        // This Player and its mounts are left out (their collision-disable counts raised for the cast).
        let mut out = vec![me];
        let mut at = me;
        while let Some(m) = crate::sim::mount::mount_of(vm, at) {
            // The repeat test walks the list: charged as it goes (a refusal stops the walk; the cast then finds the budget
            // spent).
            if !vm.charge_work(out.len() as u64)
                || out.contains(&m)
                || out.len() > crate::sim::mount::SIM_MOUNT_CHAIN
            {
                break;
            }
            out.push(m);
            at = m;
        }
        for o in &out {
            if let Some(e) = ext::get_mut(vm.objects_mut(), *o) {
                e.collision_off = e.collision_off.saturating_add(1);
            }
        }
        let hit =
            crate::scene::container::container_ray(vm, &container, Some(&shapes), start, end, mask);
        for o in &out {
            if let Some(e) = ext::get_mut(vm.objects_mut(), *o) {
                e.collision_off = e.collision_off.saturating_sub(1);
            }
        }
        match hit {
            Ok(h) => h.map(|h| h.point),
            Err(e) => {
                self.note(Err(e));
                None
            }
        }
    }

    /// The host's Player subclass's questions ([`crate::sim_classes::SimClasses::player_query`]).
    fn class_query(&mut self, query: &mut dyn std::any::Any) {
        let classes = Rc::clone(&self.env.classes);
        let me = self.id;
        classes.player_query(self, me, query);
    }

    fn image_trigger(&mut self, _s: &PlayerState, slot: usize) -> bool {
        let id = self.id;
        self.read(|vm| crate::sim::image::trigger(vm, id, slot))
    }

    fn advance_threads(&mut self, s: &mut PlayerState) {
        // The step holds the Player's shape state (the skeleton's); a slot that ends calls its script with the state put back.
        for slot in 0..crate::scene::shapebase::SCRIPT_THREADS {
            let ended = self.skel.st.as_deref_mut().is_some_and(|st| {
                crate::scene::shapebase::advance_thread_slot(
                    st,
                    slot,
                    crate::scene::shapebase::THREAD_TICK_SEC,
                )
            });
            if ended {
                self.script(s, |vm, _, id| {
                    crate::scene::shapebase::on_end_sequence(vm, id, slot)
                });
            }
        }
    }

    fn fade_tick(&mut self, s: &mut PlayerState) {
        // The step holds the Player's shape state (the skeleton's), the fade and the sound slots with it.
        let now = self.sim_time();
        let Some(st) = self.skel.st.as_deref_mut() else {
            return;
        };
        if crate::scene::shapebase::fade_step(&mut st.fade) {
            s.mask_bits |= crate::scene::shapebase::FADE_MASK;
        }
        let cleared = crate::scene::shapebase::sound_timeout_step(&mut st.sounds, now);
        if cleared != 0 {
            s.mask_bits &= !cleared;
            let id = self.id;
            ghosts::unmark(self.vm(), id, cleared);
        }
    }

    fn image_tick(&mut self, s: &mut PlayerState, slot: usize) {
        let id = self.id;
        if slot == 0 {
            let wet = s.image_wet;
            crate::sim::image::load_wet0(self.vm(), id, wet);
        }
        // The slot's datablock is tested as its turn comes; an empty slot costs no parking.
        if self
            .read(|vm| crate::sim::image::slot_db(vm, id, slot))
            .is_none()
        {
            return;
        }
        let env = self.env;
        self.script(s, |vm, _, id| {
            crate::sim::image::tick_slot(vm, env, id, slot)
        });
        s.images = self.read(|vm| crate::sim::image::occupied(vm, id));
    }

    fn set_image_trigger(&mut self, s: &mut PlayerState, slot: usize, down: bool) {
        let id = self.id;
        // `setImageTriggerState` does nothing for an empty slot or an unchanged trigger.
        let changes = self.read(|vm| {
            crate::sim::image::slot_db(vm, id, slot).is_some()
                && crate::sim::image::trigger(vm, id, slot) != down
        });
        if !changes {
            return;
        }
        let env = self.env;
        self.script(s, |vm, _, id| {
            crate::sim::image::set_trigger(vm, env, id, slot, down)
        });
        s.images = self.read(|vm| crate::sim::image::occupied(vm, id));
    }

    fn damage_changed(&mut self, s: &mut PlayerState) {
        let (old, new) = (self.damage_before, s.damage);
        self.script(s, |vm, _, id| {
            crate::sim::vitals::update_damage_level(vm, id)?;
            ghosts::mark(vm, id, 0x20);
            let delta = (crate::library::get().difference)(new, old);
            call_db(vm, id, b"onDamage", &[format_fixed(delta, 6).into_bytes()])
        });
    }

    fn on_trigger(&mut self, s: &mut PlayerState, slot: usize, down: bool) {
        self.script(s, |vm, calls, id| {
            match object_field(vm, id, b"dataBlock") {
                Some(db) => calls.on_trigger(vm, db, id, slot, down),
                None => Ok(()),
            }
        });
    }

    fn on_contact_touch(&mut self, s: &mut PlayerState, obj: ObjectId) {
        self.script(s, |vm, _, id| {
            if vm.objects().contains(obj.0) {
                crate::sim_classes::sim_classes(vm).touched(vm, id, obj.0)?;
            }
            Ok(())
        });
    }

    fn ai_callback(&mut self, s: &mut PlayerState, name: &'static str) {
        self.script(s, |vm, _, id| call_db(vm, id, name.as_bytes(), &[]));
    }

    fn do_dismount(&mut self, s: &mut PlayerState) {
        self.script(s, |vm, _, id| call_db(vm, id, b"doDismount", &[]));
    }

    fn on_enter_liquid(&mut self, s: &mut PlayerState, coverage: f32, liquid_type: u32) {
        self.script(s, |vm, _, id| {
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

    fn on_leave_liquid(&mut self, s: &mut PlayerState, liquid_type: u32) {
        self.script(s, |vm, _, id| {
            call_db(
                vm,
                id,
                b"onLeaveLiquid",
                &[liquid_type.to_string().into_bytes()],
            )
        });
    }

    fn on_impact(&mut self, s: &mut PlayerState, hit: Option<ObjectId>, vec: F3) {
        self.script(s, |vm, _, id| {
            let [v, len] = vector_args(vec);
            let hit = hit.map_or(0, |h| h.0);
            call_db(vm, id, b"onImpact", &[hit.to_string().into_bytes(), v, len])
        });
    }

    fn on_collision(&mut self, s: &mut PlayerState, other: ObjectId, vec: F3) -> bool {
        let mut alive = true;
        self.script(s, |vm, _, id| {
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

    fn tick_controlled(&mut self, s: &mut PlayerState, control: ObjectId, mv: Option<&Move>) {
        let mv = mv.copied();
        // A control chain the scripts made (a Player controlling a Player controlling ...) or a cycle of them: the nested ticks
        // stop [`CONTROL_DEPTH`] deep.
        let Some(_depth) = ControlDepth::enter() else {
            return;
        };
        self.script(s, |vm, calls, _| {
            if crate::sim::vehicle::is_vehicle(vm, control.0) {
                return crate::sim::vehicle_tick::tick_controlled(
                    vm,
                    calls,
                    control.0,
                    mv.as_ref(),
                );
            }
            // A Player or AIPlayer control object (a turret a rider controls): one tick of it with the filtered move, as for a
            // vehicle (`Player::processTick` calls every control object's processTick).
            if let Some(env) = ext::get(vm.objects(), control.0)
                .and_then(|e| e.player.as_ref())
                .and_then(|_| crate::sim::env_of(vm, control.0))
            {
                tick(vm, calls, &env, control.0, mv.as_ref());
                return Ok(());
            }
            if !crate::scene::camera::is_camera(vm, control.0) {
                return Ok(());
            }
            let class = crate::scene::camera::camera_class(vm);
            match mv {
                Some(m) => {
                    class.process_tick(vm, control.0, &crate::sim::adapters::net_move(&m), calls)
                }
                None => class.process_tick_idle(vm, control.0),
            }
        });
    }

    fn moved(&mut self, _s: &mut PlayerState) {
        // The copies are written before the next script and at the end of the tick (nothing else reads them in between: the
        // step's own queries leave this Player out).
        self.moved = true;
    }
}
