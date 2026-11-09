//! Moving the root of a 4,096-node tree and flushing: every node is recomputed (a quarter of them hung on a node of their
//! parent), then the moved list is read.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use torque_collide::math::{IDENTITY, Matrix};
use torque_hierarchy::{Hierarchy, Host, Keep, Local, NodeRef, ObjId, ParentRef, Pose};

const NODES: u32 = 4_096;
const BRANCH: u32 = 8;

/// Every object sits at the origin; every parent has nodes 0 to 3, offset from its origin.
struct Plain;

impl Host for Plain {
    fn pose(&self, _: ObjId) -> Option<Pose> {
        Some(Pose {
            world: IDENTITY,
            scale: [1.0; 3],
        })
    }

    fn node_matrix(&self, _: ObjId, node: u16) -> Option<Matrix> {
        let mut m = IDENTITY;
        m[3] = f32::from(node);
        m[7] = 0.5;
        (node < 4).then_some(m)
    }
}

/// An 8-ary tree of `NODES` objects in breadth-first order, object 0 the root.
fn tree() -> Hierarchy {
    let mut h = Hierarchy::new(torque_hierarchy::Caps::default());
    for i in 1..NODES {
        let node = if i % 4 == 0 {
            NodeRef::Node((i % 3) as u16)
        } else {
            NodeRef::Origin
        };
        let parent = ParentRef {
            object: ObjId((i - 1) / BRANCH),
            node,
        };
        h.set_parent(ObjId(i), Some(parent), Keep::Local, &Plain)
            .expect("the tree is within the caps");
        let turn = 0.01 * i as f32;
        let local = Local {
            translation: [1.0, 0.5, 0.25],
            rotation: [0.0, 0.0, (turn / 2.0).sin(), (turn / 2.0).cos()],
            scale: [1.0; 3],
        };
        h.set_local(ObjId(i), local).expect("a valid pose");
    }
    h.flush(&Plain, usize::MAX);
    h
}

fn move_root_and_flush(c: &mut Criterion) {
    let mut h = tree();
    let mut moved = Vec::with_capacity(NODES as usize);
    let mut x = 0.0f32;
    c.bench_function("move the root of 4096 nodes and flush", |b| {
        b.iter(|| {
            x += 0.1;
            let mut world = IDENTITY;
            world[3] = x;
            h.set_world(ObjId(0), world, &Plain).expect("finite");
            let stats = h.flush(&Plain, usize::MAX);
            moved.clear();
            h.drain_moved(&mut moved);
            black_box((stats.resolved, moved.len()))
        });
    });
}

criterion_group!(benches, move_root_and_flush);
criterion_main!(benches);
