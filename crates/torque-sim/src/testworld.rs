//! A synthetic host for the Item and Projectile tests: a [`BoxWorld`] of axis-aligned boxes (solids that collide through
//! `collide`'s box convexes, and ray-only objects such as a player), a slab ray cast, and the recorders of every host call.

use torque_collide::ConvexSource;
use torque_collide::math::{F3, d};
use torque_collide::testing::BoxWorld;
use torque_collide::{ObjectId, World, types};

use crate::item::{ItemData, ItemHost, ItemState};
use crate::projectile::{ProjectileHost, ProjectileState};
use crate::scene::RayHit;

/// A solid's type bits: static world, a static shape.
pub(crate) const SOLID: u32 = types::STATIC | types::STATIC_SHAPE;
/// A player-like shape: a ShapeBase and a Player.
pub(crate) const PLAYER: u32 = types::PLAYER | types::SHAPE_BASE;

/// One object of a [`TestHost`]'s world.
#[derive(Clone)]
pub(crate) struct Obj {
    pub id: ObjectId,
    pub min: F3,
    pub max: F3,
    pub type_mask: u32,
    /// Builds a box convex into working sets (a solid); false for a ray-only object.
    pub convex: bool,
    pub vel: F3,
    pub disabled: i32,
}

pub(crate) fn solid(id: u32, min: F3, max: F3) -> Obj {
    Obj {
        id: ObjectId(id),
        min,
        max,
        type_mask: SOLID,
        convex: true,
        vel: [0.0; 3],
        disabled: 0,
    }
}

/// A floor with its top at z = 0.
pub(crate) fn floor(id: u32) -> Obj {
    solid(id, [-20.0, -20.0, -1.0], [20.0, 20.0, 0.0])
}

/// A host call a Projectile test wants to see, in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ev {
    Collision {
        col: ObjectId,
        point: F3,
        normal: F3,
        vel: F3,
        fade: f32,
    },
    Explode {
        pos: F3,
        fade: f32,
        time: u32,
    },
    ScheduleDelete {
        id: ObjectId,
        at: u32,
    },
    Delete(ObjectId),
    Source {
        id: ObjectId,
        disabled: bool,
    },
}

pub(crate) struct TestHost {
    pub world: BoxWorld,
    /// Each object's velocity, by id.
    vels: Vec<(ObjectId, F3)>,
    pub time: u32,
    /// Every ray cast: start, end, mask.
    pub rays: Vec<(F3, F3, u32)>,
    pub events: Vec<Ev>,
    /// `disable_collision` (true) and `enable_collision` (false) calls.
    pub switches: Vec<(ObjectId, bool)>,
    /// The Item's `on_collision` calls.
    pub item_collisions: Vec<(ObjectId, F3)>,
    pub item_alive: bool,
    pub enters: Vec<(f32, u32)>,
    pub leaves: Vec<u32>,
    /// What `update_container` stores as the coverage (and the liquid type 7).
    pub coverage: f32,
    pub moved: u32,
}

impl TestHost {
    pub(crate) fn new(objs: Vec<Obj>) -> TestHost {
        let mut world = BoxWorld::new();
        let mut vels = Vec::new();
        for o in objs {
            let object = world.add_box(o.id, o.min, o.max, o.type_mask);
            object.disabled = o.disabled;
            if !o.convex {
                object.source = ConvexSource::None;
            }
            vels.push((o.id, o.vel));
        }
        TestHost {
            world,
            vels,
            time: 0,
            rays: Vec::new(),
            events: Vec::new(),
            switches: Vec::new(),
            item_collisions: Vec::new(),
            item_alive: true,
            enters: Vec::new(),
            leaves: Vec::new(),
            coverage: 0.0,
            moved: 0,
        }
    }

    /// The nearest slab hit of the segment `start..end` among the objects `mask` selects whose collision is on.
    pub(crate) fn cast(&self, start: F3, end: F3, mask: u32) -> Option<RayHit> {
        let mut best: Option<(f64, RayHit)> = None;
        for o in &self.world.objects {
            if o.view.type_mask & mask == 0 || o.disabled != 0 {
                continue;
            }
            let mut tmin = 0.0f64;
            let mut tmax = 1.0f64;
            let mut axis = usize::MAX;
            let mut sign = 1.0f32;
            let mut ok = true;
            for i in 0..3 {
                let s = d(start[i]);
                let dir = d(end[i]) - s;
                let (lo, hi) = (d(o.view.world_box.min[i]), d(o.view.world_box.max[i]));
                if dir.abs() < 1e-12 {
                    if s < lo || s > hi {
                        ok = false;
                        break;
                    }
                    continue;
                }
                let (mut t1, mut t2) = ((lo - s) / dir, (hi - s) / dir);
                let mut sg = -1.0f32;
                if t1 > t2 {
                    std::mem::swap(&mut t1, &mut t2);
                    sg = 1.0;
                }
                if t1 > tmin {
                    tmin = t1;
                    axis = i;
                    sign = sg;
                }
                tmax = tmax.min(t2);
                if tmin > tmax {
                    ok = false;
                    break;
                }
            }
            if !ok || axis == usize::MAX {
                continue;
            }
            if best.as_ref().is_none_or(|(t, _)| tmin < *t) {
                let mut n = [0.0f32; 3];
                n[axis] = sign;
                let p = [
                    (d(start[0]) + tmin * (d(end[0]) - d(start[0]))) as f32,
                    (d(start[1]) + tmin * (d(end[1]) - d(start[1]))) as f32,
                    (d(start[2]) + tmin * (d(end[2]) - d(start[2]))) as f32,
                ];
                best = Some((
                    tmin,
                    RayHit {
                        object: o.id,
                        point: p,
                        normal: n,
                        type_mask: o.view.type_mask,
                    },
                ));
            }
        }
        best.map(|(_, h)| h)
    }
}

impl ItemHost for TestHost {
    fn world(&mut self) -> &mut dyn World {
        &mut self.world
    }
    fn sim_time(&self) -> u32 {
        self.time
    }
    fn cast_ray(&mut self, start: F3, end: F3, mask: u32) -> Option<RayHit> {
        self.rays.push((start, end, mask));
        self.cast(start, end, mask)
    }
    fn disable_collision(&mut self, id: ObjectId) {
        self.switches.push((id, true));
        self.world.disable_collision(id);
    }
    fn enable_collision(&mut self, id: ObjectId) {
        self.switches.push((id, false));
        self.world.enable_collision(id);
    }
    fn object_velocity(&mut self, id: ObjectId) -> F3 {
        self.vels
            .iter()
            .find(|(v, _)| *v == id)
            .map_or([0.0; 3], |(_, vel)| *vel)
    }
    fn update_container(&mut self, s: &mut ItemState, db: &ItemData) {
        crate::item::no_zones(s, db);
        s.water_coverage = self.coverage;
        s.liquid_type = 7;
    }
    fn moved(&mut self, _s: &mut ItemState) {
        self.moved += 1;
    }
    fn on_collision(&mut self, _s: &mut ItemState, other: ObjectId, vec: F3) -> bool {
        self.item_collisions.push((other, vec));
        self.item_alive
    }
    fn on_enter_liquid(&mut self, _s: &mut ItemState, coverage: f32, liquid_type: u32) {
        self.enters.push((coverage, liquid_type));
    }
    fn on_leave_liquid(&mut self, _s: &mut ItemState, liquid_type: u32) {
        self.leaves.push(liquid_type);
    }
}

impl ProjectileHost for TestHost {
    fn sim_time(&self) -> u32 {
        self.time
    }
    fn cast_ray(&mut self, start: F3, end: F3, mask: u32) -> Option<RayHit> {
        self.rays.push((start, end, mask));
        self.cast(start, end, mask)
    }
    fn set_source_collision(&mut self, source: ObjectId, disabled: bool) {
        self.events.push(Ev::Source {
            id: source,
            disabled,
        });
    }
    fn moved(&mut self, _s: &mut ProjectileState) {
        self.moved += 1;
    }
    fn on_collision(&mut self, s: &mut ProjectileState, col: ObjectId, point: F3, normal: F3) {
        self.events.push(Ev::Collision {
            col,
            point,
            normal,
            vel: s.velocity,
            fade: s.fade,
        });
    }
    fn on_explode(&mut self, _s: &mut ProjectileState, pos: F3, fade: f32) {
        self.events.push(Ev::Explode {
            pos,
            fade,
            time: self.time,
        });
    }
    fn schedule_delete(&mut self, id: ObjectId, at: u32) {
        self.events.push(Ev::ScheduleDelete { id, at });
    }
    fn delete_object(&mut self, id: ObjectId) {
        self.events.push(Ev::Delete(id));
    }
}
