//! A world of boxes for the tests of this crate and of the movers built on it (the `testing` feature; only
//! dev-dependencies turn it on): [`BoxWorld`] answers [`World`] from a list of axis-aligned boxes, and [`BoxConvex`] is the
//! class its boxes collide as.

use std::rc::Rc;

use crate::convex::{
    BOX_CONVEX_TYPE, ConvexBuilder, ConvexKind, ConvexSource, ObjectView, WorkingSet, World,
    query_in_object,
};
use crate::math::{Box3, F3};
use crate::mesh::Hull;
use crate::{Exhausted, ObjectId, work};

/// The collision of an axis-aligned box: one ortho box of `size` at the object box's center, made when the query (in the
/// object's space) meets the object box and the object has no box in the set yet; it is charged one convex.
#[derive(Debug)]
pub struct BoxConvex {
    /// The box's full size; the convex's half sizes are `f32(size * 0.5)`.
    pub size: F3,
}

impl BoxConvex {
    /// The source of an object whose world box is `min..max`.
    #[must_use]
    pub fn source(min: F3, max: F3) -> ConvexSource {
        let size = std::array::from_fn(|i| max[i] - min[i]);
        ConvexSource::Custom(Rc::new(BoxConvex { size }))
    }
}

impl ConvexBuilder for BoxConvex {
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
        set.add(
            id,
            ConvexKind::OrthoBox {
                center: [c(0), c(1), c(2)],
                size: [half(0), half(1), half(2)],
            },
        );
        Ok(())
    }
}

/// One object of a [`BoxWorld`].
#[derive(Debug, Clone)]
pub struct BoxObject {
    /// The object's id.
    pub id: ObjectId,
    /// The object as the world answers for it.
    pub view: ObjectView,
    /// Which `buildConvex` it has.
    pub source: ConvexSource,
    /// Its collision meshes by slot.
    pub hulls: Vec<(u32, Rc<Hull>)>,
    /// The collision-disabled count ([`BoxWorld::disable_collision`]); the object is skipped while it is not zero.
    pub disabled: i32,
}

/// A [`World`] of objects in a list: `find_objects` walks them in insertion order, skipping disabled ones and `exclude`, and the
/// work budget is a number that runs down.
#[derive(Debug, Clone)]
pub struct BoxWorld {
    /// The objects, in the order `find_objects` meets them.
    pub objects: Vec<BoxObject>,
    /// The work units left; a charge it cannot pay empties it, so every later charge fails too.
    pub budget: u64,
    /// Every `find_objects` is charged [`work::CANDIDATE`] per object in the world, as a client's world charges its
    /// candidates.
    pub charge_scan: bool,
}

impl Default for BoxWorld {
    fn default() -> BoxWorld {
        BoxWorld {
            objects: Vec::new(),
            budget: u64::MAX,
            charge_scan: false,
        }
    }
}

impl BoxWorld {
    /// An empty world with an unlimited budget.
    #[must_use]
    pub fn new() -> BoxWorld {
        BoxWorld::default()
    }

    /// Adds an object with any view and source.
    pub fn add(&mut self, id: ObjectId, view: ObjectView, source: ConvexSource) -> &mut BoxObject {
        self.objects.push(BoxObject {
            id,
            view,
            source,
            hulls: Vec::new(),
            disabled: 0,
        });
        self.objects.last_mut().expect("just pushed")
    }

    /// Adds the box `min..max` with the type bits `type_mask`, colliding as a [`BoxConvex`].
    pub fn add_box(&mut self, id: ObjectId, min: F3, max: F3, type_mask: u32) -> &mut BoxObject {
        self.add(
            id,
            ObjectView::from_box(min, max, type_mask),
            BoxConvex::source(min, max),
        )
    }

    /// Gives the object `id` the mesh `hull` in collision slot `slot`; nothing when there is no such object.
    pub fn add_hull(&mut self, id: ObjectId, slot: u32, hull: impl Into<Rc<Hull>>) {
        if let Some(o) = self.get_mut(id) {
            o.hulls.push((slot, hull.into()));
        }
    }

    /// The object `id`.
    #[must_use]
    pub fn get(&self, id: ObjectId) -> Option<&BoxObject> {
        self.objects.iter().find(|o| o.id == id)
    }

    /// The object `id`, to change.
    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut BoxObject> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    /// Raises the collision-disabled count of the object `id`.
    pub fn disable_collision(&mut self, id: ObjectId) {
        if let Some(o) = self.get_mut(id) {
            o.disabled += 1;
        }
    }

    /// Lowers the collision-disabled count of the object `id` (it stops at zero).
    pub fn enable_collision(&mut self, id: ObjectId) {
        if let Some(o) = self.get_mut(id) {
            o.disabled = (o.disabled - 1).max(0);
        }
    }
}

impl World for BoxWorld {
    fn object(&self, id: ObjectId) -> Option<ObjectView> {
        let o = self.get(id)?;
        Some(ObjectView {
            collision_disabled: o.view.collision_disabled || o.disabled != 0,
            ..o.view
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
        if self.charge_scan && !self.charge(work::CANDIDATE * self.objects.len() as u64) {
            return Err(Exhausted);
        }
        for o in &self.objects {
            if out.len() >= limit {
                break;
            }
            let skipped = Some(o.id) == exclude
                || o.disabled != 0
                || o.view.collision_disabled
                || o.view.type_mask & mask == 0;
            if !skipped && o.view.world_box.overlaps(query) {
                out.push(o.id);
            }
        }
        Ok(())
    }

    fn convex_source(&self, id: ObjectId) -> ConvexSource {
        self.get(id)
            .map_or(ConvexSource::None, |o| o.source.clone())
    }

    fn hull(&self, id: ObjectId, slot: u32) -> Option<Rc<Hull>> {
        self.get(id)?
            .hulls
            .iter()
            .find(|(s, _)| *s == slot)
            .map(|(_, h)| Rc::clone(h))
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
