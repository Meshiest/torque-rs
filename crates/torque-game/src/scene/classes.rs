//! The per-class scene answers a host adds for its own scene classes: what an object of the class answers a ray (`castRay`)
//! and a polygon list (`buildPolyList`) with, which convex it gives `collide` (`buildConvex`), what its `setTransform` makes
//! of a matrix, what follows a change of its world box, and whether the orbit camera's eye looks through it.
//!
//! The engine's own classes (`Player`, `Item`, the other `ShapeBase` classes, `Camera`, `PhysicalZone`, ...) answer in
//! [`super::rays`], [`super::container_world`] and [`super::object`]; a class registered here ([`register`]) answers for itself
//! and for its subclasses that have no registration of their own (the most derived registration wins), ahead of those. A class
//! with no registration and no engine answer is a plain `SceneObject`: no hit, no polygons, no convex.
//!
//! The registry is a [`Vm`] extension, so the leaf functions that have only the `Vm` reach it. An object's class is resolved
//! once per class and kept; a registration clears what was kept. Every method is a leaf: no script runs in it, except
//! [`SceneClass::set_transform`], which gets the `Vm` mutably as the engine's `setTransform` does.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use collide::convex::ConvexSource;
use collide::poly::PolyList;
use torque_scene::transform::Matrix;
use ts_vm::sim::{ClassId, ObjId};
use ts_vm::vm::{Vm, VmError};

use super::rays::ObjectHit;
use crate::shapes::{KeptShape, Shapes};

/// What a host's scene class answers. Every method has the plain `SceneObject`'s answer as its default.
pub trait SceneClass {
    /// `castRay` for the world segment `start` to `end`: the fraction and the normal in the world, `None` for a miss.
    /// Charged to the run.
    fn cast_ray(
        &self,
        vm: &Vm,
        id: ObjId,
        start: [f32; 3],
        end: [f32; 3],
    ) -> Result<Option<ObjectHit>, VmError> {
        let _ = (vm, id, start, end);
        Ok(None)
    }

    /// `buildPolyList` into `list`: true when the object added its geometry. Charged.
    fn build_poly_list(
        &self,
        vm: &Vm,
        id: ObjId,
        list: &mut dyn PolyList,
    ) -> Result<bool, VmError> {
        let _ = (vm, id, list);
        Ok(false)
    }

    /// `buildConvex`: which convex the object gives `collide`'s queries.
    fn convex_source(&self, vm: &Vm, shapes: &Shapes, id: ObjId) -> ConvexSource {
        let _ = (vm, shapes, id);
        ConvexSource::None
    }

    /// The shape a collision slot's hull is made from and its node transforms, for an object whose meshes do not move (the
    /// engine animates a `ShapeBase`'s instance instead). `pay` charges the work.
    fn hull_shape(
        &self,
        vm: &Vm,
        shapes: &Shapes,
        id: ObjId,
        pay: &mut dyn FnMut(u64) -> bool,
    ) -> Option<(Rc<KeptShape>, Rc<Vec<Matrix>>)> {
        let _ = (vm, shapes, id, pay);
        None
    }

    /// The class's own `setTransform`: true when it took the matrix (the engine's classes are not asked then).
    fn set_transform(&self, vm: &mut Vm, id: ObjId, matrix: Matrix) -> bool {
        let _ = (vm, id, matrix);
        false
    }

    /// The object's world box was made anew (`resetWorldBox`): an index that holds the object refreshes its place.
    fn world_box_changed(&self, vm: &Vm, id: ObjId) {
        let _ = (vm, id);
    }

    /// Whether the orbit camera's eye cast (`validateEyePoint`) passes through the object when it meets it.
    fn eye_see_through(&self, vm: &Vm, id: ObjId) -> bool {
        let _ = (vm, id);
        false
    }
}

/// The registered classes by name, in the order they were registered.
type Named = Vec<(Vec<u8>, Rc<dyn SceneClass>)>;

/// The registry: the classes by name, and each object class resolved to its answer.
#[derive(Default)]
pub struct SceneClasses {
    named: RefCell<Named>,
    resolved: RefCell<HashMap<ClassId, Option<Rc<dyn SceneClass>>>>,
}

impl SceneClasses {
    fn resolve(&self, vm: &Vm, class: ClassId) -> Option<Rc<dyn SceneClass>> {
        if let Some(found) = self.resolved.borrow().get(&class) {
            return found.clone();
        }
        let classes = &vm.objects().classes;
        let named = self.named.borrow();
        let mut at = Some(class);
        let mut found = None;
        // A chain longer than the class count is a cycle; the registry's classes form none, but the walk is bounded anyway.
        let mut left = classes.len();
        while let Some(c) = at
            && left > 0
        {
            let name = classes.name(c);
            if let Some((_, answer)) = named.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
                found = Some(Rc::clone(answer));
                break;
            }
            at = classes.get(c).and_then(|d| d.parent);
            left -= 1;
        }
        self.resolved.borrow_mut().insert(class, found.clone());
        found
    }
}

/// Registers `answer` for the class `class` and its subclasses (replacing one the class had).
pub fn register(vm: &mut Vm, class: &str, answer: Rc<dyn SceneClass>) {
    let registry = match vm.extension::<SceneClasses>() {
        Some(r) => r,
        None => {
            let r = Rc::new(SceneClasses::default());
            vm.set_extension(Rc::clone(&r));
            r
        }
    };
    let mut named = registry.named.borrow_mut();
    named.retain(|(n, _)| !n.eq_ignore_ascii_case(class.as_bytes()));
    named.push((class.as_bytes().to_vec(), answer));
    registry.resolved.borrow_mut().clear();
}

/// The registered answer for `id`'s class (its own or its nearest ancestor's), if any.
pub fn of(vm: &Vm, id: ObjId) -> Option<Rc<dyn SceneClass>> {
    let class = vm.objects().get(id)?.class();
    vm.extension::<SceneClasses>()?.resolve(vm, class)
}
