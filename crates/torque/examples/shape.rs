//! Reads a DTS shape from bytes and asks its collision mesh a ray query.
//!
//! The shape is made in memory (a unit cube as the collision detail `Collision-1`) and written with the crate's own
//! writer, so the example needs no file; reading a `.dts` from disk is the same `parse` over the file's bytes.
//!
//! `cargo run -p torque --example shape`

use torque::shapes::collide::Collision;
use torque::shapes::preload::collision_slots;
use torque::shapes::shape::Shape;
use torque::shapes::{dts, synth};

/// The work budget a parse or a query may spend; a host passes a closure that counts down and refuses past its limit.
fn unlimited(_work: u64) -> bool {
    true
}

fn main() {
    let bytes = synth::solid_dts(
        [-1.0, -1.0, -1.0, 2.0, 2.0, 2.0],
        &[("Collision-1", [0.0, 0.0, 0.0, 1.0, 1.0, 1.0])],
        [0, 0, 0, 32767],
        [0.0; 3],
        false,
    );
    println!("{} bytes of .dts", bytes.len());

    let model = dts::parse(&bytes, &mut unlimited).expect("a valid shape");
    let shape = Shape::for_server(model);
    let name = |i: i32| {
        usize::try_from(i)
            .ok()
            .and_then(|i| shape.names.get(i))
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default()
    };
    println!(
        "version {}, {} nodes, {} objects, {} meshes, {} sequences",
        shape.version,
        shape.nodes.len(),
        shape.objects.len(),
        shape.meshes.len(),
        shape.sequences.len()
    );
    for detail in &shape.details {
        println!("detail {:?}, size {}", name(detail.name), detail.size);
    }
    println!("bounds {:?}", shape.bounds);

    // The collision and line-of-sight details, then a ray from outside through the cube along +x.
    let slots = collision_slots(&shape.names, &shape.details);
    let collision = Collision::build(&shape, &slots, &mut unlimited).expect("collision data");
    let (from, to) = ([-1.0, 0.5, 0.5], [2.0, 0.5, 0.5]);
    match collision
        .cast_ray(&slots, from, to, &mut unlimited)
        .expect("within budget")
    {
        Some(hit) => println!(
            "ray {from:?} -> {to:?} hits at t = {:.4}, normal {:?}",
            hit.t, hit.normal
        ),
        None => println!("ray {from:?} -> {to:?} misses"),
    }
}
