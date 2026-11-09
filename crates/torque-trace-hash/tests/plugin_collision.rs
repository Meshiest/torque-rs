//! A plugin's collision source in a server and a predicting client: a figure walks into a box (and a hull) a plugin owns, over
//! 600 ticks, and the server's container and the client's prediction agree on it.

use std::cell::RefCell;
use std::rc::Rc;

use torque_collide::source::{
    CollisionShape, CollisionSource, MAX_SHAPES_PER_SOURCE, PluginCollision, PluginHull, ShapeKind,
    Side,
};
use torque_engine::{EngineBuilder, Plugin};
use torque_trace_hash::rig::{Control, Rig};

const SETTLE: u32 = 400;
const TICKS: u32 = 600;
/// A static shape's type bit: what the figure's movers collide with.
const STATIC_SHAPE: u32 = 1 << 13;

/// The shapes a plugin owns, read by its server's source and its client's alike.
#[derive(Debug, Clone, Default)]
struct Shapes(Rc<RefCell<Vec<CollisionShape>>>);

impl CollisionSource for Shapes {
    fn shapes(&self, out: &mut Vec<CollisionShape>) {
        out.extend(self.0.borrow().iter().cloned());
    }
}

impl Shapes {
    fn set(&self, shapes: Vec<CollisionShape>) {
        *self.0.borrow_mut() = shapes;
    }
}

/// A plugin with a server's source; a test gives the client its own through the rig.
#[derive(Clone, Debug, torque_engine::PluginId)]
#[plugin(id = "test.plugin-collision")]
struct CollisionPlugin {
    shapes: Shapes,
}

impl Plugin for CollisionPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.push(PluginCollision::server(
            "Wall",
            Rc::new(self.shapes.clone()),
        ));
    }
}

fn pose(x: f32, y: f32, z: f32) -> [f32; 16] {
    let mut m = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    (m[3], m[7], m[11]) = (x, y, z);
    m
}

fn cuboid(key: u32, at: [f32; 3], half: [f32; 3], hull: bool) -> CollisionShape {
    CollisionShape {
        key,
        pose: pose(at[0], at[1], at[2]),
        kind: if hull {
            ShapeKind::Hull(Rc::new(PluginHull::cuboid(half).unwrap()))
        } else {
            ShapeKind::Box { half }
        },
        type_mask: STATIC_SHAPE,
        passes: None,
    }
}

/// A rig whose server has the plugin's source and, when `client`, whose client has it too.
fn rig(shapes: &Shapes, client: bool) -> Rig {
    let plugin = CollisionPlugin {
        shapes: shapes.clone(),
    };
    let mut rig = Rig::new(plugin, Control::Figure).unwrap();
    if client {
        let entries = [
            PluginCollision::server("Wall", Rc::new(shapes.clone())),
            PluginCollision::client("Wall", Rc::new(shapes.clone())),
        ];
        let warnings =
            torque_game::plugin_collision::install(&mut rig.client_vm, &entries, Side::Client);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
    assert!(rig.settle(SETTLE), "{:?}", rig.server.drain_log());
    rig
}

fn server_at(rig: &Rig) -> [f32; 3] {
    let vm = rig.server.vm();
    let id = vm.objects().find(b"TraceFigure").unwrap();
    torque_game::sim::player::position(vm, id).unwrap()
}

fn client_at(rig: &Rig) -> [f32; 3] {
    let st = rig.client.borrow();
    let p = &st.conn.conn.as_ref().unwrap().predict;
    p.predicted.as_ref().unwrap().player.s.position()
}

fn corrections(rig: &Rig) -> u64 {
    let st = rig.client.borrow();
    st.conn.conn.as_ref().unwrap().predict.stats.corrections
}

fn walk(rig: &mut Rig, ticks: u32) {
    rig.set_global("$mvForwardAction", 1.0);
    for _ in 0..ticks {
        rig.step();
    }
}

fn gap(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

#[test]
fn a_figure_stops_at_a_plugin_wall_and_the_prediction_agrees() {
    for hull in [false, true] {
        // A wall 4 high across the way, its near face at y = 7.5.
        let shapes = Shapes::default();
        shapes.set(vec![cuboid(0, [0.0, 8.0, 2.0], [6.0, 0.5, 2.0], hull)]);
        let mut rig = rig(&shapes, true);
        let before = corrections(&rig);
        walk(&mut rig, TICKS);
        let (s, c) = (server_at(&rig), client_at(&rig));
        // The figure is half a unit deep: it stands against the face, not in the wall and not far from it.
        assert!(s[1] > 6.8 && s[1] < 7.1, "hull {hull}: server {s:?}");
        assert!(gap(s, c) < 1e-3, "hull {hull}: server {s:?} client {c:?}");
        assert_eq!(corrections(&rig), before, "hull {hull}");
    }
}

#[test]
fn without_the_shape_the_figure_walks_through() {
    let shapes = Shapes::default();
    let mut rig = rig(&shapes, true);
    walk(&mut rig, TICKS);
    assert!(server_at(&rig)[1] > 20.0);
}

#[test]
fn a_client_without_the_source_is_corrected() {
    let shapes = Shapes::default();
    shapes.set(vec![cuboid(0, [0.0, 8.0, 2.0], [6.0, 0.5, 2.0], false)]);
    let mut rig = rig(&shapes, false);
    let before = corrections(&rig);
    walk(&mut rig, TICKS);
    assert!(corrections(&rig) > before);
}

#[test]
fn a_figure_steps_onto_a_low_plugin_box_and_stands_on_it() {
    for hull in [false, true] {
        // A slab 0.5 high (under the figure's step height), long enough to stand on for the whole walk.
        let shapes = Shapes::default();
        shapes.set(vec![cuboid(0, [0.0, 90.0, 0.25], [6.0, 90.0, 0.25], hull)]);
        let mut rig = rig(&shapes, true);
        let before = corrections(&rig);
        walk(&mut rig, TICKS);
        let (s, c) = (server_at(&rig), client_at(&rig));
        assert!(s[1] > 20.0, "hull {hull}: {s:?}");
        // Still walking: the client is ahead of the server by the ticks in flight, at the same height.
        assert!((s[2] - 0.5).abs() < 0.05, "hull {hull}: server {s:?}");
        assert!((c[2] - 0.5).abs() < 0.05, "hull {hull}: client {c:?}");
        assert_eq!(corrections(&rig), before, "hull {hull}");
    }
}

#[test]
fn a_moved_box_is_seen_at_its_new_pose_on_both_sides() {
    for hull in [false, true] {
        let shapes = Shapes::default();
        shapes.set(vec![cuboid(0, [0.0, 8.0, 2.0], [6.0, 0.5, 2.0], hull)]);
        let mut rig = rig(&shapes, true);
        let before = corrections(&rig);
        walk(&mut rig, 200);
        let stopped = server_at(&rig);
        assert!(stopped[1] < 7.1, "hull {hull}: {stopped:?}");
        // The wall moves 30 units on between two ticks: the figure goes on and meets it at its new face.
        shapes.set(vec![cuboid(0, [0.0, 38.0, 2.0], [6.0, 0.5, 2.0], hull)]);
        for _ in 0..300 {
            rig.step();
        }
        let (s, c) = (server_at(&rig), client_at(&rig));
        assert!(s[1] > 36.8 && s[1] < 37.1, "hull {hull}: server {s:?}");
        assert!(gap(s, c) < 1e-3, "hull {hull}: server {s:?} client {c:?}");
        // The client runs ahead of the server by the ticks in flight, so it met the wall's old pose a tick or two after the
        // server had moved it: one correction at the move, none after it.
        assert!(corrections(&rig) - before <= 1, "hull {hull}");
    }
}

#[test]
fn the_shapes_past_the_cap_do_not_block() {
    // One more wall than a source may have, the last in the figure's way: it is dropped on both sides.
    let shapes = Shapes::default();
    let mut walls: Vec<CollisionShape> = (0..MAX_SHAPES_PER_SOURCE as u32)
        .map(|k| {
            cuboid(
                k,
                [500.0 + 20.0 * k as f32, 0.0, 2.0],
                [1.0, 1.0, 2.0],
                false,
            )
        })
        .collect();
    walls.push(cuboid(
        MAX_SHAPES_PER_SOURCE as u32,
        [0.0, 8.0, 2.0],
        [6.0, 0.5, 2.0],
        false,
    ));
    shapes.set(walls);
    let mut rig = rig(&shapes, true);
    let before = corrections(&rig);
    walk(&mut rig, 300);
    assert!(server_at(&rig)[1] > 20.0);
    assert_eq!(corrections(&rig), before);
}
