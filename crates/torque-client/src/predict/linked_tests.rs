//! The prediction's placement of scene-hierarchy children ([`place_linked`]): on a parent's node read from its shape, carried
//! by the predicted object, and within its work budget whatever tree a server sends.

use std::time::{Duration, Instant};

use ghost_world::tge::{TgeClientSet, TgeDatablock};
use ghost_world::world::Ghost;
use ghosts::block::{GhostBlock, LinkField, PoseField};
use ghosts::shapes::StaticShape;
use ghosts::{F32, TgeClass, TgeRecord, TgeShapeBase, Value, Xf};
use hierarchy::{HARD_CHILDREN, HARD_DEPTH, HARD_DESCENDANTS, Local, QuantLocal};

use super::*;
use crate::conn::tge::TgeWorld;

const SHAPE: &[u8] = b"kit/stand.dts";
const DATABLOCK: u32 = 3;

/// A StaticShape ghost at `at`, of the datablock whose shape has a `mount0` node two units over its origin.
fn shape_ghost(at: [f32; 3]) -> Ghost<TgeClientSet> {
    let record = StaticShape {
        shape: TgeShapeBase {
            game: ghosts::parts::GameBase {
                scale: None,
                datablock: Some(DATABLOCK),
            },
            body: None,
        },
        transform: Some((
            Xf {
                position: at.map(F32::new),
                quat: [F32::new(0.0); 3],
                negative_w: false,
            },
            [F32::new(1.0); 3],
        )),
        powered: false,
    };
    Ghost {
        class: Some(TgeClass::StaticShape),
        serial: 1,
        always: false,
        record: Some(TgeRecord::StaticShape(record)),
        part: (),
        position: None,
        updates: 0,
    }
}

fn reader() -> FileReader {
    let b = [-0.5, -0.5, -0.5, 0.5, 0.5, 0.5];
    let mut model = shapes::synth::hull_shape(b, &[("detail32", b)], &["mount0"], &[]);
    model.shape.default_translations[1] = [0.0, 0.0, 2.0];
    let bytes = shapes::write::write_dts(&model);
    Rc::new(move |path: &[u8]| (path == SHAPE).then(|| bytes.clone()))
}

fn world() -> TgeWorld {
    let mut w = TgeWorld::default();
    w.records.datablocks.insert(
        DATABLOCK,
        TgeDatablock {
            class: "StaticShapeData".into(),
            fields: vec![("shapeFile".into(), 0, Value::str("kit/stand.dts"))],
        },
    );
    w
}

/// Hangs ghost `child` on `parent` (on its node `node`) at `translation`, as a server's block does.
fn link(w: &mut TgeWorld, child: u32, parent: u32, node: Option<u16>, translation: [f32; 3]) {
    let q = QuantLocal::from_local(&Local {
        translation,
        ..Local::IDENTITY
    });
    let block = GhostBlock {
        link: Some(LinkField::Parent {
            ghost: parent,
            node,
        }),
        pose: Some(PoseField {
            translation: q.translation,
            dropped: q.rotation.dropped,
            parts: q.rotation.parts,
            scale: None,
            snap: true,
        }),
    };
    let records = &mut w.records;
    let ghosts = &records.ghosts;
    records
        .links
        .apply(child, &block, &|i| ghosts.contains_key(&i), 0, 0);
}

/// The container's object of ghost `index`, at the origin.
fn object(index: u32) -> (ObjectId, Object) {
    let b = Box3::new([-0.5; 3], [0.5; 3]);
    let object = Object {
        view: view_of(collide::math::IDENTITY, b, [1.0; 3], STATIC_SHAPE_TYPE_MASK),
        source: Source::Torque(ConvexSource::None),
        shape: None,
        map: None,
    };
    (ObjectId(index + 1), object)
}

fn linked<'a>(
    shapes: Option<&'a mut drive::ShapeCache>,
    reader: Option<&'a FileReader>,
    me: u32,
) -> Linked<'a> {
    Linked {
        shapes,
        reader,
        driven: None,
        ghosts: None,
        player_at: None,
        me,
    }
}

fn position(o: &(ObjectId, Object)) -> [f32; 3] {
    o.1.view.position()
}

#[test]
fn a_child_sits_on_its_parents_node_and_the_predicted_objects_children_go_with_it() {
    let mut w = world();
    w.records.ghosts.insert(1, shape_ghost([10.0, 0.0, 0.0]));
    w.records.ghosts.insert(2, shape_ghost([0.0; 3]));
    // Ghost 5 stands in for the predicted object; its record still has it at the origin.
    w.records.ghosts.insert(5, shape_ghost([0.0; 3]));
    w.records.ghosts.insert(3, shape_ghost([0.0; 3]));
    link(&mut w, 2, 1, Some(1), [1.0, 0.0, 0.0]);
    link(&mut w, 3, 5, None, [0.0, 1.0, 0.0]);
    let mut objects = vec![object(2), object(3)];
    let (reader, mut cache, mut work) = (reader(), drive::ShapeCache::default(), 0);
    let carried = place_linked(
        &w,
        &mut objects,
        linked(Some(&mut cache), Some(&reader), 5),
        &mut ghost_world::world::TickPoses::default(),
        &mut work,
        TICK_WORK,
    );
    // On `mount0` (two units over the parent's origin), one unit along x.
    assert_eq!(position(&objects[0]), [11.0, 0.0, 2.0]);
    // From the predicted object's record, then from where the object is before each move.
    assert_eq!(position(&objects[1]), [0.0, 1.0, 0.0]);
    assert_eq!(carried.len(), 1);
    let mut c = ClientCollision::empty();
    c.by_id = index_of(&objects);
    c.objects = objects;
    c.carried = carried;
    let mut at = collide::math::IDENTITY;
    [at[3], at[7], at[11]] = [5.0, 5.0, 0.0];
    c.carry_children(&at);
    let view = c.get(ObjectId(4)).expect("the carried child").view;
    assert_eq!(view.position(), [5.0, 6.0, 0.0]);
    assert_eq!(
        c.get(ObjectId(3)).expect("the node child").view.position(),
        [11.0, 0.0, 2.0]
    );
}

/// The biggest tree a client's links hold: deep chains under seven roots and an eighth root with the most children, each with
/// children of its own. Every ghost but the roots is a child and has an object in the container.
fn hostile_world() -> (TgeWorld, Vec<(ObjectId, Object)>) {
    let mut w = world();
    let mut next = 1u32;
    let mut make = |w: &mut TgeWorld| {
        let i = next;
        next += 1;
        w.records.ghosts.insert(i, shape_ghost([0.0; 3]));
        i
    };
    let mut children = Vec::new();
    let chains = HARD_DESCENDANTS / HARD_DEPTH - 1;
    for _ in 0..7 {
        let root = make(&mut w);
        for _ in 0..chains {
            let mut parent = root;
            for _ in 0..HARD_DEPTH {
                let child = make(&mut w);
                link(&mut w, child, parent, None, [0.0, 0.0, 1.0]);
                children.push(child);
                parent = child;
            }
        }
    }
    let root = make(&mut w);
    let grandchildren = HARD_DESCENDANTS / HARD_CHILDREN - 2;
    for _ in 0..HARD_CHILDREN {
        let child = make(&mut w);
        link(&mut w, child, root, Some(1), [1.0, 0.0, 0.0]);
        children.push(child);
        for _ in 0..grandchildren {
            let grandchild = make(&mut w);
            link(&mut w, grandchild, child, Some(1), [0.0, 1.0, 0.0]);
            children.push(grandchild);
        }
    }
    assert_eq!(
        w.records.links.linked_count(),
        children.len(),
        "every link was held"
    );
    let objects = children.into_iter().map(object).collect();
    (w, objects)
}

#[test]
fn a_hostile_tree_is_placed_within_the_ticks_budget_each_ghost_worked_out_once() {
    let (w, objects) = hostile_world();
    let ghosts = objects.len() as u64;
    let reader = reader();
    // Within a tick's budget: placing stops once it is spent, quickly.
    let mut cut = objects.clone();
    let (mut cache, mut work) = (drive::ShapeCache::default(), 0);
    let start = Instant::now();
    place_linked(
        &w,
        &mut cut,
        linked(Some(&mut cache), Some(&reader), u32::MAX),
        &mut ghost_world::world::TickPoses::default(),
        &mut work,
        TICK_WORK,
    );
    let took = start.elapsed();
    println!("{ghosts} children in a tick's budget: charged {work} in {took:?}");
    assert!(work <= TICK_WORK + LINK_WORK, "charged {work}");
    assert!(took < Duration::from_millis(250), "took {took:?}");
    // Without a budget each link is walked at most twice to read the nodes' shapes and each ghost worked out once to place
    // the children, whatever chains share it: a walk of each child's whole chain would be more than ten times as much. The
    // shape is read before, so only the links are charged.
    let mut all = objects;
    let mut cache = drive::ShapeCache::default();
    cache.get(SHAPE, Some(&reader), &mut 0, u64::MAX);
    let mut work = 0;
    place_linked(
        &w,
        &mut all,
        linked(Some(&mut cache), Some(&reader), u32::MAX),
        &mut ghost_world::world::TickPoses::default(),
        &mut work,
        u64::MAX,
    );
    println!("{ghosts} children without a budget: charged {work}");
    assert!(
        work <= 3 * (ghosts + 8) * LINK_WORK,
        "charged {work} for {ghosts} children"
    );
    let deepest = all.iter().map(|o| position(o)[2]).fold(0.0, f32::max);
    assert_eq!(
        deepest, HARD_DEPTH as f32,
        "the deepest child is on its whole chain"
    );
}

/// A container over `objects` of `w`'s hostile tree, its children placed as a tick's construction does.
fn hostile_container(w: &TgeWorld, objects: Vec<(ObjectId, Object)>) -> ClientCollision {
    let (reader, mut cache) = (reader(), drive::ShapeCache::default());
    let mut c = ClientCollision::empty();
    c.by_id = index_of(&objects);
    c.objects = objects;
    c.place_children(w, linked(Some(&mut cache), Some(&reader), u32::MAX));
    c
}

#[test]
fn a_hostile_tree_leaves_the_moves_half_of_the_ticks_budget() {
    let (w, objects) = hostile_world();
    let mut c = hostile_container(&w, objects);
    // The tree outran the children's cap: the placement took all of it and no more.
    let cap = c.child_cap();
    assert!(
        (cap - LINK_WORK..=cap + LINK_WORK).contains(&c.work()),
        "children took {}",
        c.work()
    );
    assert!(!c.exhausted());
    // The moves' own work (a query's candidates) still fits, up to the other half.
    let left = TICK_WORK - c.work();
    assert!(left >= TICK_WORK / 2);
    assert!(collide::World::charge(&mut c, left));
    assert!(!collide::World::charge(&mut c, 1));
}

/// A world of `count` StaticShape ghosts (index 1 up), each also an object a construction makes; the first `linked` of the
/// others hung in chains of [`HARD_DEPTH`] on the first.
fn tree_world(count: u32, linked: u32) -> TgeWorld {
    let mut w = world();
    for i in 1..=count {
        w.records.ghosts.insert(i, shape_ghost([0.0; 3]));
    }
    {
        let depth = HARD_DEPTH as u32;
        for i in 2..=(linked + 1).min(count) {
            let parent = if (i - 2) % depth == 0 { 1 } else { i - 1 };
            link(&mut w, i, parent, None, [0.0, 0.0, 1.0]);
        }
    }
    w
}

/// The container a tick makes for the predicted object `u32::MAX`: the ghost walk, then the children.
fn built(w: &TgeWorld) -> ClientCollision {
    let mut statics = StaticWorld::<TgeWorld> {
        reader: Some(reader()),
        ..StaticWorld::default()
    };
    let mut cache = drive::ShapeCache::default();
    ClientCollision::new(
        w,
        u32::MAX,
        TICK_WORK,
        Some(Ctx {
            near: [0.0; 3],
            static_world: &mut statics,
            shapes: &mut cache,
            driven: None,
            ghosts: None,
            disabled: None,
            player_at: None,
        }),
    )
}

#[test]
fn children_stop_at_their_cap_of_the_whole_ticks_work_after_a_real_ghost_walk() {
    // 16,000 children and 16,001 ghosts to walk: the walk takes about a sixth of the budget, the placement would take well over
    // what is left of the children's cap.
    let walked = built(&tree_world(16_001, 0));
    let mut c = built(&tree_world(16_001, 16_000));
    assert!(!c.objects.is_empty(), "the walk made objects");
    assert!(walked.work() > 60_000, "the walk took {}", walked.work());
    let cap = c.child_cap();
    assert!(
        (cap - LINK_WORK..=cap + LINK_WORK).contains(&c.work()),
        "walk {} and children {}, cap {cap}",
        walked.work(),
        c.work()
    );
    assert!(!c.exhausted());
    let left = TICK_WORK - c.work();
    assert!(left >= TICK_WORK / 2);
    assert!(collide::World::charge(&mut c, left));
}

#[test]
fn a_ghost_walk_that_took_the_childrens_cap_leaves_them_nothing() {
    // 40,000 ghosts to walk: the walk alone is past the children's cap, so a tree among them costs nothing more.
    let alone = built(&tree_world(40_000, 0));
    let tree = built(&tree_world(40_000, 16_000));
    assert!(alone.work() >= tree.child_cap(), "{}", alone.work());
    assert_eq!(tree.work(), alone.work());
    assert!(tree.carried.is_empty());
}

#[test]
fn the_frame_is_placed_past_the_childrens_cap_on_a_slice_of_its_own_and_no_ghost_is_charged_twice()
{
    let (w, objects) = hostile_world();
    let mut c = hostile_container(&w, objects);
    // A predicted object whose children are all the ones the tree has: the cap is spent, so the carry takes nothing.
    let ids: Vec<ObjectId> = c.objects.iter().map(|(id, _)| *id).collect();
    c.carried = ids
        .iter()
        .map(|&id| CarriedChild {
            id,
            rel: collide::math::IDENTITY,
            upright: false,
        })
        .collect();
    let before = c.work();
    c.carry_children(&collide::math::IDENTITY);
    assert_eq!(c.work(), before, "the children's cap was spent");
    // The predicted object's own frame hangs on ghost 1. The placement's poses are stopped, which refuses every later call
    // on them.
    let frame = framed::ChildFrame {
        parent: 1,
        node: None,
        local: Local::IDENTITY,
        at: None,
    };
    assert!(
        w.tick_pose_in(2, &|_, _| None, &|_| None, &mut c.tick_poses, &mut || false)
            .is_none()
    );
    assert!(c.tick_poses.stopped());
    let (reader, mut cache) = (reader(), drive::ShapeCache::default());
    let mut own = linked(Some(&mut cache), Some(&reader), u32::MAX);
    let at = c.attach_frame(&w, &mut own, &frame);
    assert!(at.is_some(), "the frame's parent was not placed");
    let placed = c.work();
    assert!(
        placed - before <= c.frame_share(),
        "the frame took {}",
        placed - before
    );
    assert!(!c.exhausted());
    // Asked again (the poses kept), the same chain costs nothing.
    assert!(c.attach_frame(&w, &mut own, &frame).is_some());
    assert_eq!(c.work(), placed, "a ghost was charged twice");
    // The frame's chain is bounded by its slice: a parent a hostile depth down is refused, not charged without end.
    let mut deep = world();
    for i in 1..=200u32 {
        deep.records.ghosts.insert(i, shape_ghost([0.0; 3]));
        if i > 1 {
            link(&mut deep, i, i - 1, None, [0.0, 0.0, 1.0]);
        }
    }
    let mut c = ClientCollision::empty();
    c.work = TICK_WORK - 10;
    let frame = framed::ChildFrame {
        parent: 200,
        ..frame
    };
    assert!(c.attach_frame(&deep, &mut own, &frame).is_none());
    assert!(c.work() <= TICK_WORK + 2 * LINK_WORK);
}

#[test]
fn a_few_children_are_all_carried_within_the_share() {
    let mut w = world();
    w.records.ghosts.insert(5, shape_ghost([0.0; 3]));
    let mut objects = Vec::new();
    for i in 10..30 {
        w.records.ghosts.insert(i, shape_ghost([0.0; 3]));
        link(&mut w, i, 5, None, [0.0, 1.0, 0.0]);
        objects.push(object(i));
    }
    let (reader, mut cache) = (reader(), drive::ShapeCache::default());
    let mut c = ClientCollision::empty();
    c.by_id = index_of(&objects);
    c.objects = objects;
    c.place_children(&w, linked(Some(&mut cache), Some(&reader), 5));
    assert_eq!(c.carried.len(), 20);
    let before = c.work();
    let mut at = collide::math::IDENTITY;
    at[3] = 7.0;
    c.carry_children(&at);
    assert_eq!(c.work(), before + 20 * GHOST_WORK);
    assert_eq!(
        c.get(ObjectId(11)).expect("a child").view.position(),
        [7.0, 1.0, 0.0]
    );
}

#[test]
fn the_moves_keep_half_the_budget_under_a_huge_walk_a_hostile_tree_and_a_deep_frame_chain() {
    // Ghost 33 hangs at the end of a chain of the deepest depth; the predicted object's frame hangs on it.
    let frame = framed::ChildFrame {
        parent: 33,
        node: None,
        local: Local::IDENTITY,
        at: None,
    };
    for ghosts in [100_000, 16_001, 5_000] {
        let w = tree_world(ghosts, 16_000);
        let mut c = built(&w);
        assert_eq!(c.walk_stopped(), ghosts == 100_000, "{ghosts}");
        let reserve = c.moves_reserve();
        assert_eq!(reserve, TICK_WORK / 2);
        // The children it carries: every object of the tree.
        c.carried = c
            .objects
            .iter()
            .map(|(id, _)| CarriedChild {
                id: *id,
                rel: collide::math::IDENTITY,
                upright: false,
            })
            .collect();
        let mut own = linked(None, None, u32::MAX);
        let at = c.attach_frame(&w, &mut own, &frame);
        c.carry_children(&collide::math::IDENTITY);
        println!(
            "{ghosts} ghosts: walk and children and frame took {}",
            c.work()
        );
        assert!(
            c.work() <= TICK_WORK - reserve,
            "{ghosts} ghosts: {} before the moves",
            c.work()
        );
        if ghosts < 100_000 {
            assert!(at.is_some(), "{ghosts} ghosts: the deep frame was placed");
        }
        // The moves still get the half: their work is charged and fits.
        assert!(!c.exhausted());
        assert!(collide::World::charge(&mut c, reserve), "{ghosts} ghosts");
    }
}

#[test]
fn what_a_part_asks_of_the_world_is_made_on_the_budget_for_parts_and_charged_to_it() {
    let w = tree_world(100_000, 16_000);
    let mut p = Predictor::<TgeWorld>::new(".".into());
    p.static_world.reader = Some(reader());
    let region = ([-10.0; 3], [10.0; 3]);
    // A hostile world's look, the queries' work included, costs the parts' budget no more than one tick's budget of its own
    // (`TICK_WORK`); the predictor's tick is another, its construction held to half of it.
    let tick = built(&w).work();
    let around = p.around(&w, Some(region), Some(region));
    assert!(around.built);
    assert!(tick <= TICK_WORK / 2, "{tick}");
    assert!(p.part_work > tick, "the look was not charged");
    assert!(
        p.part_work <= TICK_WORK + collide::work::CANDIDATE,
        "around {}",
        p.part_work
    );
    // Asked for nothing it looks at nothing; with no budget for parts left it does not look, and says so.
    let before = p.part_work;
    assert!(p.around(&w, None, None).built);
    assert_eq!(p.part_work, before);
    p.part_work = slot::PART_TICK_WORK;
    assert!(!p.around(&w, Some(region), Some(region)).built);
    assert_eq!(p.part_work, slot::PART_TICK_WORK);
    // With a little left it is made on that and no more (the queries may run out of it: then it is not built).
    p.part_work = slot::PART_TICK_WORK - 1_000;
    let _ = p.around(&w, Some(region), Some(region));
    assert!(
        p.part_work <= slot::PART_TICK_WORK + collide::work::CANDIDATE,
        "{}",
        p.part_work
    );
}

#[test]
fn the_work_of_a_parts_queries_is_charged_and_a_query_that_runs_out_is_no_look() {
    // 3,000 static shapes in the region: the bodies query looks at every one of them, the statics query stops at its limit.
    let w = tree_world(3_000, 0);
    let region = ([-10.0; 3], [10.0; 3]);
    let walk = built(&w).work();
    let mut p = Predictor::<TgeWorld>::new(".".into());
    p.static_world.reader = Some(reader());
    let around = p.around(&w, Some(region), Some(region));
    assert!(around.built);
    assert!(around.bodies.is_empty() && !around.statics.is_empty());
    let queries = 3_000 * collide::work::CANDIDATE;
    assert!(
        p.part_work >= walk + queries,
        "walk {walk}, charged {}: the queries were not counted",
        p.part_work
    );
    // A collision with room for the walk's objects but not for a query over them: each query runs out.
    let mut c = built(&w);
    assert!(c.objects.len() > 1_000);
    c.budget = c.work() + 1_000;
    let seen = look(&mut c, Some(region), Some(region));
    assert!(!seen.built);
    assert!(seen.statics.is_empty() && seen.bodies.is_empty());
    // Either query alone does it.
    for (statics, bodies) in [(Some(region), None), (None, Some(region))] {
        let mut c = built(&w);
        c.budget = c.work() + 100;
        // The statics query stops at its limit after 257 candidates, so it needs the smaller room.
        assert!(
            !look(&mut c, statics, bodies).built,
            "{statics:?} {bodies:?}"
        );
    }
    // With room it is built, and asking for nothing is built too.
    let mut c = built(&w);
    assert!(look(&mut c, Some(region), Some(region)).built);
    assert!(look(&mut c, None, None).built);
}
