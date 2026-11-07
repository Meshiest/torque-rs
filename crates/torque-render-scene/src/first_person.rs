//! The parts of the client's own body its first-person view still draws (`ClientWorld::first_person_nodes`): the objects on the
//! named nodes and on every node under them, posed by the body's own animation, drawn as the body is. Everything else of it
//! (the head the eye sits in, the torso under the view) stays hidden, as TGE hides the whole body there.

use shapes::instance::ShapeInstance;
use shapes::shape::Shape;

use crate::math::Mat;

/// What a first-person part build's label starts with (its draws' labels: `shape <label> object <n>`): a build of its own beside
/// the body's (a shadow-only copy of the whole body in the enhanced renderer).
const FIRST_PERSON_LABEL: &str = "first-person ";

/// A first-person part build's label, from the body's.
pub fn label(body: &str) -> String {
    format!("{FIRST_PERSON_LABEL}{body}")
}

/// Per object of `shape`: whether the first-person view leaves it out (its node is none of `nodes`, by name, and under none of
/// them). None when `nodes` names none of the shape's nodes: nothing of the body is drawn.
pub fn hidden_objects(shape: &Shape, nodes: &[Vec<u8>]) -> Option<Vec<bool>> {
    if nodes.is_empty() {
        return None;
    }
    let named = |n: &shapes::shape::Node| {
        usize::try_from(n.name)
            .ok()
            .and_then(|i| shape.names.get(i))
            .is_some_and(|name| nodes.iter().any(|w| w.eq_ignore_ascii_case(name)))
    };
    if !shape.nodes.iter().any(named) {
        return None;
    }
    // A node is shown when it or a node over it is named (at most as many steps up as there are nodes: a shape's parents are
    // not trusted to end).
    let shown = |node: i32| {
        let mut at = node;
        for _ in 0..=shape.nodes.len() {
            let Some(n) = usize::try_from(at).ok().and_then(|i| shape.nodes.get(i)) else {
                return false;
            };
            if named(n) {
                return true;
            }
            at = n.parent;
        }
        false
    };
    Some(shape.objects.iter().map(|o| !shown(o.node)).collect())
}

/// The vertices of the objects of `shape`'s sub-shape `sub_shape` not `hidden`, at object detail `object_detail`, placed by
/// `instance`'s nodes and then by `xform` (in the world).
pub fn part_points(
    shape: &Shape,
    instance: &ShapeInstance,
    hidden: &[bool],
    (sub_shape, object_detail): (i32, i32),
    xform: &Mat,
) -> Vec<[f32; 3]> {
    let Some(ss) = usize::try_from(sub_shape)
        .ok()
        .and_then(|i| shape.sub_shapes.get(i))
    else {
        return Vec::new();
    };
    let first = usize::try_from(ss.first_object).unwrap_or(0);
    let count = usize::try_from(ss.num_objects).unwrap_or(0);
    let nodes = instance.node_transforms();
    let mut out = Vec::new();
    let objects = shape.objects.iter().zip(hidden).skip(first).take(count);
    for (o, _) in objects.filter(|(_, h)| !**h) {
        if object_detail < 0 || object_detail >= o.num_meshes {
            continue;
        }
        let mesh = usize::try_from(o.start_mesh_index + object_detail)
            .ok()
            .and_then(|i| shape.meshes.get(i))
            .and_then(Option::as_ref);
        let node = usize::try_from(o.node).ok().and_then(|i| nodes.get(i));
        let (Some(mesh), Some(node)) = (mesh, node) else {
            continue;
        };
        out.extend(
            mesh.positions
                .iter()
                .map(|p| xform.mul_p(shapes::math::mul_p(node, *p))),
        );
    }
    out
}

/// How far the first-person parts at `points` (in the world) are lowered so that none in the middle of a level view from
/// `eye` looking along `yaw` comes above `clear` radians under its line of sight: 0 when none does. A point counts fully
/// within as far either side of the line of sight as `clear` reaches under it, less and less over as far again beyond (no
/// sudden drop as a hand comes in from the side), not at all behind the eye or further out.
pub fn clear_drop(points: &[[f32; 3]], eye: [f32; 3], yaw: f32, clear: f32) -> f32 {
    if !(clear > 0.0 && clear < std::f32::consts::FRAC_PI_2) {
        return 0.0;
    }
    let t = clear.tan();
    let (sy, cy) = yaw.sin_cos();
    points
        .iter()
        .map(|p| {
            let d = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
            let ahead = d[0] * sy + d[1] * cy;
            if ahead <= 1e-4 {
                return 0.0;
            }
            let side = (d[0] * cy - d[1] * sy).abs() / ahead;
            let s = ((side - t) / t).clamp(0.0, 1.0);
            let weight = 1.0 - s * s * (3.0 - 2.0 * s);
            weight * (d[2] + t * ahead)
        })
        .filter(|d| d.is_finite())
        .fold(0.0, f32::max)
}

#[cfg(test)]
#[path = "first_person_tests.rs"]
mod tests;
