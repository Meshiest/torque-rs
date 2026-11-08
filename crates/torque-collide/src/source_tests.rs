use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::convex::WorkingSet;
use crate::math::IDENTITY;
use crate::types::{PLAYER, TERRAIN};

#[derive(Debug, Default)]
struct Shapes(RefCell<Vec<CollisionShape>>);

impl CollisionSource for Shapes {
    fn shapes(&self, out: &mut Vec<CollisionShape>) {
        out.extend(self.0.borrow().iter().cloned());
    }
}

fn at(x: f32, y: f32, z: f32) -> Matrix {
    let mut m = IDENTITY;
    (m[3], m[7], m[11]) = (x, y, z);
    m
}

fn boxed(key: u32, pose: Matrix, half: F3) -> CollisionShape {
    CollisionShape {
        key,
        pose,
        kind: ShapeKind::Box { half },
        type_mask: STATIC_SHAPE,
    }
}

fn source(shapes: Vec<CollisionShape>) -> Rc<Shapes> {
    Rc::new(Shapes(RefCell::new(shapes)))
}

fn sources(entries: &[PluginCollision], side: Side) -> CollisionSources {
    CollisionSources::from_entries(entries, side).0
}

#[test]
fn bits_follow_the_sorted_names_of_both_sides() {
    let s = source(vec![]);
    let entries = [
        PluginCollision::server("Props", s.clone()),
        PluginCollision::client("Props", s.clone()),
        PluginCollision::client("Alpha", s.clone()),
    ];
    let server = sources(&entries, Side::Server);
    let client = sources(&entries, Side::Client);
    // `Alpha` is first of the names though only the client has it: `Props` is the second on both sides.
    assert_eq!(server.bit("Props"), Some(FREE_TYPE_BITS[1]));
    assert_eq!(client.bit("Props"), Some(FREE_TYPE_BITS[1]));
    assert_eq!(client.bit("Alpha"), Some(FREE_TYPE_BITS[0]));
    assert_eq!(server.bit("Alpha"), None);
    // The order the entries were registered in changes nothing.
    let mut reversed = entries.to_vec();
    reversed.reverse();
    assert_eq!(
        sources(&reversed, Side::Server).bit("Props"),
        server.bit("Props")
    );
}

#[test]
fn free_bits_are_free_of_the_library_types() {
    use crate::types::*;
    let used = STATIC
        | ENVIRONMENT
        | TERRAIN
        | INTERIOR
        | WATER
        | TRIGGER
        | MARKER
        | GAME_BASE
        | SHAPE_BASE
        | CAMERA
        | STATIC_SHAPE
        | PLAYER
        | ITEM
        | VEHICLE
        | VEHICLE_BLOCKER
        | PROJECTILE
        | EXPLOSION
        | CORPSE
        | DEBRIS
        | PHYSICAL_ZONE
        | STATIC_TS
        | AI
        | STATIC_RENDERED
        | DAMAGABLE_ITEM;
    for bit in FREE_TYPE_BITS {
        assert_eq!(bit.count_ones(), 1);
        assert_eq!(used & bit, 0, "{bit:#x}");
    }
}

#[test]
fn names_and_counts_are_checked() {
    let s = source(vec![]);
    let mut entries: Vec<PluginCollision> = ["A", "", "has space", &"x".repeat(MAX_NAME + 1)]
        .iter()
        .map(|n| PluginCollision::server(*n, s.clone()))
        .collect();
    entries.push(PluginCollision::server("A", s.clone()));
    let (set, warnings) = CollisionSources::from_entries(&entries, Side::Server);
    assert_eq!(set.bits().count(), 1);
    assert_eq!(warnings.len(), 4, "{warnings:?}");
    // Past the free bits the later names are refused, the first ones keep their bits.
    let many: Vec<PluginCollision> = (0..MAX_SOURCES + 3)
        .map(|i| PluginCollision::server(format!("S{i:02}"), s.clone()))
        .collect();
    let (set, warnings) = CollisionSources::from_entries(&many, Side::Server);
    assert_eq!(set.bits().count(), MAX_SOURCES);
    assert_eq!(warnings.len(), 3);
    assert_eq!(set.bit("S00"), Some(FREE_TYPE_BITS[0]));
    assert_eq!(set.bit(&format!("S{:02}", MAX_SOURCES)), None);
}

#[test]
fn a_shape_has_its_sources_bit_and_a_stable_id() {
    let s = source(vec![
        boxed(5, at(0.0, 0.0, 0.0), [1.0; 3]),
        boxed(2, at(10.0, 0.0, 0.0), [1.0; 3]),
    ]);
    let entries = [PluginCollision::server("Props", s.clone())];
    let src = sources(&entries, Side::Server);
    let set = src.snapshot();
    let bit = src.bit("Props").unwrap();
    assert_eq!(set.len(), 2);
    let id = |key: u32| ObjectId(PLUGIN_ID_BASE + key);
    assert!(is_plugin_id(id(2)));
    assert!(!is_plugin_id(ObjectId(PLUGIN_ID_BASE - 1)));
    assert_eq!(set.object(id(5)).unwrap().type_mask, STATIC_SHAPE | bit);
    // The list order of the source does not matter: the keys do.
    s.0.borrow_mut().reverse();
    let again = src.snapshot();
    assert_eq!(again.object(id(2)).unwrap().position(), [10.0, 0.0, 0.0]);
    assert_eq!(set.object(id(7)), None);
}

#[test]
fn a_shape_asks_only_for_static_bits() {
    let mut shape = boxed(0, IDENTITY, [1.0; 3]);
    shape.type_mask = PLAYER | TERRAIN | STATIC;
    let entries = [PluginCollision::server("P", source(vec![shape]))];
    let src = sources(&entries, Side::Server);
    let set = src.snapshot();
    assert_eq!(
        set.object(ObjectId(PLUGIN_ID_BASE)).unwrap().type_mask,
        STATIC | src.bit("P").unwrap()
    );
}

#[test]
fn caps_hold() {
    let many: Vec<CollisionShape> = (0..MAX_SHAPES_PER_SOURCE as u32 * 2)
        .map(|k| boxed(k, at(k as f32 * 4.0, 0.0, 0.0), [1.0; 3]))
        .collect();
    let entries = [PluginCollision::server("P", source(many))];
    let src = sources(&entries, Side::Server);
    let set = src.snapshot();
    assert_eq!(set.len(), MAX_SHAPES_PER_SOURCE);
    // A key at the cap is dropped even when the list is short, as is a repeated key.
    let shapes = vec![
        boxed(MAX_SHAPES_PER_SOURCE as u32, IDENTITY, [1.0; 3]),
        boxed(3, at(1.0, 0.0, 0.0), [1.0; 3]),
        boxed(3, at(2.0, 0.0, 0.0), [1.0; 3]),
    ];
    let entries = [PluginCollision::server("P", source(shapes))];
    let set = sources(&entries, Side::Server).snapshot();
    assert_eq!(set.len(), 1);
    assert_eq!(
        set.object(ObjectId(PLUGIN_ID_BASE + 3)).unwrap().position(),
        [1.0, 0.0, 0.0]
    );
}

#[test]
fn hostile_poses_and_boxes_are_refused() {
    let mut nan = at(0.0, 0.0, 0.0);
    nan[5] = f32::NAN;
    let shapes = vec![
        boxed(0, nan, [1.0; 3]),
        boxed(1, at(f32::INFINITY, 0.0, 0.0), [1.0; 3]),
        boxed(2, at(MAX_COORD * 2.0, 0.0, 0.0), [1.0; 3]),
        boxed(3, IDENTITY, [f32::NAN, 1.0, 1.0]),
        boxed(4, IDENTITY, [MAX_HALF_EXTENT * 2.0, 1.0, 1.0]),
        boxed(5, IDENTITY, [0.0, 1.0, 1.0]),
        boxed(6, IDENTITY, [-1.0, 1.0, 1.0]),
    ];
    let entries = [PluginCollision::server("P", source(shapes))];
    let set = sources(&entries, Side::Server).snapshot();
    // The flat box is raised to the smallest half-extent, the negative one is its absolute value.
    assert_eq!(set.len(), 2);
    let flat = set.object(ObjectId(PLUGIN_ID_BASE + 5)).unwrap();
    assert_eq!(flat.obj_box.max[0], MIN_HALF_EXTENT);
    let neg = set.object(ObjectId(PLUGIN_ID_BASE + 6)).unwrap();
    assert_eq!(neg.obj_box.min[0], -1.0);
}

#[test]
fn hulls_are_checked() {
    let tri = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    // One triangle is flat, not a solid.
    assert!(PluginHull::new(&tri, &[[0, 1, 2]]).is_none());
    assert!(PluginHull::new(&tri, &[]).is_none());
    assert!(PluginHull::new(&tri, &[[0, 1, 3]]).is_none());
    assert!(PluginHull::new(&[[f32::NAN, 0.0, 0.0], tri[1], tri[2]], &[[0, 1, 2]]).is_none());
    assert!(PluginHull::new(&[[1.0e9, 0.0, 0.0], tri[1], tri[2]], &[[0, 1, 2]]).is_none());
    let many_verts = vec![[0.0; 3]; MAX_HULL_VERTS + 1];
    assert!(PluginHull::new(&many_verts, &[[0, 1, 2]]).is_none());
    let many_tris = vec![[0, 1, 2]; MAX_HULL_TRIS + 1];
    assert!(PluginHull::new(&tri, &many_tris).is_none());
    let cuboid = PluginHull::cuboid([1.0, 2.0, 3.0]).unwrap();
    assert_eq!(
        cuboid.bounds(),
        Box3::new([-1.0, -2.0, -3.0], [1.0, 2.0, 3.0])
    );
}

#[test]
fn a_query_is_filtered() {
    let shapes: Vec<CollisionShape> = (0..10)
        .map(|k| boxed(k, at(k as f32 * 10.0, 0.0, 0.0), [1.0; 3]))
        .collect();
    let entries = [PluginCollision::server("P", source(shapes))];
    let src = sources(&entries, Side::Server);
    let bit = src.bit("P").unwrap();
    let set = src.snapshot();
    let id = |k: u32| ObjectId(PLUGIN_ID_BASE + k);
    let near = Box3::new([8.0, -2.0, -2.0], [32.0, 2.0, 2.0]);
    let find = |mask: u32, exclude: Option<ObjectId>, limit: usize| {
        let mut out = Vec::new();
        set.find(&near, mask, exclude, limit, &mut out);
        out
    };
    assert_eq!(find(STATIC_SHAPE, None, 8), [id(1), id(2), id(3)]);
    // The mask is the shape's, its source's bit finds it too, and another type does not.
    assert_eq!(find(bit, None, 8).len(), 3);
    assert_eq!(find(PLAYER, None, 8).len(), 0);
    assert_eq!(find(STATIC_SHAPE, Some(id(2)), 8), [id(1), id(3)]);
    assert_eq!(find(STATIC_SHAPE, None, 2).len(), 2);
}

#[test]
fn a_set_over_the_cap_holds_none_on_either_side() {
    // Four sources of a full 1024 are more than a set holds: the set is empty, as it is for the client and the server alike.
    let many = |n: usize| -> Vec<CollisionShape> {
        (0..n as u32)
            .map(|k| boxed(k, at(k as f32 * 4.0, 0.0, 0.0), [1.0; 3]))
            .collect()
    };
    let names = ["A", "B", "C", "D", "E"];
    let entries: Vec<PluginCollision> = names
        .iter()
        .flat_map(|n| {
            let s = source(many(MAX_SHAPES_PER_SOURCE));
            [
                PluginCollision::server(*n, s.clone()),
                PluginCollision::client(*n, s),
            ]
        })
        .collect();
    for side in [Side::Server, Side::Client] {
        let src = sources(&entries, side);
        assert_eq!(src.snapshot().len(), 0, "{side:?}");
        // Four are the most that fit.
        let four = &entries[..8];
        assert_eq!(sources(four, side).snapshot().len(), MAX_SET_SHAPES);
    }
}

#[test]
fn names_are_one_whatever_their_case() {
    let s = source(vec![]);
    let entries = [
        PluginCollision::server("wall", s.clone()),
        PluginCollision::server("Wall", s.clone()),
        PluginCollision::client("WALL", s.clone()),
        PluginCollision::client("Alpha", s.clone()),
        PluginCollision::server("staticshape", s.clone()),
    ];
    let (server, warnings) = CollisionSources::from_entries(&entries, Side::Server);
    // The one the order of spelling puts first (`WALL`, though the client alone wrote it) is the name; the second of a side is refused, as is a built-in type's.
    assert_eq!(server.bits().map(|(n, _)| n).collect::<Vec<_>>(), ["WALL"]);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    // `Alpha` sorts first, so `Wall` is the second on both sides, whichever case a side wrote it in.
    let client = sources(&entries, Side::Client);
    assert_eq!(server.bit("wall"), Some(FREE_TYPE_BITS[1]));
    assert_eq!(client.bit("Wall"), Some(FREE_TYPE_BITS[1]));
    assert_eq!(
        source_names(["b", "B", "a", "bad name", "Player"]),
        ["a", "B"]
    );
}

#[test]
fn hostile_poses_are_refused_by_their_rotation_too() {
    let mut big = IDENTITY;
    big[0] = MAX_POSE_SCALE * 2.0;
    let mut scaled = IDENTITY;
    scaled[5] = MAX_POSE_SCALE;
    let entries = [PluginCollision::server(
        "P",
        source(vec![boxed(0, big, [1.0; 3]), boxed(1, scaled, [1.0; 3])]),
    )];
    let set = sources(&entries, Side::Server).snapshot();
    assert_eq!(set.len(), 1);
    assert!(set.object(ObjectId(PLUGIN_ID_BASE + 1)).is_some());
}

#[test]
fn a_hull_must_be_a_convex_solid() {
    let cube = [
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    let tris: [[u32; 3]; 12] = [
        [0, 1, 2],
        [0, 2, 3],
        [4, 6, 5],
        [4, 7, 6],
        [0, 5, 1],
        [0, 4, 5],
        [1, 6, 2],
        [1, 5, 6],
        [2, 7, 3],
        [2, 6, 7],
        [3, 4, 0],
        [3, 7, 4],
    ];
    assert!(PluginHull::new(&cube, &tris).is_some());
    // Wound the other way round: inverted.
    let inverted: Vec<[u32; 3]> = tris.iter().map(|t| [t[0], t[2], t[1]]).collect();
    assert!(PluginHull::new(&cube, &inverted).is_none());
    // A repeated index, and a triangle with no area.
    let mut repeated = tris;
    repeated[0] = [0, 1, 1];
    assert!(PluginHull::new(&cube, &repeated).is_none());
    let mut line = cube;
    line[2] = [1.0, -1.0, -1.0];
    assert!(PluginHull::new(&line, &tris).is_none());
    // A dent: one vertex pushed into the solid.
    let mut dent = cube;
    dent[6] = [0.0, 0.0, 0.0];
    assert!(PluginHull::new(&dent, &tris).is_none());
    // A vertex no triangle uses, and a flat mesh (two triangles over one square).
    let mut extra = cube.to_vec();
    extra.push([0.0, 0.0, 0.0]);
    assert!(PluginHull::new(&extra, &tris).is_none());
    let flat = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    assert!(PluginHull::new(&flat, &[[0, 1, 2], [0, 2, 3]]).is_none());
    assert!(PluginHull::new(&flat, &[[0, 2, 1], [0, 3, 2]]).is_none());
}

/// A world of one set, as a host's world answers for the plugin range.
struct SetWorld(ShapeSet);

impl World for SetWorld {
    fn object(&self, id: ObjectId) -> Option<ObjectView> {
        self.0.object(id)
    }

    fn find_objects(
        &mut self,
        query: &Box3,
        mask: u32,
        exclude: Option<ObjectId>,
        limit: usize,
        out: &mut Vec<ObjectId>,
    ) -> Result<(), Exhausted> {
        self.0.find(query, mask, exclude, limit, out);
        Ok(())
    }

    fn convex_source(&self, id: ObjectId) -> ConvexSource {
        self.0.convex_source(id)
    }

    fn hull(&self, id: ObjectId, slot: u32) -> Option<Rc<Hull>> {
        self.0.hull(id, slot)
    }

    fn charge(&mut self, _: u64) -> bool {
        true
    }
}

#[test]
fn a_working_set_builds_boxes_and_hulls_and_follows_a_move() {
    let hull = Rc::new(PluginHull::cuboid([1.0; 3]).unwrap());
    let s = source(vec![
        boxed(0, at(0.0, 0.0, 0.0), [1.0, 2.0, 3.0]),
        CollisionShape {
            key: 1,
            pose: at(20.0, 0.0, 0.0),
            kind: ShapeKind::Hull(hull),
            type_mask: STATIC_SHAPE,
        },
    ]);
    let entries = [PluginCollision::server("P", s.clone())];
    let src = sources(&entries, Side::Server);
    let mut world = SetWorld(src.snapshot());
    let mut working = WorkingSet::default();
    let query = Box3::new([-5.0; 3], [25.0, 5.0, 5.0]);
    working
        .update(&mut world, ObjectId(1), &query, STATIC_SHAPE)
        .unwrap();
    let kinds: Vec<ConvexKind> = working.to_vec().iter().map(|c| c.kind).collect();
    assert_eq!(kinds.len(), 2);
    assert!(kinds.contains(&ConvexKind::Box {
        center: [0.0; 3],
        size: [1.0, 2.0, 3.0]
    }));
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, ConvexKind::Hull { slot: 0, .. }))
    );
    // The box is resized and moved: the next phase's set holds the new pose, and the convex is made again from it.
    s.0.borrow_mut()[0] = boxed(0, at(0.0, 7.0, 0.0), [2.0, 2.0, 3.0]);
    let mut world = SetWorld(src.snapshot());
    let query = Box3::new([-5.0, 2.0, -5.0], [5.0, 12.0, 5.0]);
    working
        .update(&mut world, ObjectId(1), &query, STATIC_SHAPE)
        .unwrap();
    let boxes: Vec<_> = working
        .to_vec()
        .into_iter()
        .filter_map(|c| match c.kind {
            ConvexKind::Box { size, .. } => Some((c.transform(&world).unwrap()[7], size)),
            _ => None,
        })
        .collect();
    assert_eq!(boxes, [(7.0, [2.0, 2.0, 3.0])]);
    // The hull, now out of the query box, is gone from the set; the box out of reach of a later query too.
    assert_eq!(working.len(), 1);
    let far = Box3::new([100.0; 3], [101.0; 3]);
    working
        .update(&mut world, ObjectId(1), &far, STATIC_SHAPE)
        .unwrap();
    assert!(working.is_empty());
}
