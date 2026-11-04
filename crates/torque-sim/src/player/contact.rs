//! `Player::findContact`, `updateWorkingCollisionSet` and `step`, as TGE 1.4's `player.cc` writes them: the defaults of
//! [`PlayerClass::find_contact`] and [`PlayerClass::update_working_set`], and the ledge step of `updatePos`.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/player.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::math::{Box3, F3, Plane, mul_box};
use torque_collide::{Exhausted, ObjectId, types, work};

use super::data::PlayerData;
use super::ext::{PlayerClass, TorquePlayer};
use super::host::PlayerHost;
use super::update_move::len3;
use super::{DT, MAX_CONTACT_TOUCHES, Player, shape_base};

/// How far below the feet a surface still counts as a contact (`sTractionDistance`).
pub const TRACTION_DISTANCE: f32 = 0.03;
/// The height a step's surface is lifted by, and the gap kept above it (`sMinFaceDistance`).
pub const MIN_FACE_DISTANCE: f32 = 0.01;
/// The working set's cache sentinel: the cache is initialised to it, and a cache whose min x holds it is refreshed.
pub const CACHE_SENTINEL: f32 = -1.0e9;

/// The six planes of a box, outward (`n . p + d > 0` outside), in the order `findContact` and `step` set them.
pub(super) fn box_planes(min: F3, max: F3) -> [Plane; 6] {
    [
        Plane::new(-1.0, 0.0, 0.0, min[0]),
        Plane::new(0.0, 1.0, 0.0, -max[1]),
        Plane::new(1.0, 0.0, 0.0, -max[0]),
        Plane::new(0.0, -1.0, 0.0, min[1]),
        Plane::new(0.0, 0.0, -1.0, min[2]),
        Plane::new(0.0, 0.0, 1.0, -max[2]),
    ]
}

/// `Box3F::isContained`: `inner` lies within `outer`.
fn contains(outer: &Box3, inner: &Box3) -> bool {
    (0..3).all(|k| outer.min[k] <= inner.min[k] && inner.max[k] <= outer.max[k])
}

/// `findContact(&run, &jump, &normal)` (TGE's; [`PlayerClass::find_contact`]'s default). The polygons under the feet, within
/// [`TRACTION_DISTANCE`], are collected from the working set; the one that faces up the most gives the normal, and `run` /
/// `jump` say whether it is flat enough to run or jump on. Triggers met are told of the Player; corpses and items whose world
/// box overlaps the Player's are queued as collisions (an item not thrown by this Player); an object in the host's touch mask
/// is offered to the host ([`PlayerHost::contact_touch`]) and polled after the walk. The result is kept in the state's contact
/// snapshot.
pub fn find_contact(
    p: &mut Player,
    db: &PlayerData,
    run: &mut bool,
    jump: &mut bool,
    normal: &mut F3,
    host: &mut dyn PlayerHost,
) -> Result<(), Exhausted> {
    let s = &mut *p.s;
    let pos = s.position();
    let b = s.scaled_box;
    let min = [
        pos[0] + b.min[0],
        pos[1] + b.min[1],
        pos[2] + b.min[2] - TRACTION_DISTANCE,
    ];
    let max = [
        pos[0] + b.max[0],
        pos[1] + b.max[1],
        pos[2] + b.min[2] + TRACTION_DISTANCE,
    ];
    let list = &mut p.scratch.contact;
    list.clear();
    list.base.reset();
    list.base.set_interest_normal(Some([0.0, 0.0, -1.0]));
    list.normal = [0.0; 3];
    list.planes.clear();
    list.planes.extend_from_slice(&box_planes(min, max));
    let plist_box = Box3::new(min, max);
    let masks = *host.type_masks();
    let mut touched: [Option<ObjectId>; MAX_CONTACT_TOUCHES] = [None; MAX_CONTACT_TOUCHES];
    let mut n_touched = 0usize;
    let now = host.sim_time();
    for c in p.working.iter() {
        let world = host.world();
        if !world.charge(work::CONVEX) {
            return Err(Exhausted);
        }
        let Some(view) = world.object(c.object) else {
            continue;
        };
        let tm = view.type_mask;
        let obj = c.object;
        if tm & types::TRIGGER != 0 {
            host.potential_enter_trigger(s, obj);
            continue;
        }
        if tm & (types::CORPSE | types::ITEM) != 0 {
            let thrown = tm & types::CORPSE == 0 && host.item_thrower(obj) == Some(s.id);
            if s.world_box.overlaps(&view.world_box) && !thrown {
                let v = host.object_velocity(obj);
                let u = s.velocity;
                let vec = [u[0] - v[0], u[1] - v[1], u[2] - v[2]];
                shape_base::queue_collision(s, now, obj, vec);
            }
            continue;
        }
        if tm & masks.player_touch != 0 {
            let full = n_touched >= MAX_CONTACT_TOUCHES;
            if host.contact_touch(s, obj, &view, &plist_box, full) && !full {
                touched[n_touched] = Some(obj);
                n_touched += 1;
            }
        } else if tm & masks.player_working == 0 || tm & types::PHYSICAL_ZONE != 0 {
            continue;
        }
        let world = host.world();
        let Some(bb) = c.bounding_box(world) else {
            continue;
        };
        if !plist_box.overlaps(&bb) {
            continue;
        }
        c.poly_list(world, &mut *list)?;
    }
    for t in touched.iter().take(n_touched).flatten() {
        host.on_contact_touch(s, *t);
    }
    // The flattest surface.
    if list.polys.is_empty() {
        *run = false;
        *jump = false;
    } else {
        let mut best = -1.0f32;
        for poly in &list.polys {
            let vd = poly.plane.z;
            if vd > best {
                best = vd;
                *normal = poly.plane.normal();
            }
        }
        *run = best > db.run_surface_cos;
        *jump = best > db.jump_surface_cos;
    }
    s.contact_found = !list.polys.is_empty();
    if s.contact_found {
        s.contact_normal = *normal;
    }
    s.contact_run = *run;
    s.contact_jump = *jump;
    Ok(())
}

/// `updateWorkingCollisionSet` (TGE's; [`PlayerClass::update_working_set`]'s default). The convex's box is grown by
/// `l = (|v dt| + 10 dt) * 1.1 + 0.1` on every side; when the cached query box does not hold it, the set is rebuilt over that
/// box grown again by `2 l`.
pub fn update_working_collision_set(
    p: &mut Player,
    _db: &PlayerData,
    host: &mut dyn PlayerHost,
) -> Result<(), Exhausted> {
    let class = host.player_class();
    let class: &dyn PlayerClass = class.as_deref().unwrap_or(&TorquePlayer);
    let s = &mut *p.s;
    let v = s.velocity;
    let len = len3([v[0] * DT, v[1] * DT, v[2] * DT]);
    let new_len = len + 10.0 * DT;
    let mut convex_box = mul_box(&s.obj_to_world, &class.working_box(s));
    let l = new_len * 1.1 + 0.1;
    for k in 0..3 {
        convex_box.min[k] -= l;
        convex_box.max[k] += l;
    }
    let c = s.working_box;
    if c.min[0] != CACHE_SENTINEL && contains(&c, &convex_box) {
        return Ok(());
    }
    let mut query = convex_box;
    for k in 0..3 {
        query.min[k] -= 2.0 * l;
        query.max[k] += 2.0 * l;
    }
    s.working_box = query;
    let id = s.id;
    let wmask = host.type_masks().player_working;
    p.working.update(host.world(), id, &query, wmask)
}

/// `step(&pos, &maxStep, time)`: true when the position rose onto a ledge (z only) within the budget `maxStep`. The box of
/// [`PlayerClass::step_box`] after `time` at the velocity, its top raised by `maxStepHeight * scale.z` and
/// [`MIN_FACE_DISTANCE`], collects the polygons of the host's step objects; the highest vertex, lifted by
/// [`MIN_FACE_DISTANCE`], is the step when it is above the position and the rise is below the budget.
pub(super) fn step_up(
    p: &mut Player,
    db: &PlayerData,
    pos: &mut F3,
    budget: &mut f32,
    t: f32,
    host: &mut dyn PlayerHost,
) -> Result<bool, Exhausted> {
    let step_mask = host.type_masks().player_step;
    let class = host.player_class();
    let class: &dyn PlayerClass = class.as_deref().unwrap_or(&TorquePlayer);
    let s = &*p.s;
    let offset = [s.velocity[0] * t, s.velocity[1] * t, s.velocity[2] * t];
    let mut b = class.step_box(s, *pos, offset);
    b.max[2] += db.max_step_height * s.scale[2] + MIN_FACE_DISTANCE;
    let list = &mut p.scratch.step;
    list.clear();
    list.base.reset();
    list.normal = [0.0; 3];
    list.normal_test = false;
    list.planes.clear();
    list.planes.extend_from_slice(&box_planes(b.min, b.max));
    for c in p.working.iter() {
        let world = host.world();
        if !world.charge(work::CONVEX) {
            return Err(Exhausted);
        }
        let Some(view) = world.object(c.object) else {
            continue;
        };
        if view.type_mask & step_mask == 0 {
            continue;
        }
        let Some(bb) = c.bounding_box(world) else {
            continue;
        };
        if !b.overlaps(&bb) {
            continue;
        }
        c.poly_list(world, &mut *list)?;
    }
    if !host.world().charge(list.indices.len() as u64) {
        return Err(Exhausted);
    }
    let mut step_height = pos[2] - MIN_FACE_DISTANCE;
    for &i in &list.indices {
        if let Some(v) = list.verts.get(i as usize) {
            let h = v.point[2] + MIN_FACE_DISTANCE;
            if h > step_height {
                step_height = h;
            }
        }
    }
    let step = step_height - pos[2];
    if step_height > pos[2] && step < *budget {
        pos[2] = step_height;
        *budget -= step;
        return Ok(true);
    }
    Ok(false)
}
