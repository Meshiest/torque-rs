//! A debug view of a figure's hitboxes: each part's hull at the server's pose now (what a bolt is cast at), drawn as a box of
//! lines over a picture of the client's view. The box is measured by casting at the hull from outside along each axis of its
//! node, so a side the hull leaves open shows: that part is drawn to its mesh's bounds, in magenta. Else the head is red, the
//! torso yellow, the arms cyan and the legs green, the left side's parts paler.

use torque::app::server::GameServer;
use torque::game::scene::{access, hitboxes, rays};
use torque::script::vm::sim::ObjId;
use torque::shapes::collide::Hull;
use torque::shapes::math::{IDENTITY, mul_p};
use torque::shapes::shape::Mesh;
use torque_render_core::Frame;
use torque_render_scene::camera::Camera;
use torque_render_scene::source::Eye;

/// One part's box: the part's name, the box's eight corners in the world (corner `k` takes each axis's max where bit `axis` of
/// `k` is set), and whether its hull is open on some side.
#[derive(Debug, Clone, PartialEq)]
pub struct PartBox {
    pub name: String,
    pub corners: [[f32; 3]; 8],
    pub open: bool,
}

/// The extent of a part's hull in its node's space: per axis, from outside along it to the middle of the mesh's bounds; the
/// bounds where nothing is hit, and whether that happened.
fn hull_extent(mesh: &Mesh) -> ([f32; 6], bool) {
    let b = mesh.bounds;
    let mut free = |_: u64| true;
    let Ok(hull) = Hull::build(mesh, &mut free) else {
        return (b, true);
    };
    let middle: [f32; 3] = std::array::from_fn(|a| (b[a] + b[a + 3]) / 2.0);
    let (mut out, mut open) = (b, false);
    for a in 0..3 {
        for (side, far) in [(0, -10.0), (3, 10.0)] {
            let mut start = middle;
            start[a] += far;
            match hull.cast(start, middle, &mut free) {
                Ok(Some(hit)) => out[a + side] = start[a] + (middle[a] - start[a]) * hit.t,
                _ => open = true,
            }
        }
    }
    (out, open)
}

/// The hitboxes of the Player `id` on `server` at its pose now; none for a Player without them.
pub fn part_boxes(server: &mut GameServer, id: ObjId) -> Vec<PartBox> {
    let vm = server.vm_mut();
    let Ok(Some(pose)) = hitboxes::pose_now(vm, id) else {
        return Vec::new();
    };
    let (Some(shape), Some(matrix)) = (pose.kept.shape.as_ref(), access::matrix_of(vm, id)) else {
        return Vec::new();
    };
    let scale = access::scale_of(vm, id);
    let mut free = |_: u64| true;
    let mut out = Vec::new();
    for &detail in pose.los.iter() {
        for (oi, mi) in rays::detail_objects(shape, detail, &mut free).unwrap_or_default() {
            let Some(Some(mesh)) = shape.meshes.get(mi) else {
                continue;
            };
            let object = &shape.objects[oi];
            let node = usize::try_from(object.node)
                .ok()
                .and_then(|n| pose.nodes.get(n).copied())
                .unwrap_or(IDENTITY);
            let (b, open) = hull_extent(mesh);
            let corners = std::array::from_fn(|k| {
                let local =
                    std::array::from_fn(|a| if k & (1 << a) != 0 { b[a + 3] } else { b[a] });
                let p = mul_p(&node, local);
                mul_p(&matrix, std::array::from_fn(|a| p[a] * scale[a]))
            });
            let name = usize::try_from(object.name)
                .ok()
                .and_then(|n| shape.names.get(n))
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default();
            out.push(PartBox {
                name,
                corners,
                open,
            });
        }
    }
    out
}

/// The line color of a part.
fn color(name: &str) -> [u8; 3] {
    let (left, part) = match name.strip_prefix('l') {
        Some(rest) if name != "torso" => (true, rest),
        _ => (false, name.strip_prefix('r').unwrap_or(name)),
    };
    let base = match part {
        "head" => [255, 40, 40],
        "torso" => [255, 220, 0],
        "arm" | "forearm" | "hand" => [0, 210, 255],
        _ => [40, 230, 40],
    };
    if left {
        base.map(|c| c / 2 + 128)
    } else {
        base
    }
}

/// Draws `boxes` as lines over `frame`, a picture seen from `eye`.
pub fn draw(frame: &mut Frame, eye: Eye, boxes: &[PartBox]) {
    let (w, h) = (frame.width, frame.height);
    let m = Camera::from_eye(eye, f32::from(eye.fov.unwrap_or(90)), 1000.0, w, h).view_proj();
    let project = |p: [f32; 3]| -> Option<[f32; 2]> {
        let v = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2]), 1.0];
        let clip: [f64; 4] = std::array::from_fn(|r| (0..4).map(|c| m[c * 4 + r] * v[c]).sum());
        (clip[3] > 0.1).then(|| {
            [
                ((clip[0] / clip[3] + 1.0) * 0.5 * f64::from(w)) as f32,
                ((1.0 - clip[1] / clip[3]) * 0.5 * f64::from(h)) as f32,
            ]
        })
    };
    for part in boxes {
        let rgb = if part.open {
            [255, 0, 255]
        } else {
            color(&part.name)
        };
        for i in 0..8usize {
            for axis in 0..3 {
                let j = i | (1 << axis);
                if j == i {
                    continue;
                }
                if let (Some(a), Some(b)) = (project(part.corners[i]), project(part.corners[j])) {
                    line(frame, a, b, rgb);
                }
            }
        }
    }
}

/// A line two pixels wide from `a` to `b`.
fn line(frame: &mut Frame, a: [f32; 2], b: [f32; 2], rgb: [u8; 3]) {
    let steps = (b[0] - a[0])
        .abs()
        .max((b[1] - a[1]).abs())
        .ceil()
        .clamp(1.0, 8192.0) as u32;
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let (x, y) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
        for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
            let (px, py) = ((x + dx) as i64, (y + dy) as i64);
            if px < 0 || py < 0 || px >= i64::from(frame.width) || py >= i64::from(frame.height) {
                continue;
            }
            let at = (py as usize * frame.width as usize + px as usize) * 4;
            frame.rgba[at..at + 3].copy_from_slice(&rgb);
        }
    }
}
