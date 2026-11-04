//! What the datablocks that load a shape find in it (the reading of `ShapeBaseData::preload`).
//!
//! **Collision details** ([`collision_slots`]). Every detail whose name, lowercased, contains `collision-` is a collision detail,
//! in detail order, as many as the shape has. Each takes as its line-of-sight detail the detail named `LOS-(i + 9)` (`i` the
//! collision detail's own index among the details, 9 being one past [`SLOTS`]), else itself. Then every detail whose
//! lowercased name contains `los-` and that no entry claims yet is appended to the LOS list, which can so grow longer than
//! the collision list. A detail is found by name ([`find_detail`]): the name table's first entry equal to the text without
//! regard to case, then the first detail that names that entry. Torque lowercases the detail names in the shape's own table as
//! it tests them; this reads them without changing the table. A datablock keeps three lists: the collision details, their
//! collision boxes and the LOS details. A host that picks the details by another rule hands its own to [`facts_with`].

use crate::bounds::detail_bounds;
use crate::shape::{Detail, Shape, find_name};
use crate::{Charge, ShapeError, pay};

/// `MaxCollisionShapes`: the offset of a collision detail's `LOS-` name.
pub const SLOTS: usize = 8;

/// The collision details and the LOS details of a shape, as detail indices; -1 marks an entry with none (a host's rule may
/// leave holes).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Slots {
    pub collision: Vec<i32>,
    pub los: Vec<i32>,
}

impl Slots {
    /// How many entries have a collision detail (the number of hulls the datablock keeps).
    pub fn hulls(&self) -> usize {
        self.collision.iter().filter(|&&d| d >= 0).count()
    }

    /// How many LOS entries have a detail of their own, against borrowing their collision entry's detail.
    pub fn own_los(&self) -> usize {
        self.los
            .iter()
            .enumerate()
            .filter(|&(i, &l)| l >= 0 && self.collision.get(i) != Some(&l))
            .count()
    }
}

/// The detail indices of a shape's collision and LOS details: a rule [`facts_with`] takes.
pub type SlotRule = fn(&[Vec<u8>], &[Detail]) -> Slots;

/// `TSShape::findDetail`: the first detail whose name is the name table's entry for `text`, or -1.
pub fn find_detail(names: &[Vec<u8>], details: &[Detail], text: &str) -> i32 {
    let Some(index) = find_name(names, text.as_bytes()) else {
        return -1;
    };
    details
        .iter()
        .position(|d| d.name == index)
        .and_then(|j| i32::try_from(j).ok())
        .unwrap_or(-1)
}

/// Whether detail `d`'s name, lowercased, contains `needle` (lowercase); a name index outside the table names nothing.
fn name_contains(names: &[Vec<u8>], d: &Detail, needle: &[u8]) -> bool {
    usize::try_from(d.name)
        .ok()
        .and_then(|n| names.get(n))
        .is_some_and(|name| {
            name.to_ascii_lowercase()
                .windows(needle.len())
                .any(|w| w == needle)
        })
}

/// `ShapeBaseData::preload`'s collision and LOS details of a shape with these names and details (module docs).
pub fn collision_slots(names: &[Vec<u8>], details: &[Detail]) -> Slots {
    let mut slots = Slots::default();
    for (i, d) in details.iter().enumerate() {
        if !name_contains(names, d, b"collision-") {
            continue;
        }
        let index = i32::try_from(i).unwrap_or(i32::MAX);
        slots.collision.push(index);
        let los = find_detail(names, details, &format!("LOS-{}", i + 1 + SLOTS));
        slots.los.push(if los == -1 { index } else { los });
    }
    for (i, d) in details.iter().enumerate() {
        let index = i32::try_from(i).unwrap_or(i32::MAX);
        if name_contains(names, d, b"los-") && !slots.los.contains(&index) {
            slots.los.push(index);
        }
    }
    slots
}

/// The mount-point nodes `ShapeBaseData::preload` looks up (`mount0` to `mount31`, `NumMountPoints`).
pub const MOUNT_POINTS: usize = 32;

/// What the bounds check found for one slot's collision box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finding {
    /// The box lies inside the shape's bounds and max is not below min: kept as computed.
    Inside,
    /// Not inside the shape's bounds (a NaN counts): replaced by the shape's bounds, and the datablock prints the
    /// `bounds exceed` warning ([`exceed_line`]).
    Exceeds,
    /// Inside, but max is below min on an axis (a detail with no mesh has the box 1e31 to -1e31): replaced by the shape's
    /// bounds, and the datablock prints the `bounds box invalid` error always.
    Invalid,
}

/// Everything the preloads find in a shape, computed once per shape (the datablocks that load it print the findings, each its
/// own): the collision and LOS details, the collision boxes after the check, `Debris-17`, the `eye` and
/// `cam` nodes (`cam` falls back to `eye`), `mount0` to `mount31`, and `AIRepairNode`. The sequences `Visibility` and `Damage`
/// are looked up with [`sequence_index`] when a datablock needs them (a `TSShapeConstructor` may import more later).
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub slots: Slots,
    /// The collision box of each collision entry, `[min.x, min.y, min.z, max.x, max.y, max.z]` (the shape's bounds where the
    /// check replaced it); zeros for an entry without a detail.
    pub bounds: Vec<[f32; 6]>,
    /// The check's finding for each collision entry, `None` for an entry without a detail.
    pub findings: Vec<Option<Finding>>,
    pub debris_detail: i32,
    pub eye_node: i32,
    pub camera_node: i32,
    pub mount_nodes: [i32; MOUNT_POINTS],
    pub ai_repair_node: i32,
}

impl Facts {
    /// True when every hull passes (or there is none).
    pub fn all_inside(&self) -> bool {
        self.findings
            .iter()
            .all(|f| matches!(f, None | Some(Finding::Inside)))
    }
}

/// `Box3F::isContained`: `outer` holds `inner` when no `inner` minimum is below `outer`'s and no `outer` maximum is
/// below `inner`'s; a NaN makes the compare unordered, which counts as "not contained".
fn contains(outer: &[f32; 6], inner: &[f32; 6]) -> bool {
    (0..3).all(|k| outer[k] <= inner[k] && outer[k + 3] >= inner[k + 3])
}

/// The box `computeBounds` returns for a detail with no mesh: `1e31` for every minimum, `-1e31` for every maximum.
const EMPTY_BOX: [f32; 6] = [1e31, 1e31, 1e31, -1e31, -1e31, -1e31];

/// The first node whose name is the name table's entry for `text` (`TSShape::findNode`).
fn node_named(shape: &Shape, text: &str) -> i32 {
    let Some(index) = find_name(&shape.names, text.as_bytes()) else {
        return -1;
    };
    shape
        .nodes
        .iter()
        .position(|n| n.name == index)
        .and_then(|j| i32::try_from(j).ok())
        .unwrap_or(-1)
}

/// The first sequence whose name is the name table's entry for `text` (`TSShape::findSequence`; -1 when there is none).
pub fn sequence_index(shape: &Shape, text: &str) -> i32 {
    let Some(index) = find_name(&shape.names, text.as_bytes()) else {
        return -1;
    };
    shape
        .sequences
        .iter()
        .position(|s| s.name == index)
        .and_then(|j| i32::try_from(j).ok())
        .unwrap_or(-1)
}

/// What `ShapeBaseData::preload` finds in `shape`, its collision details by [`collision_slots`]. The lookups are charged
/// first (a few hundred table scans), then the walk over the details' objects and primitives that counts the collision
/// triangles, then each collision box (`computeBounds`, per vertex): a run that cannot pay ends with
/// [`ShapeError::OutOfBudget`].
pub fn facts(shape: &Shape, charge: Charge) -> Result<Facts, ShapeError> {
    facts_with(shape, collision_slots, charge)
}

/// [`facts`] with the collision and LOS details picked by `rule` instead of [`collision_slots`].
pub fn facts_with(shape: &Shape, rule: SlotRule, charge: Charge) -> Result<Facts, ShapeError> {
    let table = (shape.names.len() + shape.nodes.len() + shape.details.len()) as u64;
    pay(charge, 64 + 64 * table)?;
    let slots = rule(&shape.names, &shape.details);
    // The shape's collision queries build a hull and a polygon list of these meshes: a shape that makes more than
    // the cap of triangles is refused with the other caps, before any work is done on it.
    let triangles = crate::collide::slot_triangles(shape, &slots, charge)?;
    if triangles > crate::collide::MAX_COLLISION_TRIANGLES as u64 {
        return Err(ShapeError::Cap {
            what: "collision triangles",
            value: triangles,
            cap: crate::collide::MAX_COLLISION_TRIANGLES as u64,
        });
    }
    let mut bounds = vec![[0.0f32; 6]; slots.collision.len()];
    let mut findings = vec![None; slots.collision.len()];
    let shape_bounds = shape.bounds;
    for i in 0..slots.collision.len() {
        let Ok(detail) = usize::try_from(slots.collision[i]) else {
            continue;
        };
        let computed = detail_bounds(shape, detail, charge)?.unwrap_or(EMPTY_BOX);
        let (finding, kept) = if !contains(&shape_bounds, &computed) {
            (Finding::Exceeds, shape_bounds)
        } else if (0..3).any(|k| computed[k + 3] < computed[k]) {
            (Finding::Invalid, shape_bounds)
        } else {
            (Finding::Inside, computed)
        };
        bounds[i] = kept;
        findings[i] = Some(finding);
    }
    let eye_node = node_named(shape, "eye");
    let cam = node_named(shape, "cam");
    let mut mount_nodes = [-1; MOUNT_POINTS];
    for (i, node) in mount_nodes.iter_mut().enumerate() {
        *node = node_named(shape, &format!("mount{i}"));
    }
    Ok(Facts {
        slots,
        bounds,
        findings,
        debris_detail: find_detail(&shape.names, &shape.details, "Debris-17"),
        eye_node,
        camera_node: if cam == -1 { eye_node } else { cam },
        mount_nodes,
        ai_repair_node: node_named(shape, "AIRepairNode"),
    })
}

/// The warning a hull whose box exceeds the shape's prints (`Con::warnf`, always): the datablock's shape name as it is spelt,
/// the collision entry's index, and the **detail index** in the second parentheses (the detail's index is printed where the
/// text says `Collision-%d`: the arguments are the name, the entry and the detail).
pub fn exceed_line(shape_name: &[u8], slot: usize, detail: i32) -> String {
    format!(
        "Warning: shape {} collision detail {slot} (Collision-{detail}) bounds exceed that of shape.",
        String::from_utf8_lossy(shape_name)
    )
}

/// The error a hull with max below min prints (`Con::errorf`, always), with the same arguments.
pub fn invalid_line(shape_name: &[u8], slot: usize, detail: i32) -> String {
    format!(
        "Error: shape {}-collision detail {slot} (Collision-{detail}) bounds box invalid!",
        String::from_utf8_lossy(shape_name)
    )
}

#[cfg(test)]
#[path = "preload_tests.rs"]
mod tests;

#[cfg(test)]
mod fact_tests {
    use super::*;
    use crate::synth::{hull_dts, hull_shape};

    fn free(_: u64) -> bool {
        true
    }

    const BIG: [f32; 6] = [-10.0, -10.0, -10.0, 10.0, 10.0, 10.0];

    fn facts_of(details: &[(&str, [f32; 6])], nodes: &[&str]) -> Facts {
        let model = hull_shape(BIG, details, nodes, &[]);
        facts(&model.shape, &mut free).unwrap()
    }

    #[test]
    fn a_hull_inside_the_shape_keeps_its_box() {
        let f = facts_of(&[("Collision-1", [-1.0, -2.0, -3.0, 1.0, 2.0, 3.0])], &[]);
        assert_eq!(f.findings[0], Some(Finding::Inside));
        assert_eq!(f.bounds[0], [-1.0, -2.0, -3.0, 1.0, 2.0, 3.0]);
        assert!(f.all_inside());
        // A box exactly the shape's bounds is inside (the compares are `<=` and `>=`).
        let f = facts_of(&[("Collision-1", BIG)], &[]);
        assert_eq!(f.findings[0], Some(Finding::Inside));
    }

    #[test]
    fn a_hull_past_the_shapes_bounds_exceeds_and_takes_them() {
        for hull in [
            [-11.0, -1.0, -1.0, 1.0, 1.0, 1.0],
            [-1.0, -1.0, -1.0, 1.0, 1.0, 10.5],
        ] {
            let f = facts_of(&[("Collision-1", hull)], &[]);
            assert_eq!(f.findings[0], Some(Finding::Exceeds), "{hull:?}");
            assert_eq!(f.bounds[0], BIG);
            assert!(!f.all_inside());
        }
    }

    #[test]
    fn a_detail_without_a_mesh_has_the_inverted_box_and_is_invalid() {
        // No mesh: computeBounds leaves 1e31 / -1e31, which the shape contains and which is inverted.
        let mut model = hull_shape(BIG, &[("Collision-1", BIG)], &[], &[]);
        model.shape.meshes = vec![None];
        let f = facts(&model.shape, &mut free).unwrap();
        assert_eq!(f.findings[0], Some(Finding::Invalid));
        assert_eq!(f.bounds[0], BIG);
        assert_eq!(f.slots.collision[0], 0);
    }

    #[test]
    fn each_collision_detail_has_its_own_finding_and_an_unclaimed_los_detail_is_appended() {
        let f = facts_of(
            &[
                (
                    "Collision-1",
                    [-1.0; 3]
                        .into_iter()
                        .chain([1.0; 3])
                        .collect::<Vec<_>>()
                        .try_into()
                        .unwrap(),
                ),
                ("Collision-3", [-20.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
                ("LOS-11", [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0]),
            ],
            &[],
        );
        // Two collision details (details 0 and 1), whatever their numbers.
        assert_eq!(f.findings, [Some(Finding::Inside), Some(Finding::Exceeds)]);
        assert_eq!(f.slots.collision, [0, 1]);
        // Neither has an LOS-9 or LOS-10 (details 0 and 1 plus nine): each borrows itself; LOS-11 is claimed by none, so the
        // second pass appends it.
        assert_eq!(f.slots.los, [0, 1, 2]);
    }

    #[test]
    fn the_nodes_looked_up_are_found_by_name_without_case() {
        let f = facts_of(
            &[],
            &["Eye", "mount0", "MOUNT31", "mount32", "AIRepairNode", "Cam"],
        );
        // root is node 0.
        assert_eq!(f.eye_node, 1);
        assert_eq!(f.mount_nodes[0], 2);
        assert_eq!(f.mount_nodes[31], 3);
        assert_eq!(f.mount_nodes[1], -1);
        assert_eq!(f.ai_repair_node, 5);
        assert_eq!(f.camera_node, 6);
        // Without `cam` the camera is the eye; without either it is -1.
        let f = facts_of(&[], &["eye"]);
        assert_eq!((f.eye_node, f.camera_node), (1, 1));
        let f = facts_of(&[], &[]);
        assert_eq!((f.eye_node, f.camera_node, f.debris_detail), (-1, -1, -1));
        let f = facts_of(&[("Debris-17", BIG)], &[]);
        assert_eq!(f.debris_detail, 0);
    }

    #[test]
    fn a_sequence_is_found_by_name_without_case() {
        let model = hull_shape(BIG, &[], &[], &["Visibility", "damage"]);
        assert_eq!(sequence_index(&model.shape, "Visibility"), 0);
        assert_eq!(sequence_index(&model.shape, "Damage"), 1);
        assert_eq!(sequence_index(&model.shape, "Other"), -1);
    }

    #[test]
    fn the_facts_of_a_parsed_file_are_those_of_its_model() {
        let bytes = hull_dts(
            BIG,
            &[("Collision-1", BIG), ("Collision-2", [0.0; 6])],
            &["mount3"],
            &[],
        );
        let model = crate::dts::parse(&bytes, &mut free).unwrap();
        let f = facts(&model.shape, &mut free).unwrap();
        assert_eq!(f.findings[0], Some(Finding::Inside));
        assert_eq!(f.findings[1], Some(Finding::Inside));
        assert_eq!(f.mount_nodes[3], 1);
    }

    #[test]
    fn the_facts_are_paid_for_before_the_work_and_a_refusal_ends_them() {
        let model = hull_shape(BIG, &[("Collision-1", BIG)], &[], &[]);
        let mut paid = Vec::new();
        facts(&model.shape, &mut |u| {
            paid.push(u);
            true
        })
        .unwrap();
        // The table scans first, then one payment per collision box walk.
        assert!(paid.len() >= 3, "{paid:?}");
        assert!(paid[0] >= 64);
        for budget in 0..paid.len() {
            let mut left = budget;
            let result = facts(&model.shape, &mut |_| {
                if left == 0 {
                    false
                } else {
                    left -= 1;
                    true
                }
            });
            assert_eq!(result, Err(ShapeError::OutOfBudget), "{budget}");
        }
    }

    #[test]
    fn the_two_lines_name_the_slot_and_the_detail() {
        assert_eq!(
            exceed_line(b"base/x.dts", 2, 5),
            "Warning: shape base/x.dts collision detail 2 (Collision-5) bounds exceed that of shape."
        );
        assert_eq!(
            invalid_line(b"base/x.dts", 0, 1),
            "Error: shape base/x.dts-collision detail 0 (Collision-1) bounds box invalid!"
        );
    }

    #[test]
    fn many_objects_over_shared_meshes_and_empty_primitives_are_paid_for_before_the_walk() {
        // A hostile case (`synth::shared_mesh_dts`): 16 details, 4,096 objects on the same 16 meshes, 120,000
        // primitives of no elements in each. It holds no triangle, so the triangle cap never sees it; the walk must pay.
        let bytes = crate::synth::shared_mesh_dts(4_096, 120_000);
        assert!(bytes.len() < crate::MAX_DTS_BYTES, "{}", bytes.len());
        let model = crate::dts::parse(&bytes, &mut |_| true).unwrap();
        let shape = Shape::for_server(model);
        let mut paid = 0u64;
        let found = facts(&shape, &mut |u| {
            paid += u;
            true
        })
        .unwrap();
        assert_eq!(found.slots.hulls(), 8);
        let primitives = 16 * 120_000;
        let objects = 16 * 4_096;
        // The walk that counts the triangles pays exactly that: each mesh's primitives once, however many objects share it.
        let mut counting = 0u64;
        let triangles = crate::collide::slot_triangles(&shape, &found.slots, &mut |u| {
            counting += u;
            true
        });
        assert_eq!(triangles, Ok(0));
        assert_eq!(
            counting,
            primitives * crate::collide::PRIMITIVE_WORK + objects * crate::collide::OBJECT_WORK
        );
        // The primitives are walked once for each mesh, the objects once for each visit.
        assert!(
            paid >= primitives + objects,
            "paid {paid} for {primitives} primitives and {objects} objects"
        );
        assert!(paid < 4 * (primitives + objects), "paid {paid}");
        // A budget a tenth of that ends the walk with the out-of-budget error, not the whole walk.
        let mut left = (primitives + objects) / 10;
        let refused = facts(&shape, &mut |u| {
            if u <= left {
                left -= u;
                true
            } else {
                false
            }
        });
        assert_eq!(refused, Err(ShapeError::OutOfBudget));
    }
}
