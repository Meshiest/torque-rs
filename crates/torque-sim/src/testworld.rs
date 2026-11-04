//! A synthetic world for the Item and Projectile tests: axis-aligned boxes (solids that collide through `collide`'s box convexes,
//! and ray-only objects such as a player), a slab ray cast, and the recorders of every host call.

use std::rc::Rc;

use torque_collide::convex::{
    BOX_CONVEX_TYPE, ConvexBuilder, ConvexKind, ConvexSource, WorkingSet, query_in_object,
};
use torque_collide::math::{Box3, F3, IDENTITY, d};
use torque_collide::mesh::Hull;
use torque_collide::{Exhausted, ObjectId, ObjectView, World, types, work};

use crate::item::{ItemData, ItemHost, ItemState};
use crate::projectile::{ProjectileHost, ProjectileState};
use crate::scene::RayHit;

/// A solid's type bits: static world, a static shape.
pub(crate) const SOLID: u32 = types::STATIC | types::STATIC_SHAPE;
/// A player-like shape: a ShapeBase and a Player.
pub(crate) const PLAYER: u32 = types::PLAYER | types::SHAPE_BASE;

/// The test objects' class: one ortho box of `size` at the object box's centre (half sizes `f32(size * 0.5)`), made when the
/// query (in the object's space) meets the object box and the object has no box in the set yet.
#[derive(Debug)]
pub(crate) struct TestBox {
    pub size: F3,
}

impl TestBox {
    /// The source of an object whose world box is `min..max`.
    pub(crate) fn source(min: F3, max: F3) -> ConvexSource {
        let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        ConvexSource::Custom(Rc::new(TestBox { size }))
    }
}

impl ConvexBuilder for TestBox {
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
        let c = |k: usize| (b.min[k] + b.max[k]) * 0.5;
        let half = |k: usize| self.size[k] * 0.5;
        let kind = ConvexKind::OrthoBox {
            center: [c(0), c(1), c(2)],
            size: [half(0), half(1), half(2)],
        };
        set.add(id, kind);
        Ok(())
    }
}

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
    pub objs: Vec<Obj>,
    pub budget: u64,
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
        TestHost {
            objs,
            budget: u64::MAX,
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

    pub(crate) fn obj(&self, id: ObjectId) -> Option<&Obj> {
        self.objs.iter().find(|o| o.id == id)
    }

    /// The nearest slab hit of the segment `start..end` among the objects `mask` selects whose collision is on.
    pub(crate) fn cast(&self, start: F3, end: F3, mask: u32) -> Option<RayHit> {
        let mut best: Option<(f64, RayHit)> = None;
        for o in &self.objs {
            if o.type_mask & mask == 0 || o.disabled != 0 {
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
                let (lo, hi) = (d(o.min[i]), d(o.max[i]));
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
                        type_mask: o.type_mask,
                    },
                ));
            }
        }
        best.map(|(_, h)| h)
    }
}

impl World for TestHost {
    fn object(&self, id: ObjectId) -> Option<ObjectView> {
        let b = self.obj(id)?;
        let c = [
            (b.min[0] + b.max[0]) * 0.5,
            (b.min[1] + b.max[1]) * 0.5,
            (b.min[2] + b.max[2]) * 0.5,
        ];
        let h = [
            ((d(b.max[0]) - d(b.min[0])) * 0.5) as f32,
            ((d(b.max[1]) - d(b.min[1])) * 0.5) as f32,
            ((d(b.max[2]) - d(b.min[2])) * 0.5) as f32,
        ];
        let mut m = IDENTITY;
        m[3] = c[0];
        m[7] = c[1];
        m[11] = c[2];
        let mut wto = IDENTITY;
        wto[3] = -c[0];
        wto[7] = -c[1];
        wto[11] = -c[2];
        Some(ObjectView {
            obj_to_world: m,
            world_to_obj: wto,
            scale: [1.0; 3],
            obj_box: Box3::new([-h[0], -h[1], -h[2]], h),
            world_box: Box3::new(b.min, b.max),
            world_radius: 1.0,
            type_mask: b.type_mask,
            collision_disabled: b.disabled != 0,
        })
    }

    fn find_objects(
        &mut self,
        query: &Box3,
        mask: u32,
        exclude: Option<ObjectId>,
        limit: usize,
        out: &mut Vec<ObjectId>,
    ) -> Result<(), Exhausted> {
        for b in &self.objs {
            if out.len() >= limit {
                break;
            }
            if Some(b.id) != exclude
                && b.disabled == 0
                && b.type_mask & mask != 0
                && Box3::new(b.min, b.max).overlaps(query)
            {
                out.push(b.id);
            }
        }
        Ok(())
    }

    fn convex_source(&self, id: ObjectId) -> ConvexSource {
        let Some(b) = self.obj(id) else {
            return ConvexSource::None;
        };
        if !b.convex {
            return ConvexSource::None;
        }
        TestBox::source(b.min, b.max)
    }

    fn hull(&self, _id: ObjectId, _slot: u32) -> Option<Rc<Hull>> {
        None
    }

    fn charge(&mut self, units: u64) -> bool {
        if self.budget < units {
            self.budget = 0;
            return false;
        }
        self.budget -= units;
        true
    }
}

impl ItemHost for TestHost {
    fn world(&mut self) -> &mut dyn World {
        self
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
        if let Some(o) = self.objs.iter_mut().find(|o| o.id == id) {
            o.disabled += 1;
        }
    }
    fn enable_collision(&mut self, id: ObjectId) {
        self.switches.push((id, false));
        if let Some(o) = self.objs.iter_mut().find(|o| o.id == id)
            && o.disabled > 0
        {
            o.disabled -= 1;
        }
    }
    fn object_velocity(&mut self, id: ObjectId) -> F3 {
        self.obj(id).map_or([0.0; 3], |o| o.vel)
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
