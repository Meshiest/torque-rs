//! The hostile-input fuzz: links, poses and wire codes with NaN, infinite, huge and out-of-range values, unknown objects, nodes
//! the host does not have and a host that answers with rubbish go through the tree and the quantizer. It must not panic, the
//! tree must pass its audit, and every cap must hold. `TORQUE_FUZZ_HIERARCHY` sets the number of operations (default 2,000;
//! the full run is 1,000,000).

use crate::common;

use common::{Rng, random_local, random_pose};
use torque_collide::math::{IDENTITY, Matrix};
use torque_hierarchy::{
    Caps, Hierarchy, Host, Keep, Local, NodeRef, ObjId, ParentRef, Pose, QuantLocal, QuantRotation,
};

fn count() -> u64 {
    std::env::var("TORQUE_FUZZ_HIERARCHY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000)
}

fn float(r: &mut Rng) -> f32 {
    match r.below(12) {
        0 => f32::NAN,
        1 => f32::INFINITY,
        2 => f32::NEG_INFINITY,
        3 => f32::MAX,
        4 => f32::MIN_POSITIVE,
        5 => 0.0,
        6 => 1e-30,
        7 => r.range(-1e6, 1e6),
        8 => f32::from_bits(r.next_u64() as u32),
        _ => r.range(-3.0, 3.0),
    }
}

fn hostile_matrix(r: &mut Rng) -> Matrix {
    if r.chance(40) {
        return random_pose(r).world;
    }
    let mut m = IDENTITY;
    for v in &mut m {
        *v = if r.chance(50) { *v } else { float(r) };
    }
    m
}

fn hostile_local(r: &mut Rng) -> Local {
    if r.chance(40) {
        return random_local(r);
    }
    Local {
        translation: [float(r), float(r), float(r)],
        rotation: [float(r), float(r), float(r), float(r)],
        scale: [float(r), float(r), float(r)],
    }
}

/// A host whose answers are random: no pose, a wild pose, no node, a wild node matrix.
struct Hostile {
    rng: std::cell::RefCell<Rng>,
}

impl Host for Hostile {
    fn pose(&self, _: ObjId) -> Option<Pose> {
        let mut r = self.rng.borrow_mut();
        match r.below(10) {
            0 => None,
            1 => Some(Pose {
                world: hostile_matrix(&mut r),
                scale: [float(&mut r), float(&mut r), float(&mut r)],
            }),
            _ => Some(random_pose(&mut r)),
        }
    }

    fn node_matrix(&self, _: ObjId, node: u16) -> Option<Matrix> {
        let mut r = self.rng.borrow_mut();
        match r.below(10) {
            0 => None,
            1 => Some(hostile_matrix(&mut r)),
            _ => {
                let mut m = IDENTITY;
                m[3] = f32::from(node % 7);
                Some(m)
            }
        }
    }
}

fn id(r: &mut Rng) -> ObjId {
    match r.below(20) {
        0 => ObjId(u32::MAX),
        1 => ObjId(r.next_u64() as u32),
        _ => ObjId(r.below(150) as u32),
    }
}

fn parent_ref(r: &mut Rng) -> Option<ParentRef> {
    r.chance(90).then(|| ParentRef {
        object: id(r),
        node: match r.below(6) {
            0 => NodeRef::Node(u16::MAX),
            1 | 2 => NodeRef::Node(r.below(10) as u16),
            _ => NodeRef::Origin,
        },
    })
}

fn quant(r: &mut Rng) -> QuantLocal {
    let word = |r: &mut Rng| r.next_u64();
    QuantLocal {
        translation: [word(r) as i32, word(r) as i32, word(r) as i32],
        rotation: QuantRotation {
            dropped: word(r) as u8,
            parts: [word(r) as u16, word(r) as u16, word(r) as u16],
        },
        scale: [word(r) as u16, word(r) as u16, word(r) as u16],
    }
}

fn assert_caps(h: &Hierarchy) {
    let caps = h.caps();
    assert!(h.parented_count() <= caps.total);
    for i in 0..150 {
        let id = ObjId(i);
        assert!(h.depth(id) <= caps.depth);
        assert!(h.child_count(id) <= caps.children);
        assert!(h.descendant_count(id) <= caps.descendants);
    }
}

/// Runs `ops` operations; returns how many links were made and the most objects parented at once.
fn run(caps: Caps, seed: u64, ops: u64) -> (u64, usize) {
    let (mut linked, mut most) = (0, 0);
    let mut r = Rng::new(seed);
    let host = Hostile {
        rng: std::cell::RefCell::new(Rng::new(seed ^ 0x5555)),
    };
    let mut h = Hierarchy::new(caps).quantize_locals(seed.is_multiple_of(2));
    let mut out = Vec::new();
    for n in 0..ops {
        match r.below(100) {
            0..=24 => {
                let keep = if r.chance(50) {
                    Keep::World
                } else {
                    Keep::Local
                };
                let (child, parent) = (id(&mut r), parent_ref(&mut r));
                linked +=
                    u64::from(parent.is_some() && h.set_parent(child, parent, keep, &host).is_ok());
            }
            25..=39 => {
                let (child, parent) = (id(&mut r), parent_ref(&mut r));
                if let Some(parent) = parent {
                    let local = hostile_local(&mut r);
                    linked += u64::from(h.link(child, parent, local, &host).is_ok());
                }
            }
            40..=47 => {
                let (child, local) = (id(&mut r), hostile_local(&mut r));
                let _ = h.set_local(child, local);
            }
            48..=55 => {
                let (child, world) = (id(&mut r), hostile_matrix(&mut r));
                let _ = h.set_world(child, world, &host);
            }
            56..=60 => {
                let (child, scale) = (id(&mut r), [float(&mut r), float(&mut r), float(&mut r)]);
                let _ = h.set_scale(child, scale);
            }
            61..=64 => h.mark_dirty(id(&mut r)),
            65..=70 => {
                let budget = r.below(40) as usize;
                h.flush(&host, budget);
            }
            71..=73 => {
                out.clear();
                h.drain_moved(&mut out);
            }
            74..=79 => {
                out.clear();
                let object = id(&mut r);
                h.delete(object, &host, &mut out);
            }
            80..=83 => {
                out.clear();
                let object = id(&mut r);
                h.delete_recursive(object, &mut out);
            }
            84..=87 => {
                let (object, flag) = (id(&mut r), r.chance(50));
                let _ = h.set_delete_with_parent(object, flag);
            }
            88..=91 => {
                let (child, parent) = (id(&mut r), parent_ref(&mut r));
                let _ = h.check_parent(child, parent);
            }
            92..=95 => {
                let code = quant(&mut r);
                let local = code.to_local();
                assert!(local.validate().is_ok(), "{code:?} decodes to {local:?}");
                let _ = code.check();
            }
            _ => {
                let local = hostile_local(&mut r);
                let back = QuantLocal::from_local(&local).to_local();
                assert!(back.validate().is_ok(), "{local:?} became {back:?}");
            }
        }
        most = most.max(h.parented_count());
        if n % 997 == 996 {
            h.audit().expect("the tree passes its audit");
            assert_caps(&h);
        }
    }
    h.flush(&host, usize::MAX);
    h.audit().expect("the tree passes its audit");
    assert_caps(&h);
    (linked, most)
}

#[test]
fn hostile_input_never_panics_or_breaks_a_cap() {
    let n = count();
    let small = Caps {
        depth: 6,
        children: 5,
        descendants: 30,
        total: 100,
    };
    for (caps, seed) in [(small, 1), (Caps::default(), 2)] {
        let (linked, most) = run(caps, seed, n / 2);
        eprintln!("{linked} links made, {most} parented at most");
        assert!(
            linked > n / 40 && most >= 10,
            "the fuzz left the tree nearly empty"
        );
    }
}

#[test]
fn caps_are_clamped_to_the_hard_caps() {
    let huge = Caps {
        depth: usize::MAX,
        children: usize::MAX,
        descendants: usize::MAX,
        total: usize::MAX,
    };
    assert_eq!(
        huge.clamped(),
        Caps {
            depth: 32,
            children: 1_024,
            descendants: 16_384,
            total: 131_072,
        }
    );
    let zero = Caps {
        depth: 0,
        children: 0,
        descendants: 0,
        total: 0,
    };
    assert_eq!(
        zero.clamped(),
        Caps {
            depth: 1,
            children: 1,
            descendants: 1,
            total: 1,
        }
    );
    run(zero, 3, 2_000);
    run(huge, 4, 2_000);
}
