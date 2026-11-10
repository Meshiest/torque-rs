//! The server container as `collide`'s [`World`]: the seam `sim`'s Player and AIPlayer and `vehicle`'s movers
//! run their collision queries through. It answers from the container's spatial indexes and its bins (every other scene
//! object), in `Container::findObjects`'s order, reads each object's state as it is at query time, and builds a convex's
//! meshes at the object's current pose.
//!
//! - [`World::object`] is the `SceneObject` fields: `mObjToWorld`, `mWorldToObj` (the affine inverse), the scale, both boxes,
//!   the world radius, the type mask, the collision-disable count
//! - [`World::find_objects`] is `Container::findObjects`: each index, then the bins; an object whose collision is disabled is
//!   skipped, and `exclude` too (the mover, disabled around its own query)
//! - [`World::order_candidates`] is the container's candidate order ([`super::container::CandidateOrder`]), each compare
//!   charged; none keeps the container's
//! - [`World::convex_source`] is which `buildConvex` the class has: a registered class's own ([`super::classes`]),
//!   `Player::buildConvex`, `ShapeBase::buildConvex`, `PhysicalZone`'s, `Item`'s
//! - [`World::hull`] is a collision slot's meshes at the current pose: a `ShapeBase`'s after
//!   `TSShapeInstance::animate(detail)`, a registered class's from its own [`super::classes::SceneClass::hull_shape`]
//! - [`World::charge`] is the run's budget ([`Vm::charge_work`])
//! - the plugins' collision sources ([`crate::plugin_collision`]) answer for the ids in their range
//!   ([`collide::source::is_plugin_id`]): their shapes as they are at the phase's first query, listed after the bins
//!
//! # The borrow contract (read this before holding one)
//!
//! A [`ContainerWorld`] holds the `Vm` **exclusively** (`&mut Vm`, kept in a `RefCell` so that `hull`, which takes `&self`, can
//! animate an instance) and borrows the container's bins for one method call at a time. So:
//!
//! - **Build it for one query or one collision phase and drop it before any script runs.** A host callback that runs script
//!   (`onCollision`, `onImpact`, `onEnterLiquid`, ...) needs the `Vm`, which the borrow checker will not lend while a
//!   `ContainerWorld` lives: end the phase (drop it), run the callback, then build a new one. It is cheap to build: three
//!   references and two empty caches.
//! - **Nothing inside it runs script**, so its own `RefCell` is never borrowed twice: every method takes the borrow it needs
//!   and gives it back before it returns, and no method calls another.
//! - The hulls it hands out are cached for its life: the pose of an object does not change while no script and no tick runs.
//!   Beyond it, a kept shape keeps the hulls of each detail by the poses of their parts ([`BuiltHulls`]), so scenery at rest
//!   is made into a hull once, whatever phase meets it; a phase pays the same charges whether it makes the hull or finds it.
//!
//! **Budget.** Every walk, candidate, compare, hull and accelerator is charged before it is made; once the budget refuses,
//! every later [`World::charge`] answers false and [`ContainerWorld::exhausted`] says so (the caller ends the run with the step
//! limit).
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree:
//! `scene/sceneContainer.cpp` (Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use collide::convex::{ConvexSource, HullSlot, ObjectView};
use collide::math::Box3;
use collide::mesh::{Hull, HullAccel};
use collide::source::{ShapeSet, is_plugin_id};
use collide::{Exhausted, ObjectId, World};
use shapes::instance::ShapeInstance;
use shapes::preload::SLOTS;
use torque_scene::transform as grid;
use ts_vm::sim::ObjId;
use ts_vm::vm::Vm;

use super::access::{matrix_of, scale_of};
use super::camera::is_class;
use super::container::{Container, SceneView};
use super::ext;
use super::rays::{RayClass, detail_objects, detail_parts, node_of, ray_class};
use super::shapebase;
use crate::shapes::{KeptShape, Shapes};

/// Work per compare of the search sort (as `torque_scene::query`'s sort).
const COMPARE_WORK: u64 = torque_scene::query::COMPARE_WORK;

/// The hulls a phase handed out, by object and slot (`None` for a slot with none).
type HullsBySlot = HashMap<(u32, u32), Option<Rc<Hull>>>;

/// The server container for a collision phase (see the module's borrow contract).
pub struct ContainerWorld<'a> {
    vm: RefCell<&'a mut Vm>,
    container: &'a Container,
    shapes: &'a Shapes,
    exhausted: Cell<bool>,
    /// The hulls handed out in this phase, by object and slot.
    hulls: RefCell<HullsBySlot>,
    /// The shapes of the plugins' collision sources as they are at the phase's first query ([`crate::plugin_collision`]).
    plugin: OnceCell<Rc<ShapeSet>>,
}

impl<'a> ContainerWorld<'a> {
    /// The server container for a phase that holds the `Vm`. The queries use the math tables installed for the process
    /// (`torque_collide::library`, `torque_shapes::library`): a host installs its own when it builds its server, before the
    /// first world; without one they are the plain ones.
    ///
    /// The scene hierarchy's children are written first ([`super::links::resolve`]), so the phase meets them where their
    /// parents are now.
    pub fn new(vm: &'a mut Vm, container: &'a Container, shapes: &'a Shapes) -> ContainerWorld<'a> {
        super::links::resolve(vm);
        ContainerWorld {
            vm: RefCell::new(vm),
            container,
            shapes,
            exhausted: Cell::new(false),
            hulls: RefCell::new(HashMap::new()),
            plugin: OnceCell::new(),
        }
    }

    /// Whether the budget refused a charge in this phase (the caller ends the run with the step limit).
    pub fn exhausted(&self) -> bool {
        self.exhausted.get()
    }

    /// Ends the phase and gives the `Vm` back (before a script runs: the borrow contract above).
    pub fn into_vm(self) -> &'a mut Vm {
        self.vm.into_inner()
    }

    /// A read of the `Vm` during the phase (no script, no write).
    pub fn with_vm<R>(&self, f: impl FnOnce(&Vm) -> R) -> R {
        f(&self.vm.borrow())
    }

    /// The plugin shapes of this phase: made at the first query that asks, empty when the `Vm` has no source. Charged to no
    /// budget, as the client's are not (see [`collide::source`]).
    fn plugin_shapes(&self) -> &ShapeSet {
        self.plugin.get_or_init(|| {
            let vm: &Vm = &self.vm.borrow();
            crate::plugin_collision::sources_of(vm).map_or_else(ShapeSet::none, |s| s.snapshot())
        })
    }

    fn pay(&self, vm: &Vm, units: u64) -> bool {
        if self.exhausted.get() {
            return false;
        }
        let paid = vm.charge_work(units);
        if !paid {
            self.exhausted.set(true);
        }
        paid
    }
}

/// The scene state of an object as a `collide` view, `None` once it is gone or for an object with no scene state.
pub fn object_view(vm: &Vm, id: ObjId) -> Option<ObjectView> {
    let e = ext::get(vm.objects(), id)?;
    let scene = e.scene?;
    // A map object answers the view its collision was made with (the client's, from the same record).
    if let Some(m) = &e.map {
        return Some(ObjectView {
            collision_disabled: e.collision_off != 0,
            ..m.view
        });
    }
    let matrix = matrix_of(vm, id)?;
    Some(ObjectView {
        obj_to_world: matrix,
        world_to_obj: grid::affine_inverse(&matrix),
        scale: scale_of(vm, id),
        obj_box: Box3 {
            min: scene.object_min,
            max: scene.object_max,
        },
        world_box: Box3 {
            min: scene.world_min,
            max: scene.world_max,
        },
        world_radius: scene.world_radius,
        type_mask: e.type_mask,
        collision_disabled: e.collision_off != 0,
    })
}

/// The eight collision slots of a shape as `collide` takes them ([`HullSlot::of_shape`]). Made once per kept shape (every
/// working-set update of every object of the shape asks).
pub fn hull_slots(kept: &KeptShape) -> Rc<[HullSlot]> {
    let slots = kept.hulls.slots.get_or_init(|| match kept.facts.as_ref() {
        Some(facts) => HullSlot::of_shape(SLOTS, &facts.slots.collision, &facts.bounds),
        None => Rc::from([]),
    });
    Rc::clone(slots)
}

/// The hulls made of one detail of a kept shape ([`crate::shapes::HullCache::built`]): what making one charged (the same at
/// every pose: the parts' vertex and polygon counts), the shape objects its parts come from, and the last [`BUILT_POSES`]
/// hulls, each with the node transforms of its parts. A collision phase that meets the shape at one of those poses pays the
/// same charges, in the same order, and takes the hull; at another it makes one, paying the same.
#[derive(Debug, Clone, Default)]
pub struct BuiltHulls {
    charges: Vec<u64>,
    objects: Vec<usize>,
    /// Oldest first.
    hulls: VecDeque<(Vec<shapes::math::Matrix>, Rc<Hull>)>,
}

/// Poses kept a detail: a shape at rest (most scenery) needs one, an animated one a few.
pub const BUILT_POSES: usize = 4;

impl BuiltHulls {
    /// The hull made at the part poses `poses` gives, if one was.
    fn find(&self, poses: impl Fn(usize) -> shapes::math::Matrix) -> Option<Rc<Hull>> {
        let same = |a: &shapes::math::Matrix, b: &shapes::math::Matrix| {
            a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
        };
        self.hulls
            .iter()
            .find(|(nodes, _)| {
                nodes
                    .iter()
                    .zip(&self.objects)
                    .all(|(n, &object)| same(n, &poses(object)))
            })
            .map(|(_, h)| Rc::clone(h))
    }

    /// Keeps `hull` at its parts' poses, letting the oldest go past [`BUILT_POSES`].
    fn keep(&mut self, hull: &Rc<Hull>) {
        if self.hulls.len() >= BUILT_POSES {
            self.hulls.pop_front();
        }
        let nodes = hull.parts.iter().map(|p| p.node).collect();
        self.hulls.push_back((nodes, Rc::clone(hull)));
    }
}

/// The accelerator of a detail (pose-free: `computeAccelerator` reads the meshes at the identity), made once per kept shape and
/// detail and charged when made.
fn accel_of(
    kept: &KeptShape,
    detail: i32,
    parts: &[collide::mesh::MeshPart],
    pay: &mut dyn FnMut(u64) -> bool,
) -> Option<Rc<HullAccel>> {
    if let Some(a) = kept.hulls.accels.borrow().get(&detail) {
        return Some(Rc::clone(a));
    }
    let verts: u64 = parts.iter().map(|p| p.verts.len() as u64).sum();
    let tris: u64 = parts.iter().map(|p| p.tris.len() as u64).sum();
    let verts = verts.min(collide::mesh::MAX_HULL_VERTS as u64);
    let tris = tris.min(collide::mesh::MAX_HULL_TRIS as u64);
    // `O(V * F^2)` at worst (the caps bound both).
    if !pay(collide::work::FEATURE
        .saturating_mul(verts.saturating_mul(tris).saturating_mul(tris.max(1))))
    {
        return None;
    }
    let a = Rc::new(HullAccel::compute(parts));
    kept.hulls.accels.borrow_mut().insert(detail, Rc::clone(&a));
    Some(a)
}

/// The node transforms of a shape at rest (an instance made with no thread), kept with the shape.
pub fn rest_nodes(
    kept: &KeptShape,
    pay: &mut dyn FnMut(u64) -> bool,
) -> Option<Rc<Vec<shapes::math::Matrix>>> {
    if let Some(n) = kept.hulls.rest.borrow().as_ref() {
        return Some(Rc::clone(n));
    }
    let shape = kept.shape.as_ref()?;
    let instance = ShapeInstance::new(shape, pay).ok()?;
    let nodes = Rc::new(instance.node_transforms().to_vec());
    *kept.hulls.rest.borrow_mut() = Some(Rc::clone(&nodes));
    Some(nodes)
}

impl World for ContainerWorld<'_> {
    fn object(&self, id: ObjectId) -> Option<ObjectView> {
        if is_plugin_id(id) {
            return self.plugin_shapes().object(id);
        }
        object_view(&self.vm.borrow(), id.0)
    }

    fn find_objects(
        &mut self,
        query: &Box3,
        mask: u32,
        exclude: Option<ObjectId>,
        limit: usize,
        out: &mut Vec<ObjectId>,
    ) -> Result<(), Exhausted> {
        if self.exhausted.get() {
            return Err(Exhausted);
        }
        let q = [
            query.min[0],
            query.min[1],
            query.min[2],
            query.max[0],
            query.max[1],
            query.max[2],
        ];
        let vm: &Vm = &self.vm.borrow();
        let exhausted = &self.exhausted;
        let mut charge = |units: u64| {
            let paid = !exhausted.get() && vm.charge_work(units);
            if !paid {
                exhausted.set(true);
            }
            paid
        };
        let skip = |id: u32| exclude == Some(ObjectId(id));
        for index in self.container.indexes() {
            index
                .find(vm, q, mask, &mut charge, &mut |object, _| {
                    if out.len() < limit && !skip(object) {
                        out.push(ObjectId(object));
                    }
                })
                .map_err(|_| Exhausted)?;
            if out.len() >= limit {
                return Ok(());
            }
        }
        let view = SceneView::new(vm);
        self.container
            .bins()
            .find(q, mask, &view, &mut charge, &mut |id| {
                if !skip(id) {
                    out.push(ObjectId(id));
                }
                out.len() < limit
            })
            .map_err(|_| Exhausted)?;
        if out.len() >= limit {
            return Ok(());
        }
        // The plugins' shapes come last, as the client's world lists them.
        let plugin = self.plugin_shapes();
        if plugin.is_empty() {
            return Ok(());
        }
        plugin.find(query, mask, exclude, limit, out);
        Ok(())
    }

    /// The container's candidate order ([`super::container::CandidateOrder`], each compare charged), else the container's own.
    fn order_candidates(&mut self, ids: &mut [ObjectId]) {
        let Some(order) = self.container.candidate_order().cloned() else {
            return;
        };
        let vm: &Vm = &self.vm.borrow();
        let exhausted = &self.exhausted;
        let mut charge = || {
            let paid = !exhausted.get() && vm.charge_work(COMPARE_WORK);
            if !paid {
                exhausted.set(true);
            }
            paid
        };
        order.order(&*self, ids, &mut charge);
    }

    fn convex_source(&self, id: ObjectId) -> ConvexSource {
        if is_plugin_id(id) {
            return self.plugin_shapes().convex_source(id);
        }
        let vm: &Vm = &self.vm.borrow();
        let id = id.0;
        if vm.objects().get(id).is_none() {
            return ConvexSource::None;
        }
        if let Some(m) = super::ext::map_of(vm, id) {
            return if m.shape.is_some() {
                ConvexSource::Map
            } else {
                ConvexSource::None
            };
        }
        let class = ray_class(vm, id);
        if let RayClass::Custom(answer) = &class {
            return answer.convex_source(vm, self.shapes, id);
        }
        if is_class(vm, id, "PhysicalZone") {
            return ConvexSource::Zone;
        }
        let body = shapebase::state(vm, id).and_then(|s| s.body.as_ref());
        match class {
            RayClass::Custom(_) => ConvexSource::None,
            // `Player::buildConvex` builds none while the Player is mounted (`sim::mount`'s link).
            RayClass::Player => crate::sim_classes::sim_classes(vm).player_convex(
                crate::sim::mount::mount_of(vm, id).is_some(),
                body.is_some(),
            ),
            RayClass::Item => ConvexSource::Item {
                has_shape: body.is_some(),
            },
            RayClass::Shape => ConvexSource::Shape {
                has_shape: body.is_some(),
                slots: body.map_or_else(|| Rc::from([]), |b| hull_slots(&b.kept)),
            },
            RayClass::None => ConvexSource::None,
        }
    }

    fn hull(&self, id: ObjectId, slot: u32) -> Option<Rc<Hull>> {
        if is_plugin_id(id) {
            return self.plugin_shapes().hull(id, slot);
        }
        if let Some(h) = self.hulls.borrow().get(&(id.0, slot)) {
            return h.clone();
        }
        let made = self.make_hull(id.0, slot);
        self.hulls.borrow_mut().insert((id.0, slot), made.clone());
        made
    }

    fn charge(&mut self, units: u64) -> bool {
        let vm: &Vm = &self.vm.borrow();
        self.pay(vm, units)
    }

    fn map_shape(&self, id: ObjectId) -> Option<Rc<collide::map::MapShape>> {
        super::ext::map_of(&self.vm.borrow(), id.0)?.shape.clone()
    }
}

impl ContainerWorld<'_> {
    /// The collision slot `slot` of the object at its current pose (see [`World::hull`]).
    fn make_hull(&self, id: u32, slot: u32) -> Option<Rc<Hull>> {
        let slot = usize::try_from(slot).ok().filter(|&s| s < SLOTS)?;
        let custom = super::classes::of(&self.vm.borrow(), id);
        if let Some(answer) = custom {
            let vm: &Vm = &self.vm.borrow();
            let mut pay = |u: u64| self.pay(vm, u);
            let (kept, nodes) = answer.hull_shape(vm, self.shapes, id, &mut pay)?;
            return self.hull_at(vm, &kept, &nodes, slot);
        }
        let detail = {
            let vm: &Vm = &self.vm.borrow();
            let body = shapebase::state(vm, id)?.body.as_ref()?;
            *body.kept.facts.as_ref()?.slots.collision.get(slot)?
        };
        let at = usize::try_from(detail).ok()?;
        // `TSShapeInstance::animate(detail)` before the meshes are read (no script runs in it).
        if shapebase::animate(&mut self.vm.borrow_mut(), id, at).is_err() {
            self.exhausted.set(true);
            return None;
        }
        let vm: &Vm = &self.vm.borrow();
        let body = shapebase::state(vm, id)?.body.as_ref()?;
        self.hull_at(vm, &body.kept, body.instance.node_transforms(), slot)
    }

    /// The hull of `kept`'s collision slot `slot` at the node transforms `nodes`: the one made at that pose when the kept shape
    /// has it ([`BuiltHulls`]), else made now. Either way a build's charges are paid, in a build's order, so the budget runs out
    /// at the same charge whether the hull is made or found.
    fn hull_at(
        &self,
        vm: &Vm,
        kept: &KeptShape,
        nodes: &[shapes::math::Matrix],
        slot: usize,
    ) -> Option<Rc<Hull>> {
        let detail = *kept.facts.as_ref()?.slots.collision.get(slot)?;
        if detail < 0 {
            return None;
        }
        let shape = kept.shape.as_ref()?;
        let mut pay = |u: u64| self.pay(vm, u);
        let mut kept_hulls = kept.hulls.built.borrow_mut();
        let found = kept_hulls.get(&detail);
        if let Some(built) = found {
            if !built.charges.iter().all(|&u| pay(u)) {
                return None;
            }
            if let Some(hull) = built.find(|object| node_of(nodes, shape, object)) {
                return Some(hull);
            }
        }
        // A build: its charges paid here unless a found entry paid them above, and recorded for the entry it makes.
        let first = found.is_none();
        let mut charges = Vec::new();
        let parts = detail_parts(shape, nodes, detail, &mut |u| {
            if !first {
                return true;
            }
            charges.push(u);
            pay(u)
        })
        .ok()?;
        let accel = accel_of(kept, detail, &parts, &mut pay)?;
        let hull = Rc::new(Hull {
            node: parts.first().map(|p| p.node),
            parts,
            accel: Some(accel),
        });
        if first {
            let objects = part_objects(shape, detail)?;
            kept_hulls.insert(
                detail,
                BuiltHulls {
                    charges,
                    objects,
                    ..BuiltHulls::default()
                },
            );
        }
        kept_hulls.get_mut(&detail)?.keep(&hull);
        Some(hull)
    }
}

/// The objects the parts of `detail` come from: those of the detail with a mesh, as `detail_parts` takes them.
fn part_objects(shape: &shapes::shape::Shape, detail: i32) -> Option<Vec<usize>> {
    let objects = detail_objects(shape, detail, &mut |_| true).ok()?;
    Some(
        objects
            .into_iter()
            .filter(|&(_, mesh)| matches!(shape.meshes.get(mesh), Some(Some(_))))
            .map(|(object, _)| object)
            .collect(),
    )
}

#[cfg(test)]
mod built_tests {
    use super::*;

    /// A hull of one part at the node transform that moves by `x`.
    fn hull_at_node(x: f32) -> Rc<Hull> {
        let mut node = shapes::math::IDENTITY;
        node[3] = x;
        Rc::new(Hull {
            node: Some(node),
            parts: vec![collide::mesh::MeshPart {
                node,
                ..Default::default()
            }],
            accel: None,
        })
    }

    #[test]
    fn a_hull_is_found_only_at_the_pose_of_its_parts_and_the_oldest_pose_goes_first() {
        let mut built = BuiltHulls {
            objects: vec![3],
            ..BuiltHulls::default()
        };
        let pose = |x: f32| {
            move |object: usize| {
                assert_eq!(object, 3, "asked for the part's object");
                let mut m = shapes::math::IDENTITY;
                m[3] = x;
                m
            }
        };
        built.keep(&hull_at_node(1.0));
        assert!(built.find(pose(1.0)).is_some());
        // Another pose of the part (an animated shape), and a pose that differs only in its zero's sign, find nothing.
        assert!(built.find(pose(2.0)).is_none());
        built.keep(&hull_at_node(0.0));
        assert!(built.find(pose(-0.0)).is_none());
        assert!(built.find(pose(0.0)).is_some());
        // Past BUILT_POSES the oldest pose is the one let go.
        for k in 2..=BUILT_POSES {
            built.keep(&hull_at_node(k as f32 * 10.0));
        }
        assert!(built.find(pose(1.0)).is_none());
        assert!(built.find(pose(0.0)).is_some());
        assert_eq!(built.hulls.len(), BUILT_POSES);
    }
}
