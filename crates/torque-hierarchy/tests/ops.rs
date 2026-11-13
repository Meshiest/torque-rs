//! Random set, clear, delete and move operations against the tree: the caps hold, nothing forms a cycle, a keep-world change
//! keeps the world pose, and a second run gives the same bits.

use crate::common;

use std::collections::BTreeMap;

use common::{Hash, Rng, World, pose_error, random_local, random_pose};
use torque_collide::math::Matrix;
use torque_hierarchy::{Caps, Hierarchy, Keep, NodeRef, ObjId, ParentError, ParentRef, Pose};

const OBJECTS: u32 = 500;
const OPS: usize = 100_000;
/// The bounds of a keep-world change in the float arithmetic: units, and radians.
const KEEP_UNITS: f32 = 1e-4;
const KEEP_RAD: f32 = 1e-4;

#[derive(Debug, Default, PartialEq)]
struct Report {
    hash: u64,
    max_units: f32,
    max_rad: f32,
    refused: BTreeMap<&'static str, u64>,
    done: BTreeMap<&'static str, u64>,
    most_parented: usize,
    deepest: usize,
    /// The largest coordinate of any world transform seen.
    largest_world: f32,
}

struct Sim {
    h: Hierarchy,
    host: World,
    rng: Rng,
    report: Report,
    caps: Caps,
}

fn node_code(p: &ParentRef) -> u64 {
    match p.node {
        NodeRef::Origin => 0,
        NodeRef::Node(i) => 1 + u64::from(i),
    }
}

impl Sim {
    fn new(caps: Caps, quantize: bool, seed: u64) -> Sim {
        let mut rng = Rng::new(seed);
        let mut host = World::default();
        for i in 0..OBJECTS {
            host.poses.insert(ObjId(i), random_pose(&mut rng));
        }
        Sim {
            h: Hierarchy::new(caps).quantize_locals(quantize),
            host,
            rng,
            report: Report::default(),
            caps: caps.clamped(),
        }
    }

    fn pick(&mut self) -> ObjId {
        ObjId(self.rng.below(u64::from(OBJECTS)) as u32)
    }

    /// Flush everything and give the host the poses of what moved.
    fn sync(&mut self) {
        self.h.flush(&self.host, usize::MAX);
        let mut moved = Vec::new();
        self.h.drain_moved(&mut moved);
        for id in moved {
            let pose = self.h.pose(id).expect("a moved object is held");
            self.host.poses.insert(id, pose);
        }
    }

    fn world_of(&self, id: ObjId) -> Matrix {
        self.h
            .world(id)
            .unwrap_or_else(|| self.host.poses[&id].world)
    }

    fn record(&mut self, what: &'static str, error: Option<(f32, f32)>) {
        *self.report.done.entry(what).or_default() += 1;
        if let Some((units, rad)) = error {
            self.report.max_units = self.report.max_units.max(units);
            self.report.max_rad = self.report.max_rad.max(rad);
        }
    }

    fn refuse(&mut self, e: ParentError) {
        *self.report.refused.entry(e.name()).or_default() += 1;
    }

    fn set_parent(&mut self) {
        let child = self.pick();
        let parent = self.rng.chance(90).then(|| ParentRef {
            object: self.pick(),
            node: if self.rng.chance(70) {
                NodeRef::Origin
            } else {
                NodeRef::Node(self.rng.below(6) as u16)
            },
        });
        self.sync();
        // Keeping a large local pose under a scaled parent multiplies it, so the worlds of a long run would grow without
        // bound. Keep-local is used where the local pose is small.
        let small_local = self
            .h
            .local(child)
            .is_some_and(|l| l.translation.iter().all(|t| t.abs() <= 0.6));
        let keep = if self.rng.chance(50) || !small_local {
            Keep::World
        } else {
            Keep::Local
        };
        let before = self.world_of(child);
        let was = self.h.parent(child);
        match self.h.set_parent(child, parent, keep, &self.host) {
            Ok(pose) => {
                self.host.poses.insert(child, pose);
                let error = (keep == Keep::World).then(|| {
                    self.sync();
                    let a = pose_error(&before, &self.world_of(child));
                    let b = pose_error(&before, &pose.world);
                    (a.0.max(b.0), a.1.max(b.1))
                });
                let name = if parent.is_some() {
                    "parent"
                } else {
                    "unparent"
                };
                self.record(name, error);
            }
            Err(e) => {
                assert_eq!(self.h.parent(child), was, "a refused link changed the tree");
                self.refuse(e);
            }
        }
    }

    fn set_local(&mut self) {
        let id = self.pick();
        let local = random_local(&mut self.rng);
        match self.h.set_local(id, local) {
            Ok(()) => self.record("local", None),
            Err(e) => self.refuse(e),
        }
    }

    fn set_world(&mut self) {
        let id = self.pick();
        self.sync();
        // A child is put near its parent and a root near the origin, so the worlds of a long run stay within a few hundred
        // units, where a float keeps a pose to its last digits.
        let mut world = random_pose(&mut self.rng).world;
        if let Some(parent) = self.h.parent(id) {
            let near = self.world_of(parent.object);
            for k in [3, 7, 11] {
                world[k] = near[k] + self.rng.range(-0.5, 0.5);
            }
        }
        let scale = self
            .h
            .pose(id)
            .map_or(self.host.poses[&id].scale, |p| p.scale);
        match self.h.set_world(id, world, &self.host) {
            Ok(held) => {
                self.host.poses.insert(id, Pose { world, scale });
                let error = held.then(|| {
                    self.sync();
                    pose_error(&world, &self.world_of(id))
                });
                self.record("world", error);
            }
            Err(e) => self.refuse(e),
        }
    }

    fn set_scale(&mut self) {
        let id = self.pick();
        let scale = [
            self.rng.range(0.5, 1.5),
            self.rng.range(0.5, 1.5),
            self.rng.range(0.5, 1.5),
        ];
        match self.h.set_scale(id, scale) {
            Ok(held) => {
                if !held {
                    let pose = self
                        .host
                        .poses
                        .get_mut(&id)
                        .expect("every object has a pose");
                    pose.scale = scale;
                }
                self.record("scale", None);
            }
            Err(e) => self.refuse(e),
        }
    }

    /// The descendants that go with `id` on a plain delete, children first.
    fn goes_with(&self, id: ObjId, out: &mut Vec<ObjId>) {
        for child in self.h.children(id) {
            if self.h.delete_with_parent(child) {
                self.goes_with(child, out);
                out.push(child);
            }
        }
    }

    /// All descendants of `id`, children first, `id` last.
    fn subtree(&self, id: ObjId, out: &mut Vec<ObjId>) {
        for child in self.h.children(id) {
            self.subtree(child, out);
        }
        out.push(id);
    }

    fn respawn(&mut self, ids: &[ObjId]) {
        for id in ids {
            self.host.poses.insert(*id, random_pose(&mut self.rng));
        }
    }

    fn delete(&mut self) {
        let id = self.pick();
        self.sync();
        let mut expected = Vec::new();
        self.goes_with(id, &mut expected);
        let kept: Vec<ObjId> = self
            .h
            .children(id)
            .filter(|c| !self.h.delete_with_parent(*c))
            .collect();
        let before: Vec<Matrix> = kept.iter().map(|c| self.world_of(*c)).collect();
        let mut out = Vec::new();
        self.h.delete(id, &self.host, &mut out);
        assert_eq!(
            out, expected,
            "a plain delete removes flagged children, children first"
        );
        self.sync();
        assert!(!self.h.contains(id) && !self.h.delete_with_parent(id));
        for gone in &out {
            assert!(!self.h.contains(*gone));
        }
        let mut worst = (0.0f32, 0.0f32);
        for (child, was) in kept.iter().zip(&before) {
            assert!(
                self.h.parent(*child).is_none(),
                "a detached child has a parent"
            );
            let e = pose_error(was, &self.world_of(*child));
            worst = (worst.0.max(e.0), worst.1.max(e.1));
        }
        self.record("delete", Some(worst));
        out.push(id);
        self.respawn(&out);
    }

    fn delete_recursive(&mut self) {
        let id = self.pick();
        let mut expected = Vec::new();
        self.subtree(id, &mut expected);
        let mut out = Vec::new();
        self.h.delete_recursive(id, &mut out);
        assert_eq!(
            out, expected,
            "a recursive delete removes the subtree, children first"
        );
        for gone in &out {
            assert!(!self.h.contains(*gone) && !self.h.delete_with_parent(*gone));
        }
        self.record("delete_recursive", None);
        self.respawn(&out);
    }

    fn flag(&mut self) {
        let id = self.pick();
        let on = self.rng.chance(50);
        match self.h.set_delete_with_parent(id, on) {
            Ok(()) => self.record("flag", None),
            Err(e) => self.refuse(e),
        }
    }

    fn partial_flush(&mut self) {
        let budget = self.rng.below(60) as usize;
        let stats = self.h.flush(&self.host, budget);
        assert_eq!(stats.node_misses, 0);
        self.record("partial_flush", None);
    }

    /// The rules, checked through the public queries alone and by the tree's own audit.
    fn check(&mut self) {
        self.h.audit().expect("the audit finds no broken rule");
        let caps = self.caps;
        assert!(self.h.parented_count() <= caps.total);
        self.report.most_parented = self.report.most_parented.max(self.h.parented_count());
        for i in 0..OBJECTS {
            let id = ObjId(i);
            assert!(self.h.depth(id) <= caps.depth);
            assert!(self.h.child_count(id) <= caps.children);
            assert!(self.h.descendant_count(id) <= caps.descendants);
            self.report.deepest = self.report.deepest.max(self.h.depth(id));
            if let Some(w) = self.h.world(id) {
                let far = w[3].abs().max(w[7].abs()).max(w[11].abs());
                self.report.largest_world = self.report.largest_world.max(far);
            }
            let mut at = id;
            for step in 0..=caps.depth {
                let Some(p) = self.h.parent(at) else { break };
                at = p.object;
                assert!(
                    step < caps.depth && at != id,
                    "the chain from {id:?} loops or is too long"
                );
            }
        }
    }

    fn digest(&mut self) {
        let mut hash = Hash(self.report.hash);
        for i in 0..OBJECTS {
            let id = ObjId(i);
            hash.add(u64::from(i));
            if let Some(p) = self.h.parent(id) {
                hash.add(u64::from(p.object.0) << 16 | node_code(&p));
            }
            hash.add(self.h.child_count(id) as u64);
            if let Some(w) = self.h.world(id) {
                hash.matrix(&w);
            }
        }
        self.report.hash = hash.0;
    }

    fn run(mut self, ops: usize) -> Report {
        for n in 0..ops {
            match self.rng.below(100) {
                0..=39 => self.set_parent(),
                40..=54 => self.set_local(),
                55..=69 => self.set_world(),
                70..=74 => self.set_scale(),
                75..=82 => self.delete(),
                83..=86 => self.delete_recursive(),
                87..=92 => self.flag(),
                _ => self.partial_flush(),
            }
            if n % 50 == 49 {
                self.check();
            }
            if n % 1_000 == 999 {
                self.sync();
                self.digest();
            }
        }
        self.sync();
        self.check();
        self.report
    }
}

/// Caps small enough that every refusal comes up.
fn small() -> Caps {
    Caps {
        depth: 5,
        children: 3,
        descendants: 14,
        total: 250,
    }
}

fn assert_exercised(r: &Report, reasons: &[&str]) {
    for name in reasons {
        let n = r.refused.get(name).copied().unwrap_or(0);
        assert!(n > 0, "no {name} refusal: {:?}", r.refused);
    }
    for op in [
        "parent",
        "unparent",
        "local",
        "world",
        "scale",
        "delete",
        "delete_recursive",
        "flag",
        "partial_flush",
    ] {
        let n = r.done.get(op).copied().unwrap_or(0);
        assert!(n > 100, "too few {op}: {:?}", r.done);
    }
}

#[test]
fn random_ops_keep_every_cap_and_the_world_pose() {
    let r = Sim::new(small(), false, 1).run(OPS);
    let reasons = [
        "self",
        "cycle",
        "depth",
        "children",
        "subtree",
        "total",
        "badNode",
        "notParented",
    ];
    assert_exercised(&r, &reasons);
    assert!(r.deepest == 5 && r.most_parented > 100, "{r:?}");
    eprintln!("small caps: {r:?}");
    assert!(
        r.max_units <= KEEP_UNITS,
        "keep-world moved an object {} units",
        r.max_units
    );
    assert!(
        r.max_rad <= KEEP_RAD,
        "keep-world turned an object {} rad",
        r.max_rad
    );
}

#[test]
fn random_ops_under_the_default_caps() {
    let r = Sim::new(Caps::default(), false, 2).run(OPS);
    assert_exercised(&r, &["self", "cycle", "badNode", "notParented"]);
    eprintln!("default caps: {r:?}");
    assert!(r.max_units <= KEEP_UNITS && r.max_rad <= KEEP_RAD, "{r:?}");
}

#[test]
fn a_second_run_gives_the_same_bits() {
    let a = Sim::new(small(), false, 1).run(OPS);
    let b = Sim::new(small(), false, 1).run(OPS);
    assert_eq!(a, b);
    let other = Sim::new(small(), false, 3).run(5_000);
    assert_ne!(a.hash, other.hash);
}

#[test]
fn quantized_locals_stay_within_the_wire_step() {
    let r = Sim::new(small(), true, 4).run(20_000);
    // Half a step of 1/1024 per axis, scaled by a parent scale up to 2, and the rotation's 15-bit parts (the dropped part's error is amplified at most 2x).
    assert!(r.max_units <= 2.5e-3, "{r:?}");
    assert!(r.max_rad <= 2.5e-4, "{r:?}");
}
