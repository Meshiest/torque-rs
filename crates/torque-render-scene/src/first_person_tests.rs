use std::collections::{BTreeSet, HashMap};

use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;
use ghosts::shapes::StaticShape;
use ghosts::{F32, TgeClass, TgeRecord, TgeShapeBase, Xf};
use shapes::shape::Model;
use torque_shape_builder::{FULL_UV, MeshData, ShapeBuilder};

use super::*;
use crate::moving::{
    ShapeDraws, ShapeSelect, draws_culled, is_shadow_body, is_view_model, with_shadow_body,
};
use crate::shapes::DetailPolicy;
use crate::textures::TextureSet;
use crate::world::Value;
use crate::world::tests::{datablock, ghost};
use crate::{Assets, Draw};

const FIGURE: &str = "demo/figure.dts";

/// A figure's parts: each a box on its node (the nodes all at the origin, so each box stays where it is built).
const PARTS: [(&str, &str, [f32; 3], [f32; 3]); 6] = [
    ("torso", "torso", [-0.2, -0.1, 1.0], [0.2, 0.1, 1.6]),
    ("head", "head", [-0.15, -0.15, 1.7], [0.15, 0.15, 2.0]),
    ("upperarm_r", "arm_r", [0.3, -0.05, 1.2], [0.4, 0.05, 1.6]),
    ("hand_r", "elbow_r", [0.3, -0.05, 0.8], [0.4, 0.05, 1.0]),
    ("upperarm_l", "arm_l", [-0.4, -0.05, 1.2], [-0.3, 0.05, 1.6]),
    ("leg_l", "leg_l", [-0.2, -0.05, 0.0], [-0.1, 0.05, 0.9]),
];

/// The figure: `root`, `torso` on it, `head`, `arm_l` and `arm_r` on the torso, `elbow_r` on `arm_r`, `leg_l` on the root.
fn figure() -> Model {
    let mut b = ShapeBuilder::new();
    let root = b.node("root", None, [0.0; 3]);
    let torso = b.node("torso", Some(root), [0.0; 3]);
    let mut nodes = HashMap::from([("root", root), ("torso", torso)]);
    for (name, parent) in [
        ("head", torso),
        ("arm_l", torso),
        ("arm_r", torso),
        ("leg_l", root),
    ] {
        nodes.insert(name, b.node(name, Some(parent), [0.0; 3]));
    }
    nodes.insert("elbow_r", b.node("elbow_r", Some(nodes["arm_r"]), [0.0; 3]));
    let drawn = b.detail("detail1", 1.0);
    let m = b.material("figure", 0);
    for (name, node, min, max) in PARTS {
        let mut mesh = MeshData::new(Some(m));
        mesh.add_box(min, max, |_| FULL_UV);
        b.object(name, nodes[node], drawn, mesh);
    }
    b.build()
}

struct Files(HashMap<String, Vec<u8>>);

impl Assets for Files {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(&path.to_ascii_lowercase()).cloned()
    }
}

/// A world of the figure (ghost 1, a StaticShape at the origin) as the client's control object.
fn world() -> (ClientWorld<TgeClientSet>, Files) {
    let files = Files([(FIGURE.to_string(), shapes::write::write_dts(&figure()))].into());
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        3,
        datablock("StaticShapeData", &[("shapeFile", Value::str(FIGURE))]),
    );
    let record = StaticShape {
        shape: TgeShapeBase {
            game: ghosts::parts::GameBase {
                scale: None,
                datablock: Some(3),
            },
            body: None,
        },
        transform: Some((
            Xf {
                position: [F32::new(0.0); 3],
                quat: [F32::new(0.0); 3],
                negative_w: false,
            },
            [F32::new(1.0); 3],
        )),
        powered: false,
    };
    w.ghosts.insert(
        1,
        ghost(TgeClass::StaticShape, TgeRecord::StaticShape(record)),
    );
    w.control.object = Some(1);
    (w, files)
}

fn draws(w: &ClientWorld<TgeClientSet>, files: &Files) -> ShapeDraws {
    let mut textures = TextureSet::new(files);
    draws_culled(
        w,
        &(),
        files,
        &mut textures,
        &mut Vec::new(),
        DetailPolicy::Highest,
        ShapeSelect::All,
        None,
    )
}

/// The parts `draws` draw, told by where their vertices are (the parts' boxes do not touch; a draw may hold several).
fn parts(draws: &[&Draw]) -> BTreeSet<&'static str> {
    draws
        .iter()
        .flat_map(|d| d.vertices.iter())
        .map(|v| {
            PARTS
                .iter()
                .find(|(_, _, min, max)| {
                    (0..3).all(|k| min[k] - 1e-4 <= v.position[k] && v.position[k] <= max[k] + 1e-4)
                })
                .map_or("?", |p| p.0)
        })
        .collect()
}

fn set(names: &[&'static str]) -> BTreeSet<&'static str> {
    names.iter().copied().collect()
}

#[test]
fn the_named_nodes_and_every_node_under_them_are_shown() {
    let model = figure();
    let shape = &model.shape;
    let name = |o: &shapes::shape::Object| {
        String::from_utf8(shape.names[o.name as usize].clone()).unwrap()
    };
    let hidden =
        hidden_objects(shape, &[b"arm_r".to_vec(), b"ARM_L".to_vec()]).expect("named nodes");
    let shown: BTreeSet<String> = shape
        .objects
        .iter()
        .zip(&hidden)
        .filter(|(_, h)| !**h)
        .map(|(o, _)| name(o))
        .collect();
    let want: BTreeSet<String> = ["upperarm_r", "hand_r", "upperarm_l"]
        .map(String::from)
        .into();
    assert_eq!(shown, want);
    // None named, or none the shape has: nothing of the body is shown.
    assert_eq!(hidden_objects(shape, &[]), None);
    assert_eq!(hidden_objects(shape, &[b"tail".to_vec()]), None);
}

#[test]
fn a_shape_whose_parents_loop_ends_the_walk() {
    let mut model = figure();
    let shape = &mut model.shape;
    let arm = shape.find_name(b"arm_r").unwrap();
    let head = shape.find_name(b"head").unwrap();
    let (a, h) = (
        shape.nodes.iter().position(|n| n.name == arm).unwrap(),
        shape.nodes.iter().position(|n| n.name == head).unwrap(),
    );
    // The head's parent is itself: never under the arm, and the walk ends.
    shape.nodes[h].parent = h as i32;
    let hidden = hidden_objects(shape, &[b"arm_r".to_vec()]).unwrap();
    let on = |node: usize| {
        shape
            .objects
            .iter()
            .zip(&hidden)
            .filter(move |(o, _)| o.node as usize == node)
    };
    assert!(on(h).all(|(_, hidden)| *hidden));
    assert!(on(a).all(|(_, hidden)| !*hidden));
}

#[test]
fn the_first_person_view_draws_only_the_named_arms_and_the_third_person_view_the_whole_body() {
    let (mut w, files) = world();
    let all = set(&[
        "torso",
        "head",
        "upperarm_r",
        "hand_r",
        "upperarm_l",
        "leg_l",
    ]);
    let arms = set(&["upperarm_r", "hand_r", "upperarm_l"]);
    let first_person = |d: &Draw| d.label.contains(&label(""));

    // Third person: the whole body, none of it a first-person build.
    let out = draws(&w, &files);
    let drawn: Vec<&Draw> = out.opaque.iter().collect();
    assert_eq!(parts(&drawn), all);
    assert!(!drawn.iter().any(|d| first_person(d)));
    // The nodes named change nothing in third person.
    w.first_person_nodes = vec![b"arm_l".to_vec(), b"arm_r".to_vec()];
    assert_eq!(
        parts(&draws(&w, &files).opaque.iter().collect::<Vec<_>>()),
        all
    );

    // First person with none named (TGE's): nothing of the body.
    w.hide_control = true;
    w.first_person_nodes.clear();
    let out = draws(&w, &files);
    assert!(out.opaque.is_empty() && out.translucent.is_empty());

    // First person with the arms named: the arms and the hand under the elbow, not the head, torso or leg.
    w.first_person_nodes = vec![b"arm_l".to_vec(), b"arm_r".to_vec()];
    let out = draws(&w, &files);
    let drawn: Vec<&Draw> = out.opaque.iter().collect();
    assert_eq!(parts(&drawn), arms);
    assert!(drawn.iter().all(|d| first_person(d)));

    // The enhanced renderer's: the whole body casting its shadow only, and the arms drawn as well.
    let out = with_shadow_body(true, || draws(&w, &files));
    let (shadow, seen): (Vec<&Draw>, Vec<&Draw>) =
        out.opaque.iter().partition(|d| is_shadow_body(&d.label));
    assert_eq!(parts(&shadow), all);
    assert_eq!(parts(&seen), arms);
    assert!(seen.iter().all(|d| first_person(d)));
    // They cast no shadow of their own (the shadow-only body casts theirs): the enhanced renderer draws them view-only.
    assert!(
        seen.iter()
            .all(|d| is_view_model(&d.label) && !is_shadow_body(&d.label))
    );
}

/// The parts are lowered just enough that nothing in the middle of a level view comes above the angle; what is lower, behind
/// the eye or out to the side stays; a part half to the side is lowered part of the way.
#[test]
fn the_parts_are_lowered_out_of_the_middle_of_a_level_view() {
    let clear = 0.12_f32;
    let t = clear.tan();
    // Looking along +y (yaw 0) from the origin.
    let drop = |p: [f32; 3]| clear_drop(&[p], [0.0; 3], 0.0, clear);
    // A hand a meter ahead at eye height: lowered to the angle's line.
    assert!((drop([0.0, 1.0, 0.0]) - t).abs() < 1e-5);
    // Already under it, behind the eye, or far out to the side: none.
    assert_eq!(drop([0.0, 1.0, -0.5]), 0.0);
    assert_eq!(drop([0.0, -1.0, 0.0]), 0.0);
    assert_eq!(drop([0.5, 1.0, 0.0]), 0.0);
    // Halfway through the edge's fade: half the way.
    assert!((drop([1.5 * t, 1.0, 0.0]) - 0.5 * t).abs() < 1e-5);
    // Looking along +x (yaw a quarter turn): the same hand ahead on x; the most any point needs.
    let along_x = clear_drop(
        &[[1.0, 0.0, 0.0], [2.0, 0.0, 0.1], [1.0, 0.0, -1.0]],
        [0.0; 3],
        std::f32::consts::FRAC_PI_2,
        clear,
    );
    assert!((along_x - (0.1 + 2.0 * t)).abs() < 1e-5);
    // Off: no angle, a nonsense angle, or nothing to keep out.
    for c in [0.0, -0.1, f32::NAN, 2.0] {
        assert_eq!(clear_drop(&[[0.0, 1.0, 0.0]], [0.0; 3], 0.0, c), 0.0);
    }
    assert_eq!(clear_drop(&[], [0.0; 3], 0.0, clear), 0.0);
}

/// In first person the arms raised before a level eye are drawn lower by the world's angle (the rest of the body, cast as a
/// shadow, stays); with no angle they stay where the body has them.
#[test]
fn the_first_person_arms_are_drawn_out_of_the_middle_of_the_view() {
    let (mut w, files) = world();
    w.hide_control = true;
    w.first_person_nodes = vec![b"arm_r".to_vec()];
    // An eye looking along +y straight down the arm's box (x 0.3 to 0.4, z 1.2 to 1.6), its top at the eye's height.
    w.view = Some(ghost_world::world::Eye {
        position: [0.35, -0.5, 1.6],
        pitch: 0.0,
        yaw: 0.0,
        fov: None,
    });
    let top = |out: &ShapeDraws| {
        out.opaque
            .iter()
            .flat_map(|d| d.vertices.iter().map(|v| v.position[2]))
            .fold(f32::MIN, f32::max)
    };
    let at_rest = top(&draws(&w, &files));
    assert!((at_rest - 1.6).abs() < 1e-4, "{at_rest}");
    w.first_person_clear = 0.12;
    let lowered = top(&draws(&w, &files));
    // The arm's top comes down to the angle's line at its far end, 0.55 ahead.
    let want = 1.6 - 0.12_f32.tan() * 0.55;
    assert!((lowered - want).abs() < 1e-3, "{lowered}, not {want}");
    let out = with_shadow_body(true, || draws(&w, &files));
    let shadow: Vec<&Draw> = out
        .opaque
        .iter()
        .filter(|d| is_shadow_body(&d.label))
        .collect();
    let shadow_top = shadow
        .iter()
        .flat_map(|d| d.vertices.iter().map(|v| v.position[2]))
        .fold(f32::MIN, f32::max);
    assert!(
        (shadow_top - 2.0).abs() < 1e-4,
        "the shadow body stays: {shadow_top}"
    );
}
