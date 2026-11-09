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
