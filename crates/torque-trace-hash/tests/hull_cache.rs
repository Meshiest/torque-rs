//! The server's hull cache (`torque_game::scene::container_world::BuiltHulls`): whatever happens to an object between two
//! collision phases (moved, scaled, given another shape, deleted and another made), the hull a phase meets is the one a build
//! from nothing makes, and a phase that finds a hull charges what making it charges, refused at the same charge.

use std::rc::Rc;

use torque_collide::mesh::Hull;
use torque_collide::{ObjectId, World};
use torque_game::scene::container_world::ContainerWorld;
use torque_trace_hash::rig::{Control, Rig};
use torque_trace_hash::scenarios::Baseline;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

/// A server of the trace world with two more StaticShapes of the floor's cube (`A` and `B`) and a datablock of another shape.
fn rig() -> Rig {
    let mut rig = Rig::new(Baseline, Control::Figure).expect("the rig");
    for _ in 0..4 {
        rig.step();
    }
    eval(
        &mut rig,
        "datablock StaticShapeData(TraceBuggyShape) { shapeFile = \"kit/buggy.dts\"; };
         new StaticShape(A) { dataBlock = TraceFloorData; position = \"20 0 0\"; };
         new StaticShape(B) { dataBlock = TraceFloorData; position = \"-20 0 0\"; scale = \"2 2 2\"; };",
    );
    rig
}

fn eval(rig: &mut Rig, script: &str) {
    rig.server
        .vm_mut()
        .eval(script.as_bytes())
        .expect("the script runs");
}

fn id(vm: &Vm, name: &str) -> ObjId {
    vm.objects().find(name.as_bytes()).expect("the object")
}

/// The hull of `name`'s first collision slot in a phase of its own, with what the phase charged; `budget` caps the charges.
fn hull(rig: &mut Rig, name: &str, budget: u64) -> (Option<Rc<Hull>>, u64) {
    let vm = rig.server.vm_mut();
    let object = id(vm, name);
    let env = torque_game::sim::env_of(vm, id(vm, "TraceFigure")).expect("the figure ticks");
    let saved = vm.begin_host_budget_with_runs(budget, u64::MAX);
    let made = {
        let world = ContainerWorld::new(vm, &env.container, &env.shapes);
        world.hull(ObjectId(object), 0)
    };
    let spent = vm.end_host_budget(saved);
    (made, spent)
}

/// Forgets the hulls the kept shape of `name` made (its accelerators stay, as they do once made).
fn forget(rig: &mut Rig, name: &str) {
    let vm = rig.server.vm_mut();
    let object = id(vm, name);
    let body = torque_game::scene::shapebase::state(vm, object)
        .and_then(|s| s.body.as_ref())
        .expect("a shape");
    body.kept.hulls.built.borrow_mut().clear();
}

/// The hull and charges of a build from nothing (after `forget`), and of the phase after it, which finds the hull.
fn fresh_then_found(rig: &mut Rig, name: &str, budget: u64) -> [(Option<Rc<Hull>>, u64); 2] {
    forget(rig, name);
    let fresh = hull(rig, name, budget);
    let found = hull(rig, name, budget);
    [fresh, found]
}

#[test]
fn a_hull_found_is_the_hull_a_build_makes_whatever_happened_to_the_object() {
    let mut rig = rig();
    // The first build of the cube makes its accelerator too; every later build charges the same.
    let (first, _) = hull(&mut rig, "A", u64::MAX);
    assert!(first.is_some());
    let check = |rig: &mut Rig, name: &str| {
        let [(fresh, paid), (found, again)] = fresh_then_found(rig, name, u64::MAX);
        let (fresh, found) = (fresh.expect("a hull"), found.expect("a hull"));
        assert_eq!(*fresh, *found, "{name}: the hull found is the one made");
        assert!(Rc::ptr_eq(&fresh, &found), "{name}: and it is shared");
        assert_eq!(paid, again, "{name}: the charges are a build's");
        assert!(paid > 0);
        fresh
    };
    let a = check(&mut rig, "A");
    // Another object of the shape at rest shares it, wherever it is and whatever its scale (a hull is in object space).
    let b = check(&mut rig, "B");
    assert_eq!(*a, *b);
    // Moved and scaled.
    eval(
        &mut rig,
        "A.setTransform(\"5 5 3 0 0 1 0.5\"); A.setScale(\"3 1 2\");",
    );
    let moved = check(&mut rig, "A");
    assert_eq!(*moved, *a);
    // Deleted, and another object of that name made of another shape (with whatever id it is given): the buggy's hull,
    // never the cube's; then again of the cube: the cube's, never the buggy's.
    eval(
        &mut rig,
        "A.delete(); new StaticShape(A) { dataBlock = TraceBuggyShape; position = \"20 0 0\"; };",
    );
    let reshaped = check(&mut rig, "A");
    assert_ne!(*reshaped, *a, "the other shape's hull");
    eval(
        &mut rig,
        "A.delete(); new StaticShape(A) { dataBlock = TraceFloorData; position = \"20 0 0\"; };",
    );
    let again = check(&mut rig, "A");
    assert_eq!(*again, *a);
    rig.step();
    assert_eq!(*hull(&mut rig, "A", u64::MAX).0.expect("a hull"), *a);
}

#[test]
fn a_budget_refuses_a_found_hull_at_the_charge_it_refuses_a_build() {
    let mut rig = rig();
    let (_, whole) = hull(&mut rig, "B", u64::MAX);
    let [(_, paid), _] = fresh_then_found(&mut rig, "B", u64::MAX);
    assert!(paid > 1 && paid <= whole);
    for budget in [0, 1, paid / 3, paid / 2, paid - 1, paid, paid + 1] {
        let [(fresh, a), (found, b)] = fresh_then_found(&mut rig, "B", budget);
        // `fresh_then_found` forgot the hull, so with a short budget neither phase finds one: both build, both are refused.
        assert_eq!(fresh.is_some(), budget >= paid, "{budget}");
        assert_eq!((fresh.is_some(), a), (found.is_some(), b), "{budget}");
        // With the hull kept, a short budget refuses the phase that finds it at the same charge.
        if budget >= paid {
            continue;
        }
        let _ = hull(&mut rig, "B", u64::MAX);
        let (kept, c) = hull(&mut rig, "B", budget);
        assert_eq!(
            (kept.is_some(), c),
            (fresh.is_some(), a),
            "{budget}: found under the cap"
        );
    }
}
