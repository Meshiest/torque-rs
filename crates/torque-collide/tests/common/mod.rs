//! A small world for the tests: objects in a list, `findObjects` in insertion order, and a budget;
//! and three object classes of a test game, built through the [`ConvexBuilder`] seam.

#![allow(dead_code)]

use std::rc::Rc;

use torque_collide::convex::{
    BOX_CONVEX_TYPE, ConvexBuilder, ConvexKind, ConvexSource, HullClass, HullSlot, ObjectView,
    WorkingSet, World, query_in_object,
};
use torque_collide::math::{Box3, F3, IDENTITY, Matrix, d, inverse, mul_box};
use torque_collide::mesh::Hull;
use torque_collide::{Exhausted, ObjectId, work};

/// A test class whose collision is a list of boxes `(offset, size)` around its object box's centre: when the query (in the
/// object's space) meets the object box and the object has no box here yet, one ortho box per entry, centred at
/// `f32(offset + f32((min + max) * 0.5))` with half sizes `f32(size * 0.5)`, each charged a convex.
#[derive(Debug, Clone, PartialEq)]
pub struct TestBoxes {
    pub boxes: Vec<(F3, F3)>,
}

impl ConvexBuilder for TestBoxes {
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
        let ob = view.obj_box;
        let c = |k: usize| ((d(ob.min[k]) + d(ob.max[k])) * 0.5) as f32;
        let center = [c(0), c(1), c(2)];
        for &(off, size) in &self.boxes {
            if !world.charge(work::CONVEX) {
                return Err(Exhausted);
            }
            let at = |k: usize| (d(off[k]) + d(center[k])) as f32;
            let half = |k: usize| (d(size[k]) * 0.5) as f32;
            let kind = ConvexKind::OrthoBox {
                center: [at(0), at(1), at(2)],
                size: [half(0), half(1), half(2)],
            };
            if !set.add(id, kind) {
                break;
            }
        }
        Ok(())
    }
}

/// The hull class of [`TestHulls`]: a type past Torque's own, and a slot whose mesh is gone collides as its bounds.
pub const TEST_HULL: HullClass = HullClass {
    type_id: 16,
    missing_mesh_as_box: true,
};

/// A test class whose collision is its mesh details, as a ShapeBase's but with no object-box test first.
#[derive(Debug, Clone, PartialEq)]
pub struct TestHulls {
    pub slots: Vec<HullSlot>,
}

impl ConvexBuilder for TestHulls {
    fn build_convex(
        &self,
        set: &mut WorkingSet,
        world: &mut dyn World,
        id: ObjectId,
        view: &ObjectView,
        query: &Box3,
    ) -> Result<(), Exhausted> {
        set.build_hulls(world, id, view, query, &self.slots, TEST_HULL, false)
    }
}

/// A test ground: when the query meets its object box, an ortho box `HALF` wide in x and y under the query's centre
/// (`f32((min + max) * 0.5)` in the world, used as the centre relative to the object), z from the object box; its box already
/// in the set is moved to the new centre instead. A convex that moves in place, through [`WorkingSet::kind_mut`].
#[derive(Debug, Clone, PartialEq)]
pub struct TestGround;

impl TestGround {
    pub const HALF: f32 = 5.0;
}

impl ConvexBuilder for TestGround {
    fn build_convex(
        &self,
        set: &mut WorkingSet,
        world: &mut dyn World,
        id: ObjectId,
        view: &ObjectView,
        query: &Box3,
    ) -> Result<(), Exhausted> {
        let q = query_in_object(view, query);
        let qx = ((d(query.min[0]) + d(query.max[0])) * 0.5) as f32;
        let qy = ((d(query.min[1]) + d(query.max[1])) * 0.5) as f32;
        if !q.overlaps(&view.obj_box) {
            return Ok(());
        }
        if let Some(i) = set.find(BOX_CONVEX_TYPE, id, None) {
            if let Some(ConvexKind::OrthoBox { center, .. } | ConvexKind::Box { center, .. }) =
                set.kind_mut(i)
            {
                center[0] = qx;
                center[1] = qy;
            }
            return Ok(());
        }
        if !world.charge(work::CONVEX) {
            return Err(Exhausted);
        }
        let b = view.obj_box;
        let cz = ((d(b.min[2]) + d(b.max[2])) * 0.5) as f32;
        let sz = (0.5 * (d(b.max[2]) - d(b.min[2]))) as f32;
        set.add(
            id,
            ConvexKind::OrthoBox {
                center: [qx, qy, cz],
                size: [Self::HALF, Self::HALF, sz],
            },
        );
        Ok(())
    }
}

/// A source of one of the test classes.
pub fn custom(b: impl ConvexBuilder + 'static) -> ConvexSource {
    ConvexSource::Custom(Rc::new(b))
}

pub struct Obj {
    pub id: ObjectId,
    pub view: ObjectView,
    pub source: ConvexSource,
    pub hulls: Vec<(u32, Rc<Hull>)>,
}

pub struct MockWorld {
    pub objects: Vec<Obj>,
    pub budget: u64,
    pub spent: u64,
}

impl MockWorld {
    pub fn new() -> MockWorld {
        MockWorld {
            objects: Vec::new(),
            budget: u64::MAX,
            spent: 0,
        }
    }

    pub fn add(&mut self, id: u32, view: ObjectView, source: ConvexSource) {
        self.objects.push(Obj {
            id: ObjectId(id),
            view,
            source,
            hulls: Vec::new(),
        });
    }

    pub fn add_hull(&mut self, id: u32, slot: u32, hull: Hull) {
        if let Some(o) = self.objects.iter_mut().find(|o| o.id == ObjectId(id)) {
            o.hulls.push((slot, Rc::new(hull)));
        }
    }
}

/// An object at `pos` with no rotation, unit scale and object box `obj_box`.
pub fn view_at(pos: F3, obj_box: Box3, mask: u32) -> ObjectView {
    let mut m: Matrix = IDENTITY;
    m[3] = pos[0];
    m[7] = pos[1];
    m[11] = pos[2];
    ObjectView {
        obj_to_world: m,
        world_to_obj: inverse(&m),
        scale: [1.0, 1.0, 1.0],
        obj_box,
        world_box: mul_box(&m, &obj_box),
        world_radius: 1.0,
        type_mask: mask,
        collision_disabled: false,
    }
}

impl World for MockWorld {
    fn object(&self, id: ObjectId) -> Option<ObjectView> {
        self.objects.iter().find(|o| o.id == id).map(|o| o.view)
    }

    fn find_objects(
        &mut self,
        query: &Box3,
        mask: u32,
        exclude: Option<ObjectId>,
        limit: usize,
        out: &mut Vec<ObjectId>,
    ) -> Result<(), Exhausted> {
        for o in &self.objects {
            if out.len() >= limit {
                break;
            }
            if Some(o.id) == exclude || o.view.collision_disabled || o.view.type_mask & mask == 0 {
                continue;
            }
            if o.view.world_box.overlaps(query) {
                out.push(o.id);
            }
        }
        Ok(())
    }

    fn convex_source(&self, id: ObjectId) -> ConvexSource {
        self.objects
            .iter()
            .find(|o| o.id == id)
            .map(|o| o.source.clone())
            .unwrap_or(ConvexSource::None)
    }

    fn hull(&self, id: ObjectId, slot: u32) -> Option<Rc<Hull>> {
        self.objects.iter().find(|o| o.id == id).and_then(|o| {
            o.hulls
                .iter()
                .find(|(s, _)| *s == slot)
                .map(|(_, h)| h.clone())
        })
    }

    fn charge(&mut self, units: u64) -> bool {
        if self.spent.saturating_add(units) > self.budget {
            return false;
        }
        self.spent += units;
        true
    }
}

/// A deterministic generator (splitmix64).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }

    /// A hostile float: ordinary values most of the time, then NaN, the infinities, huge, tiny, zero, negative zero.
    pub fn float(&mut self) -> f32 {
        match self.below(16) {
            0 => f32::NAN,
            1 => f32::INFINITY,
            2 => f32::NEG_INFINITY,
            3 => 1e30,
            4 => -3.4e38,
            5 => 1e-40,
            6 => -0.0,
            7 => 0.0,
            8 => f32::from_bits(self.next() as u32),
            _ => ((self.next() % 20_000) as f32 - 10_000.0) / 100.0,
        }
    }

    /// An ordinary float in -range..range.
    pub fn sane(&mut self, range: f32) -> f32 {
        ((self.next() % 1_000_000) as f32 / 1_000_000.0 * 2.0 - 1.0) * range
    }

    pub fn f3(&mut self) -> F3 {
        [self.float(), self.float(), self.float()]
    }
}
