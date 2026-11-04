//! The Player and AIPlayer host glue: the objects, their datablock products, the tick ([`tick`]), the skeleton over the
//! shape instance ([`skeleton`]) and the console methods ([`methods`]). The step itself is `::sim::player`.
//!
//! **Where the state lives.** `ObjectExt::player` holds a [`PlayerExt`]: the `::sim` Player (its state, its working
//! set) and the datablock products it steps with. The `PlayerState` is the truth for every field the step reads, the transform
//! included; the VM's `position` matrix and the scene's world box and sphere are copies written at every placement
//! ([`mirror`]), from the state's own `resetWorldBox` values, then the bins are checked. While the Player's own tick runs, its
//! `Player` is out of the ext and, around each script callback, its state is *parked* in [`PlayerExt::parked`]:
//! [`state`]/[`state_mut`] find it in either place, so a script that the step calls can use every method on the same Player.
//!
//! **The datablock.** [`PlayerInfo`] is built from the `PlayerData`'s fields (the class table's defaults for the unset ones),
//! the action list of its shape with each action's motion ([`ground`]) and `PlayerData::preload`'s products. Torque reads the
//! datablock's members live; the engine rebuilds the info, preload products included, on the tick after any static-field write
//! to that datablock ([`datablock_written`], the `onStaticModified` hook point): a script that changes `maxForwardSpeed`
//! mid-game is obeyed from the next tick (the preload products such as `runSurfaceCos` are rebuilt too).
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `T3D/player.cpp` and
//! `T3D/aiPlayer.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this
//! crate's own.

pub mod ground;
pub mod methods;
pub mod skeleton;
pub mod synced;
pub mod tick;

use std::rc::Rc;

use ::sim::player::{Player, PlayerData, PlayerState};
use collide::math::{Box3, F3};
use ts_vm::classes::EngineClass;
use ts_vm::fields::Value;
use ts_vm::sim::ClassId;
use ts_vm::sim::ObjId;
use ts_vm::vm::{Vm, VmError};

use crate::scene::access::{matrix_of, object_field, scale_of, value_named};
use crate::scene::camera::is_class;
use crate::scene::container::Container;
use crate::scene::ext;
use crate::scene::ghost as ghosts;
use crate::scene::object as scene;
use crate::scene::shapebase::{self, ActionRequest, is_player};
use ground::ActionMotion;

/// What a PlayerData gives the step: the `::sim` datablock (fields and preload products), the class's action table, each
/// action's motion, the `look` action and `maxTimeScale`.
#[derive(Debug, Clone)]
pub struct PlayerInfo {
    pub data: PlayerData,
    /// The Player class the datablock makes (the host's subclass, [`crate::sim_classes`]).
    pub class: Rc<dyn crate::sim_classes::PlayerKind>,
    pub actions: &'static shapebase::ActionTable,
    pub motions: Vec<ActionMotion>,
    pub look_action: u32,
    pub max_time_scale: f32,
    /// `maxFreelookAngle` as the ghost record divides by it.
    pub max_freelook: f32,
}

impl PartialEq for PlayerInfo {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
            && format!("{:?}", self.class) == format!("{:?}", other.class)
            && self.actions == other.actions
            && self.motions == other.motions
            && self.look_action == other.look_action
            && self.max_time_scale == other.max_time_scale
            && self.max_freelook == other.max_freelook
    }
}

/// A Player's host state.
#[derive(Debug, Clone)]
pub struct PlayerExt {
    /// The step's Player; `None` while its tick runs.
    pub body: Option<Player>,
    /// The state while a callback of its own tick runs script.
    pub parked: Option<Box<PlayerState>>,
    pub info: Rc<PlayerInfo>,
    /// The datablock the info was built from and its write count then.
    pub db: ObjId,
    pub generation: u32,
    /// The last tick ran a connection's move (its raw values are the floats' inverse); otherwise the last move is a `NullMove`
    /// with the AI's floats and triggers (raw axes 16, angles 0), as the ghost record sends it.
    pub net_move: bool,
}

impl PartialEq for PlayerExt {
    fn eq(&self, other: &Self) -> bool {
        self.db == other.db
            && self.info == other.info
            && self.body.as_ref().map(|p| &p.s) == other.body.as_ref().map(|p| &p.s)
            && self.parked == other.parked
    }
}

impl PlayerExt {
    pub fn state(&self) -> Option<&PlayerState> {
        self.body.as_ref().map(|p| &*p.s).or(self.parked.as_deref())
    }

    pub fn state_mut(&mut self) -> Option<&mut PlayerState> {
        match self.body.as_mut() {
            Some(p) => Some(&mut *p.s),
            None => self.parked.as_deref_mut(),
        }
    }
}

/// The Player's state (its own tick's parked copy while a callback runs).
pub fn state(vm: &Vm, id: ObjId) -> Option<&PlayerState> {
    ext::get(vm.objects(), id)?.player.as_deref()?.state()
}

pub fn state_mut(vm: &mut Vm, id: ObjId) -> Option<&mut PlayerState> {
    ext::get_mut(vm.objects_mut(), id)?
        .player
        .as_deref_mut()?
        .state_mut()
}

/// The datablock products the Player steps with.
pub fn info(vm: &Vm, id: ObjId) -> Option<Rc<PlayerInfo>> {
    Some(Rc::clone(
        &ext::get(vm.objects(), id)?.player.as_deref()?.info,
    ))
}

// --------------------------------------------------------------------------------------------------------------- the datablock

pub fn float(vm: &Vm, db: ObjId, name: &[u8], default: f32) -> f32 {
    match value_named(vm, db, name) {
        Some(Value::Float(x)) => x,
        Some(Value::Int(n)) => n as f32,
        _ => default,
    }
}

fn int(vm: &Vm, db: ObjId, name: &[u8], default: i32) -> i32 {
    match value_named(vm, db, name) {
        Some(Value::Int(n)) => n,
        Some(Value::Float(x)) => collide::math::float_to_int(f64::from(x)),
        _ => default,
    }
}

pub fn boolean(vm: &Vm, db: ObjId, name: &[u8], default: bool) -> bool {
    match value_named(vm, db, name) {
        Some(Value::Bool(b)) => b,
        Some(Value::Int(n)) => n != 0,
        _ => default,
    }
}

pub fn floats3(vm: &Vm, db: ObjId, name: &[u8], default: [f32; 3]) -> [f32; 3] {
    match value_named(vm, db, name) {
        Some(Value::Floats3(v)) => v,
        _ => default,
    }
}

/// The datablock's shape, when the engine holds its body.
pub fn shape_of(vm: &Vm, db: ObjId) -> Option<Rc<crate::shapes::KeptShape>> {
    ext::get(vm.objects(), db)?
        .shape_resource
        .as_ref()
        .map(crate::shapes::ShapeResource::current)
        .filter(|k| k.shape.is_some())
}

/// The info of PlayerData `db` (see the module docs).
pub fn build_info(vm: &Vm, db: ObjId) -> Option<PlayerInfo> {
    let charge = &mut |units: u64| vm.charge_work(units);
    let mut d = PlayerData::default();
    let classes = crate::sim_classes::sim_classes(vm);
    let kind = classes.player_kind(vm, db);
    let table = classes.action_table();
    d.actions = table.numbers();
    macro_rules! f {
        ($field:ident, $name:literal) => {
            d.$field = float(vm, db, $name, d.$field);
        };
    }
    f!(mass, b"mass");
    f!(drag, b"drag");
    f!(density, b"density");
    f!(max_energy, b"maxEnergy");
    f!(max_damage, b"maxDamage");
    f!(repair_rate, b"repairRate");
    d.is_invincible = boolean(vm, db, b"isInvincible", d.is_invincible);
    d.inherit_energy = boolean(vm, db, b"inheritEnergyFromMount", d.inherit_energy);
    f!(min_look_angle, b"minLookAngle");
    f!(max_look_angle, b"maxLookAngle");
    f!(max_freelook_angle, b"maxFreelookAngle");
    f!(run_force, b"runForce");
    f!(run_energy_drain, b"runEnergyDrain");
    f!(min_run_energy, b"minRunEnergy");
    f!(max_forward_speed, b"maxForwardSpeed");
    f!(max_backward_speed, b"maxBackwardSpeed");
    f!(max_side_speed, b"maxSideSpeed");
    f!(max_underwater_forward_speed, b"maxUnderwaterForwardSpeed");
    f!(max_underwater_backward_speed, b"maxUnderwaterBackwardSpeed");
    f!(max_underwater_side_speed, b"maxUnderwaterSideSpeed");
    f!(max_step_height, b"maxStepHeight");
    f!(run_surface_angle, b"runSurfaceAngle");
    f!(horiz_max_speed, b"horizMaxSpeed");
    f!(horiz_resist_speed, b"horizResistSpeed");
    f!(horiz_resist_factor, b"horizResistFactor");
    f!(up_max_speed, b"upMaxSpeed");
    f!(up_resist_speed, b"upResistSpeed");
    f!(up_resist_factor, b"upResistFactor");
    f!(jump_force, b"jumpForce");
    f!(jump_energy_drain, b"jumpEnergyDrain");
    f!(min_jump_energy, b"minJumpEnergy");
    f!(min_jump_speed, b"minJumpSpeed");
    f!(max_jump_speed, b"maxJumpSpeed");
    f!(jump_surface_angle, b"jumpSurfaceAngle");
    d.jump_delay = int(vm, db, b"jumpDelay", d.jump_delay);
    f!(min_impact_speed, b"minImpactSpeed");
    f!(ground_impact_min_speed, b"groundImpactMinSpeed");
    d.bounding_box = floats3(vm, db, b"boundingBox", d.bounding_box);
    f!(pickup_radius, b"pickupRadius");
    let max_time_scale = float(vm, db, b"maxTimeScale", 1.5);
    let mut motions = Vec::new();
    let mut look_action = 0;
    if let Some(kept) = shape_of(vm, db)
        && let Some(shape) = kept.shape.as_ref()
    {
        // The action list walks every sequence of the shape (and keeps at most the table's size).
        let walk = (shape.sequences.len() as u64).saturating_mul(table.max as u64);
        if !charge(walk) {
            return None;
        }
        let actions = shapebase::player_actions(shape, table);
        for (i, a) in actions.iter().enumerate() {
            let fixed = i < table.fixed.len();
            let saved = table.dirs.get(i).copied().unwrap_or([0.0; 3]);
            let motion = match usize::try_from(a.sequence) {
                Ok(seq) => ground::ground_info(shape, &a.name, seq, saved, fixed, charge)?,
                Err(_) => ActionMotion {
                    dir: saved,
                    speed: 0.0,
                    velocity_scale: fixed,
                    death: false,
                },
            };
            motions.push(motion);
            if a.name == b"look" {
                look_action = u32::try_from(i).unwrap_or(0);
            }
        }
        d.action_count = u32::try_from(actions.len()).unwrap_or(0);
        d.action_list = motions
            .iter()
            .map(|m| ::sim::player::data::ActionAnimation { death: m.death })
            .collect();
    }
    classes.player_preload(&mut d);
    Some(PlayerInfo {
        max_freelook: d.max_freelook_angle,
        data: d,
        class: kind,
        actions: table,
        motions,
        look_action,
        max_time_scale,
    })
}

thread_local! {
    /// How many times a `PlayerInfo` was built on this thread (the tests read it).
    static REBUILDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The number of `PlayerInfo` builds so far on this thread.
pub fn rebuilds() -> u64 {
    REBUILDS.with(std::cell::Cell::get)
}

/// The products of PlayerData `db` at its current write count: the datablock's cached ones, or built once (charged) and cached
/// there, so every Player of the datablock shares one build per write. `None` when the budget refused the build.
pub fn info_of(vm: &mut Vm, db: ObjId) -> Option<Rc<PlayerInfo>> {
    let count = generation(vm, db);
    if let Some((c, info)) = ext::get(vm.objects(), db).and_then(|e| e.player_info.as_ref())
        && *c == count
    {
        return Some(Rc::clone(info));
    }
    let info = Rc::new(build_info(vm, db)?);
    REBUILDS.with(|r| r.set(r.get().saturating_add(1)));
    if let Some(e) = ext::get_mut(vm.objects_mut(), db) {
        e.player_info = Some((count, Rc::clone(&info)));
    }
    Some(info)
}

/// A datablock's static-field write count (the `onStaticModified` hook point bumps it).
pub fn generation(vm: &Vm, db: ObjId) -> u32 {
    ext::get(vm.objects(), db).map_or(0, |e| e.db_generation)
}

/// The observer's part: a write to a registered `PlayerData` bumps its count, so its Players rebuild their info on their next
/// tick (and a write to a `ShapeBaseImageData` its view for the image step, [`crate::sim::image`]).
pub fn datablock_written(vm: &mut Vm, id: Option<ObjId>, class: ClassId) {
    let Some(id) = id else {
        return;
    };
    let classes = &vm.objects().classes;
    // A PlayerData's Players rebuild their products; a ShapeBaseImageData's state-machine view is rebuilt (its live members).
    let watched = [&b"PlayerData"[..], b"ShapeBaseImageData"]
        .iter()
        .any(|name| classes.find(name).is_some_and(|c| classes.is_a(class, c)));
    if !watched {
        return;
    }
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.db_generation = e.db_generation.wrapping_add(1);
    }
}

/// The info again when the datablock changed or was written since it was built (the tick's first act).
pub fn refresh_info(vm: &mut Vm, id: ObjId) {
    let Some(db) = object_field(vm, id, b"dataBlock") else {
        return;
    };
    let Some((old_db, old_gen)) = ext::get(vm.objects(), id)
        .and_then(|e| e.player.as_deref())
        .map(|p| (p.db, p.generation))
    else {
        return;
    };
    let count = generation(vm, db);
    if old_db == db && old_gen == count {
        return;
    }
    // A refused budget leaves the old products; the run ends with the step limit.
    let Some(info) = info_of(vm, db) else {
        return;
    };
    if let Some(p) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.player.as_deref_mut()) {
        p.info = info;
        p.db = db;
        p.generation = count;
    }
}

// ------------------------------------------------------------------------------------------------------------------- placement

/// The object box the scene holds for `id` (the box `Player::onNewDataBlock` made).
fn scene_box(vm: &Vm, id: ObjId) -> Option<Box3> {
    let s = ext::get(vm.objects(), id)?.scene?;
    Some(Box3::new(s.object_min, s.object_max))
}

/// `Player::setTransform`'s rotation: the yaw `-f32(atan2(-m[1], m[5]))`.
pub fn yaw_of(m: &collide::math::Matrix) -> f32 {
    -(collide::math::transcendental::atan2(-f64::from(m[1]), f64::from(m[5])) as f32)
}

/// Writes the state's transform and world box into the VM's matrix and the scene (the copies); the caller checks the bins.
pub fn mirror_no_bins(vm: &mut Vm, id: ObjId) {
    let Some(s) = state(vm, id) else {
        return;
    };
    let copy = (s.obj_to_world, s.world_box, s.world_center, s.world_radius);
    write_copies(vm, id, copy);
}

/// [`mirror_no_bins`] from a state that is out of the ext (the Player's own tick).
pub fn mirror_from(vm: &mut Vm, id: ObjId, s: &PlayerState) {
    write_copies(
        vm,
        id,
        (s.obj_to_world, s.world_box, s.world_center, s.world_radius),
    );
}

fn write_copies(vm: &mut Vm, id: ObjId, copy: (collide::math::Matrix, Box3, F3, f32)) {
    let (m, wb, wc, wr) = copy;
    crate::scene::access::set_matrix(vm, id, &m);
    if let Some(sc) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.scene.as_mut()) {
        sc.world_min = wb.min;
        sc.world_max = wb.max;
        sc.world_center = wc;
        sc.world_radius = wr;
    }
}

/// [`mirror_no_bins`], then `checkBins`.
pub fn mirror(vm: &mut Vm, container: &Container, id: ObjId) {
    mirror_no_bins(vm, id);
    scene::check_bins(vm, container, id);
}

/// `Player::setTransform(matrix)` (-> `setPosition`): the position and the yaw; mounted, the mount's node transform
/// instead of the position. The copies follow (the caller checks the bins); marks 0x2000040.
pub fn set_transform(vm: &mut Vm, id: ObjId, m: &collide::math::Matrix) {
    let yaw = yaw_of(m);
    let mount = super::mount::mount_of(vm, id);
    let node = super::mount::link(vm, id).1;
    let mt = mount.map(|mo| shapebase::mount_transform(vm, mo, node as usize));
    let (scale, obj_box) = (scale_of(vm, id), scene_box(vm, id));
    let kind = info(vm, id).map(|i| Rc::clone(&i.class));
    let Some(s) = state_mut(vm, id) else {
        shapebase::player_set_transform(vm, id, m);
        return;
    };
    // `setScale` sets the transform again: the state takes the scale and the box the scene has.
    s.scale = scale;
    if let Some(b) = obj_box {
        s.obj_box = b;
    }
    if let Some(kind) = kind {
        kind.update_scaled_box(s);
    }
    s.place([m[3], m[7], m[11]], [0.0, 0.0, yaw], mt);
    mirror_no_bins(vm, id);
    ghosts::mark(vm, id, 0x200_0040);
}

/// The mount copy: the state's mount and node from the ext's links.
pub fn sync_mount(vm: &mut Vm, id: ObjId) {
    let (on, node) = super::mount::link(vm, id);
    if let Some(s) = state_mut(vm, id) {
        s.mount = on.map(collide::ObjectId);
        s.mount_node = node;
    }
}

/// `Player::setActionThread(action, forward, hold, wait, fsp, force)` on the Player's threads, keeping the state's action in
/// step. True when the action was set.
pub fn play_action(
    vm: &mut Vm,
    id: ObjId,
    action: u32,
    forward: bool,
    hold: bool,
    wait: bool,
    fsp: bool,
) -> bool {
    let blend = crate::scene::hitboxes::wanted(vm, id);
    let Some(mut st) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take()) else {
        return false;
    };
    let request = ActionRequest {
        forward,
        hold,
        wait,
        fsp,
        force: false,
        blend,
    };
    let message = shapebase::action_thread(&mut st, action as usize, request);
    let none = crate::sim_classes::sim_classes(vm).action_table().none;
    let now = st.player.as_ref().map_or(none, |p| p.action);
    if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
        e.shape = Some(st);
    }
    if let Some(s) = state_mut(vm, id) {
        s.action_anim = now;
    }
    if let Some(m) = message {
        vm.error(m);
    }
    now == action
}

// ---------------------------------------------------------------------------------------------------------- the datablock hook

/// `Player::onNewDataBlock` after the shape part, and `ShapeBase::onNewDataBlock`'s words: the first PlayerData
/// makes the Player's state at the transform the fields gave it (the world box as the scene holds it: it is not reset here);
/// a later one keeps the state, scales damage and energy, and takes the new speeds, mass, drag and box. Any other ShapeBase
/// gets its [`super::vitals`] words.
pub fn new_datablock(vm: &mut Vm, id: ObjId, db: ObjId) -> Result<(), VmError> {
    if !is_class(vm, id, "ShapeBase") || !is_class(vm, db, "ShapeBaseData") {
        return Ok(());
    }
    let had = ext::get(vm.objects(), id).and_then(|e| e.player.as_deref().map(|p| p.db));
    if !is_player(vm, id) || !is_class(vm, db, "PlayerData") {
        let old = ext::get(vm.objects(), id).and_then(|e| e.vitals_db);
        super::vitals::new_datablock(vm, id, old, db);
        if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
            e.vitals_db = Some(db);
        }
        return Ok(());
    }
    let Some(info) = info_of(vm, db) else {
        return Err(VmError::StepLimit);
    };
    let count = generation(vm, db);
    let obj_box = scene_box(vm, id);
    let type_mask = ext::get(vm.objects(), id).map_or(0, |e| e.type_mask);
    match had {
        None => {
            let m = matrix_of(vm, id).unwrap_or(collide::math::IDENTITY);
            let mut s = info.class.new_state(
                collide::ObjectId(id),
                &info.data,
                [m[3], m[7], m[11]],
                yaw_of(&m),
            );
            s.type_mask = type_mask;
            s.scale = scale_of(vm, id);
            if let Some(b) = obj_box {
                s.obj_box = b;
            }
            info.class.update_scaled_box(&mut s);
            if let Some(sc) = ext::get(vm.objects(), id).and_then(|e| e.scene) {
                s.world_box = Box3::new(sc.world_min, sc.world_max);
                s.world_center = sc.world_center;
                s.world_radius = sc.world_radius;
            }
            if is_class(vm, id, "AIPlayer") {
                info.class.make_ai(&mut s);
            }
            s.recharge_rate = float(vm, db, b"rechargeRate", 0.0);
            if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
                e.player = Some(Box::new(PlayerExt {
                    body: Some(Player::new(s)),
                    parked: None,
                    info,
                    db,
                    generation: count,
                    net_move: false,
                }));
            }
            // The level and state updates of the first datablock.
            super::vitals::update_damage_level(vm, id)?;
            super::vitals::update_damage_state(vm, id);
        }
        Some(old) => {
            super::vitals::new_datablock(vm, id, Some(old), db);
            let d = info.data.clone();
            let old_drag = ext::get(vm.objects(), id)
                .and_then(|e| e.player.as_deref())
                .map_or(d.drag, |p| p.info.data.drag);
            if let Some(p) =
                ext::get_mut(vm.objects_mut(), id).and_then(|e| e.player.as_deref_mut())
            {
                p.info = Rc::clone(&info);
                p.db = db;
                p.generation = count;
                if let Some(s) = p.state_mut() {
                    s.mass = d.mass;
                    if old_drag != d.drag {
                        s.drag = d.drag;
                    }
                    s.speeds = [
                        d.max_forward_speed,
                        d.max_side_speed,
                        d.max_backward_speed,
                        d.max_underwater_forward_speed,
                        d.max_underwater_side_speed,
                        d.max_underwater_backward_speed,
                    ];
                    info.class.datablock_changed(s, &d);
                    if let Some(b) = obj_box {
                        s.obj_box = b;
                        info.class.update_scaled_box(s);
                    }
                }
            }
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------------------------------------------------- the classes

/// `Player::onAdd` after `ShapeBase::onAdd`: refused without a PlayerData; the object enters the bins, plays action 0
/// (`setActionThread(0, forward, ..., force)`, the constructor's action), and the datablock's script `onAdd(%obj)` runs.
struct PlayerClass {
    container: Rc<Container>,
}

impl EngineClass for PlayerClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        if state(vm, id).is_none() {
            return Ok(false);
        }
        scene::add_to_scene(vm, &self.container, id);
        if let Some(mut st) = ext::get_mut(vm.objects_mut(), id).and_then(|e| e.shape.take()) {
            let request = ActionRequest {
                forward: true,
                hold: false,
                wait: false,
                fsp: false,
                force: true,
                blend: false,
            };
            let _ = shapebase::action_thread(&mut st, 0, request);
            let none = crate::sim_classes::sim_classes(vm).action_table().none;
            let now = st.player.as_ref().map_or(none, |p| p.action);
            if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
                e.shape = Some(st);
            }
            if let Some(s) = state_mut(vm, id) {
                s.action_anim = now;
            }
        }
        if let Some(db) = object_field(vm, id, b"dataBlock") {
            vm.call_method(db, b"onAdd", &[id.to_string().into_bytes()])?;
        }
        Ok(true)
    }

    /// `Player::onRemove`: the datablock's script `onRemove(%obj)` (`scriptOnRemove`) before the scene and
    /// `ShapeBase::onRemove`'s work.
    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        shapebase::script_on_remove(vm, id)
    }
}

/// `ShapeBase::onRemove`'s parts the engine keeps: the mounts let go and the control links cut.
struct ShapeBaseClass;

impl EngineClass for ShapeBaseClass {
    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        // The destructor's release of the name handles (it runs whatever the unmount does).
        crate::scene::shapebase::release_names(vm, id);
        super::mount::removed(vm, id)?;
        methods::control_removed(vm, id);
        Ok(())
    }
}

/// Gives Player and AIPlayer their behaviour (replacing the scene entry for them), ShapeBase its removal work, and the methods.
/// After `shapebase::register`.
pub fn register(vm: &mut Vm, container: &Rc<Container>) {
    vm.set_class_behaviour(
        "Player",
        Rc::new(PlayerClass {
            container: Rc::clone(container),
        }),
    );
    vm.set_class_behaviour("ShapeBase", Rc::new(ShapeBaseClass));
    methods::register(vm);
    super::mount::register(vm);
    super::vitals::register(vm);
}

/// A point as three floats (`F3`).
pub fn position(vm: &Vm, id: ObjId) -> Option<F3> {
    state(vm, id).map(PlayerState::position)
}
