//! The vehicle step's cost: buggies driven over a floor with a block in front of each, ticked at `integration = 8` (the
//! sub-steps the stock buggy runs), and the worst client replay, 46 consecutive ticks of one vehicle (a clone each, as the
//! prediction makes one a move).
//!
//! The world is a list of boxes: a floor whose top is at z = 0 and a block ahead of every buggy, so the wheel rays, the
//! working set, the GJK states, the contacts and the hold-out all run. Two more grounds for one buggy: "on tiles" is a floor
//! cut into 0.5-unit boxes (a working set of a few hundred convexes), and "beside hulls" makes the block and 20 pillars along
//! the way convex hulls of 48 vertices (a shape's collision mesh). The allocations a tick on each ground are printed ahead of
//! the timings (a counting allocator).

use std::hint::black_box;
use std::rc::Rc;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use torque_collide::convex::{
    BOX_CONVEX_TYPE, ConvexBuilder, ConvexKind, ConvexSource, HullSlot, WorkingSet, query_in_object,
};
use torque_collide::math::{Box3, F3, IDENTITY, d};
use torque_collide::mesh::{Hull, MeshPart};
use torque_collide::{Exhausted, ObjectId, ObjectView, World, types, work};
use torque_vehicle::data::{
    ShapeBaseData, SpringData, TireData, VehicleData, WheelRecord, WheeledVehicleData,
};
use torque_vehicle::host::{RayHit, VehicleHost};
use torque_vehicle::tick::{Vehicle, process_tick};
use torque_vehicle::wheeled::{self, Wheeled};
use torque_vehicle::{Move, VehicleState};

#[global_allocator]
static GLOBAL: &StatsAlloc<std::alloc::System> = &INSTRUMENTED_SYSTEM;

/// The most ticks a client replays in one tick of its own.
const REPLAY: usize = 46;
/// The ticks one buggy runs in the scaling benches: long enough to reach its block and press on it.
const DRIVE: usize = 100;

/// The buggy's object box.
const BODY_BOX: Box3 = Box3::new([-0.9, -1.9, -0.5], [0.9, 1.9, 0.5]);

/// A static box: its world box, as a solid; a pillar is a convex hull inside that box instead.
struct Solid {
    min: F3,
    max: F3,
    hull: bool,
}

impl Solid {
    fn block(min: F3, max: F3) -> Solid {
        Solid {
            min,
            max,
            hull: false,
        }
    }
}

impl Solid {
    fn center(&self) -> F3 {
        std::array::from_fn(|i| (self.min[i] + self.max[i]) * 0.5)
    }

    fn half(&self) -> F3 {
        std::array::from_fn(|i| ((d(self.max[i]) - d(self.min[i])) * 0.5) as f32)
    }
}

/// The class of the boxes: one ortho box at the object box's centre, made when the query meets the object box.
#[derive(Debug)]
struct BoxClass {
    half: F3,
}

impl ConvexBuilder for BoxClass {
    fn build_convex(
        &self,
        set: &mut WorkingSet,
        world: &mut dyn World,
        id: ObjectId,
        view: &ObjectView,
        query: &Box3,
    ) -> Result<(), Exhausted> {
        let q = query_in_object(view, query);
        if !q.overlaps(&view.obj_box) || set.find(BOX_CONVEX_TYPE, id, None).is_some() {
            return Ok(());
        }
        if !world.charge(work::CONVEX) {
            return Err(Exhausted);
        }
        let b = view.obj_box;
        let kind = ConvexKind::OrthoBox {
            center: std::array::from_fn(|i| (b.min[i] + b.max[i]) * 0.5),
            size: self.half,
        };
        set.add(id, kind);
        Ok(())
    }
}

/// The world and the host of every vehicle in it.
struct Sandbox {
    solids: Vec<Solid>,
    sources: Vec<ConvexSource>,
    /// The buggies' collision mesh (every id from [`FIRST_VEHICLE`] on).
    body: Rc<Hull>,
    /// The pillars' hulls, by solid.
    pillars: Vec<Option<Rc<Hull>>>,
}

/// The id of the first buggy; the solids' ids are their indexes below it.
const FIRST_VEHICLE: u32 = 100_000;

/// The buggy's hull: its object box as 12 triangles wound outward.
fn body_hull(b: &Box3) -> Hull {
    let verts: Vec<F3> = (0..8)
        .map(|i| std::array::from_fn(|k| if i >> k & 1 == 1 { b.max[k] } else { b.min[k] }))
        .collect();
    let mid: F3 = std::array::from_fn(|k| (b.min[k] + b.max[k]) * 0.5);
    let mut tris = Vec::new();
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        for side in 0..2usize {
            let at = |a: usize, c: usize| (side << axis) | (a << u) | (c << v);
            for [p, q, r] in [
                [at(0, 0), at(1, 0), at(1, 1)],
                [at(0, 0), at(1, 1), at(0, 1)],
            ] {
                let (a, bb, c) = (verts[p], verts[q], verts[r]);
                let n: F3 = {
                    let (e1, e2): (F3, F3) = (
                        std::array::from_fn(|k| a[k] - bb[k]),
                        std::array::from_fn(|k| c[k] - bb[k]),
                    );
                    [
                        e1[1] * e2[2] - e1[2] * e2[1],
                        e1[2] * e2[0] - e1[0] * e2[2],
                        e1[0] * e2[1] - e1[1] * e2[0],
                    ]
                };
                let out: f32 = (0..3)
                    .map(|k| n[k] * ((a[k] + bb[k] + c[k]) / 3.0 - mid[k]))
                    .sum();
                tris.push(if out > 0.0 {
                    [p as u32, q as u32, r as u32]
                } else {
                    [r as u32, q as u32, p as u32]
                });
            }
        }
    }
    let part = MeshPart {
        node: IDENTITY,
        materials: vec![0; tris.len()],
        verts,
        tris,
    };
    Hull::new(Some(IDENTITY), vec![part])
}

/// A 24-sided prism filling the box `half` (centered on the origin): 48 vertices and 92 triangles wound outward.
fn pillar_hull(half: F3) -> Hull {
    const SIDES: u32 = 24;
    let mut verts = Vec::new();
    for z in [-half[2], half[2]] {
        for k in 0..SIDES {
            let a = k as f32 * std::f32::consts::TAU / SIDES as f32;
            verts.push([half[0] * a.cos(), half[1] * a.sin(), z]);
        }
    }
    let mut tris = Vec::new();
    for k in 0..SIDES {
        let n = (k + 1) % SIDES;
        tris.push([k, n, SIDES + n]);
        tris.push([k, SIDES + n, SIDES + k]);
    }
    for k in 1..SIDES - 1 {
        tris.push([0, k + 1, k]);
        tris.push([SIDES, SIDES + k, SIDES + k + 1]);
    }
    let part = MeshPart {
        node: IDENTITY,
        materials: vec![0; tris.len()],
        verts,
        tris,
    };
    Hull::new(Some(IDENTITY), vec![part])
}

impl Sandbox {
    fn new(solids: Vec<Solid>) -> Sandbox {
        let sources = solids
            .iter()
            .map(|s| {
                let h = s.half();
                if s.hull {
                    ConvexSource::Shape {
                        has_shape: true,
                        slots: [HullSlot {
                            slot: 0,
                            present: true,
                            bounds: Box3::new(h.map(|v| -v), h),
                        }]
                        .into(),
                    }
                } else {
                    ConvexSource::Custom(Rc::new(BoxClass { half: h }))
                }
            })
            .collect();
        let pillars = solids
            .iter()
            .map(|s| s.hull.then(|| Rc::new(pillar_hull(s.half()))))
            .collect();
        let body = Rc::new(body_hull(&BODY_BOX));
        Sandbox {
            solids,
            sources,
            body,
            pillars,
        }
    }

    fn solid(&self, id: ObjectId) -> Option<&Solid> {
        self.solids.get(id.0 as usize)
    }
}

impl World for Sandbox {
    fn object(&self, id: ObjectId) -> Option<ObjectView> {
        let b = self.solid(id)?;
        let (c, h) = (b.center(), b.half());
        let mut m = IDENTITY;
        let mut inv = IDENTITY;
        for i in 0..3 {
            m[i * 4 + 3] = c[i];
            inv[i * 4 + 3] = -c[i];
        }
        Some(ObjectView {
            obj_to_world: m,
            world_to_obj: inv,
            scale: [1.0; 3],
            obj_box: Box3::new(h.map(|v| -v), h),
            world_box: Box3::new(b.min, b.max),
            world_radius: 1.0,
            type_mask: types::STATIC | types::STATIC_SHAPE,
            collision_disabled: false,
        })
    }

    fn find_objects(
        &mut self,
        query: &Box3,
        mask: u32,
        _exclude: Option<ObjectId>,
        limit: usize,
        out: &mut Vec<ObjectId>,
    ) -> Result<(), Exhausted> {
        if mask & types::STATIC_SHAPE == 0 {
            return Ok(());
        }
        for (i, b) in self.solids.iter().enumerate() {
            if out.len() >= limit {
                break;
            }
            if Box3::new(b.min, b.max).overlaps(query) {
                out.push(ObjectId(i as u32));
            }
        }
        Ok(())
    }

    fn convex_source(&self, id: ObjectId) -> ConvexSource {
        self.sources
            .get(id.0 as usize)
            .cloned()
            .unwrap_or(ConvexSource::None)
    }

    fn hull(&self, id: ObjectId, slot: u32) -> Option<Rc<Hull>> {
        if slot != 0 {
            return None;
        }
        if id.0 >= FIRST_VEHICLE {
            return Some(Rc::clone(&self.body));
        }
        self.pillars.get(id.0 as usize)?.clone()
    }

    fn charge(&mut self, _units: u64) -> bool {
        true
    }
}

/// The nearest entry of the segment `start..end` into a box (the slab test), as a fraction of the segment and the face's normal.
fn slab(b: &Solid, start: F3, end: F3) -> Option<(f32, F3)> {
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let mut normal = [0.0; 3];
    for i in 0..3 {
        let dir = end[i] - start[i];
        if dir == 0.0 {
            if start[i] < b.min[i] || start[i] > b.max[i] {
                return None;
            }
            continue;
        }
        let (mut near, mut far) = ((b.min[i] - start[i]) / dir, (b.max[i] - start[i]) / dir);
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        if near > t0 {
            t0 = near;
            normal = [0.0; 3];
            normal[i] = if dir > 0.0 { -1.0 } else { 1.0 };
        }
        t1 = t1.min(far);
        if t0 > t1 {
            return None;
        }
    }
    (t0 > 0.0).then_some((t0, normal))
}

impl VehicleHost for Sandbox {
    fn world(&mut self) -> &mut dyn World {
        self
    }

    fn sim_time(&self) -> u32 {
        0
    }

    fn cast_ray(&mut self, start: F3, end: F3, _mask: u32) -> Option<RayHit> {
        let mut best: Option<(f32, F3)> = None;
        for b in &self.solids {
            if let Some((t, n)) = slab(b, start, end)
                && best.is_none_or(|(bt, _)| t < bt)
            {
                best = Some((t, n));
            }
        }
        let (t, normal) = best?;
        Some(RayHit {
            t,
            point: std::array::from_fn(|i| start[i] + (end[i] - start[i]) * t),
            normal,
            material: 0,
            object: None,
        })
    }
}

/// The stock buggy's numbers: four wheels, the datablock the trace scenarios drive.
fn datablock() -> WheeledVehicleData {
    let wheel = |x: f32, y: f32, opposite: i32| WheelRecord {
        opposite,
        pos: [x, y, -0.2],
        hub_node: 0,
        spring_seq: -1,
        spring_len: 0.0,
    };
    WheeledVehicleData {
        base: VehicleData {
            shape: ShapeBaseData {
                mass: 380.0,
                drag: 0.6,
                max_damage: 1000.0,
                ..Default::default()
            },
            body_friction: 0.6,
            body_restitution: 0.3,
            min_impact_speed: 5.0,
            integration: 8,
            collision_tol: 0.1,
            contact_tol: 0.4,
            max_steering_angle: 0.55,
            mass_center: [0.0, 0.0, 0.2],
            mass_box: [1.6, 3.8, 0.9],
            ..Default::default()
        },
        max_wheel_speed: 22.0,
        engine_torque: 5200.0,
        engine_brake: 250.0,
        brake_torque: 9000.0,
        wheels: vec![
            wheel(-0.8, 1.2, 1),
            wheel(0.8, 1.2, 0),
            wheel(-0.8, -1.2, 3),
            wheel(0.8, -1.2, 2),
        ],
        ..Default::default()
    }
}

/// A buggy of `db` at (x, y), height 0.9, facing +y.
fn buggy(db: &WheeledVehicleData, id: u32, x: f32, y: f32) -> Vehicle<Wheeled> {
    let tire = TireData {
        static_friction: 4.4,
        kinetic_friction: 3.4,
        lateral_force: 17000.0,
        lateral_damping: 4000.0,
        longitudinal_force: 17000.0,
        longitudinal_damping: 4000.0,
        radius: 0.5,
        ..Default::default()
    };
    let spring = SpringData {
        length: 0.42,
        force: 3400.0,
        damping: 1400.0,
        anti_sway_force: 8.0,
    };
    let mut m = IDENTITY;
    m[3] = x;
    m[7] = y;
    m[11] = 0.9;
    let s = VehicleState::new(ObjectId(id), &db.base, BODY_BOX, &m, [1.0; 3]);
    let mut v = Vehicle::new(s, Wheeled::default());
    wheeled::on_new_data_block(&mut v.s, &mut v.k, db);
    for i in 0..db.wheels.len() {
        wheeled::set_wheel_tire(&mut v, db, i, (ObjectId(1000), tire));
        wheeled::set_wheel_spring(&mut v, db, i, (ObjectId(1001), spring));
    }
    v
}

/// What the buggies drive on and into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ground {
    /// One floor box and a box block ahead of each buggy.
    Boxes,
    /// A floor cut into 0.5-unit boxes around each buggy's way, over a floor a unit lower.
    Tiles,
    /// The floor box, the block ahead a hull and 10 hull pillars on each side of the way.
    Hulls,
}

/// The floor, and a block 7 m ahead of each of the `count` buggies of an 8-wide grid; the buggies, whose ids are past the solids'.
fn scene(count: usize) -> (Sandbox, WheeledVehicleData, Vec<Vehicle<Wheeled>>) {
    scene_on(count, Ground::Boxes)
}

fn scene_on(count: usize, ground: Ground) -> (Sandbox, WheeledVehicleData, Vec<Vehicle<Wheeled>>) {
    let db = datablock();
    let floor_top = if ground == Ground::Tiles { -1.0 } else { 0.0 };
    let mut solids = vec![Solid::block(
        [-200.0, -200.0, floor_top - 1.0],
        [200.0, 200.0, floor_top],
    )];
    let mut at = Vec::new();
    for i in 0..count {
        let (x, y) = ((i % 8) as f32 * 14.0 - 50.0, (i / 8) as f32 * 30.0 - 100.0);
        solids.push(Solid {
            min: [x - 1.0, y + 7.0, 0.0],
            max: [x + 1.0, y + 9.0, 1.5],
            hull: ground == Ground::Hulls,
        });
        match ground {
            Ground::Boxes => {}
            Ground::Tiles => {
                for tx in 0..12 {
                    for ty in 0..56 {
                        let (mx, my) = (x - 3.0 + tx as f32 * 0.5, y - 4.0 + ty as f32 * 0.5);
                        solids.push(Solid::block([mx, my, -1.0], [mx + 0.5, my + 0.5, 0.0]));
                    }
                }
            }
            Ground::Hulls => {
                for side in [-1.0f32, 1.0] {
                    for k in 0..10 {
                        let (px, py) = (x + side * 2.2, y - 1.0 + k as f32 * 0.8);
                        solids.push(Solid {
                            min: [px - 0.3, py - 0.3, 0.0],
                            max: [px + 0.3, py + 0.3, 1.2],
                            hull: true,
                        });
                    }
                }
            }
        }
        at.push((x, y));
    }
    let vehicles = at
        .into_iter()
        .enumerate()
        .map(|(i, (x, y))| buggy(&db, FIRST_VEHICLE + i as u32, x, y))
        .collect();
    (Sandbox::new(solids), db, vehicles)
}

/// The throttle held and a little steering, as a drive.
fn moves() -> [Move; 4] {
    let mv = |y: f32, yaw: f32| Move {
        y,
        yaw,
        ..Default::default()
    };
    [mv(1.0, 0.0), mv(1.0, 0.2), mv(1.0, 0.0), mv(1.0, -0.2)]
}

fn drive(v: &mut Vehicle<Wheeled>, db: &WheeledVehicleData, host: &mut Sandbox, ticks: usize) {
    let moves = moves();
    for t in 0..ticks {
        let _ = process_tick(v, db, Some(&moves[t % moves.len()]), host);
    }
}

fn scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("tick");
    for count in [1usize, 8, 64] {
        let (mut host, db, vehicles) = scene(count);
        group.bench_with_input(BenchmarkId::new("buggies", count), &count, |b, _| {
            b.iter_batched(
                || vehicles.clone(),
                |mut vs| {
                    for v in &mut vs {
                        drive(v, &db, &mut host, DRIVE);
                    }
                    black_box(vs.iter().map(|v| v.s.rigid.lin_pos[1]).sum::<f32>())
                },
                BatchSize::LargeInput,
            )
        });
    }
    for (name, ground) in [("on tiles", Ground::Tiles), ("beside hulls", Ground::Hulls)] {
        let (mut host, db, vehicles) = scene_on(1, ground);
        group.bench_function(format!("buggy {name}"), |b| {
            b.iter_batched(
                || vehicles.clone(),
                |mut vs| {
                    drive(&mut vs[0], &db, &mut host, DRIVE);
                    black_box(vs[0].s.rigid.lin_pos[1])
                },
                BatchSize::LargeInput,
            )
        });
    }
    group.finish();
}

/// The allocations a tick of one buggy's drive on `ground`, and where the buggy ends.
fn allocations(ground: Ground) -> (f64, f32) {
    let (mut host, db, mut vehicles) = scene_on(1, ground);
    let region = Region::new(GLOBAL);
    drive(&mut vehicles[0], &db, &mut host, DRIVE);
    let n = region.change().allocations as f64 / DRIVE as f64;
    (n, vehicles[0].s.rigid.lin_pos[1])
}

/// A client's worst replay: a correction rewinds a vehicle and 46 ticks run again, a clone of the vehicle each.
fn replay(c: &mut Criterion) {
    let (mut host, db, vehicles) = scene(1);
    let mut warm = vehicles[0].clone();
    drive(&mut warm, &db, &mut host, 40);
    let moves = moves();
    c.bench_function("tick/replay 46 x 8", |b| {
        b.iter(|| {
            let mut v = warm.clone();
            for t in 0..REPLAY {
                v = v.clone();
                let _ = process_tick(&mut v, &db, Some(&moves[t % moves.len()]), &mut host);
            }
            black_box(v.s.rigid.lin_pos[1])
        })
    });
}

/// FNV-1a over the bits of every rigid word of 8 buggies after each tick of a drive that reaches the blocks: the same
/// value before and after a change that must not move the step. Printed, with the end point, ahead of the timings.
fn trace() -> (u64, f32) {
    let (mut host, db, mut vehicles) = scene(8);
    let moves = moves();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in &mut vehicles {
        for t in 0..DRIVE {
            let _ = process_tick(v, &db, Some(&moves[t % moves.len()]), &mut host);
            for w in v.s.rigid.to_words() {
                for b in w.to_le_bytes() {
                    h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
        }
    }
    (h, vehicles[0].s.rigid.lin_pos[1])
}

criterion_group!(benches, scaling, replay);

fn main() {
    let (hash, end) = trace();
    eprintln!(
        "rigid trace {hash:016x} (first buggy ends at y = {end:.4}; the block is 7 m ahead of its start)"
    );
    for ground in [Ground::Boxes, Ground::Tiles, Ground::Hulls] {
        let (n, end) = allocations(ground);
        eprintln!("{ground:?}: {n:.1} allocations a tick (the buggy ends at y = {end:.4})");
    }
    benches();
    Criterion::default().configure_from_args().final_summary();
}
