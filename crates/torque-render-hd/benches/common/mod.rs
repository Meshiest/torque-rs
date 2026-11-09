//! The scene the enhanced renderer's CPU benches draw: a floor, 64 figures of 4,800 vertices and 10,000 particles.

use torque_render_hd::ext::PlainScene;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::{DynamicObject, HdDraw, material};
use torque_render_scene::{Blend, Cull};

pub const SIZE: (u32, u32) = (640, 360);
pub const FIGURES: u32 = 64;

/// A figure of ten draws of twenty boxes each (4,800 vertices), about `centre`.
pub fn figure(ghost: u32, centre: [f32; 3]) -> DynamicObject {
    let mut o = box_object(ghost, centre, [0.5, 0.5, 1.0]);
    let template = o.draws.remove(0);
    for d in 0..10 {
        let mut draw = HdDraw {
            label: format!("figure part{d}"),
            vertices: Vec::new(),
            indices: Vec::new(),
            ..template.clone()
        };
        for k in 0..20 {
            let base = draw.vertices.len() as u32;
            let at = [
                centre[0] + k as f32 * 0.05 - 0.5,
                centre[1],
                centre[2] + d as f32 * 0.2,
            ];
            let b = box_object(ghost, at, [0.02, 0.1, 0.09]);
            draw.vertices.extend_from_slice(&b.draws[0].vertices);
            draw.indices
                .extend(b.draws[0].indices.iter().map(|i| base + i));
        }
        o.draws.push(draw);
    }
    o
}

/// 10,000 camera-facing particle squares in a cloud above the floor.
pub fn particles() -> HdDraw {
    let mut draw = box_object(0, [0.0; 3], [1.0; 3]).draws.remove(0);
    let template = draw.vertices[0];
    draw.label = "burst".into();
    draw.blend = Blend::Alpha;
    draw.cull = Cull::None;
    draw.depth_write = false;
    draw.vertices.clear();
    draw.indices.clear();
    for p in 0..10_000u32 {
        let c = [
            (p % 100) as f32 * 0.2 - 10.0,
            (p / 100) as f32 * 0.2 - 10.0,
            2.0 + (p % 7) as f32 * 0.3,
        ];
        let base = draw.vertices.len() as u32;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let mut v = template;
            v.position = [c[0] + a * 0.05, c[1], c[2] + b * 0.05];
            v.material = (material::KIND_SKY << material::KIND_SHIFT) | material::PARTICLE;
            draw.vertices.push(v);
        }
        draw.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    draw
}

/// The bench scene: a floor, [`FIGURES`] figures in a grid in front of the eye (with `stamp`, each its own stamp from it), and,
/// with `burst`, the particles.
pub fn scene(stamp: Option<u64>, burst: bool) -> PlainScene {
    let mut s = PlainScene::default();
    s.frame.sun.towards = [-0.3, -0.2, 0.93];
    s.frame.sun.diffuse = [0.9; 3];
    s.frame.sun.ambient = [0.4; 3];
    let mut floor = box_object(0, [0.0, 0.0, -0.5], [40.0, 40.0, 0.5])
        .draws
        .remove(0);
    floor.label = "floor".into();
    for v in &mut floor.vertices {
        v.material = material::KIND_PLANE << material::KIND_SHIFT;
    }
    s.frame.planes = vec![floor];
    s.frame.planes_generation += 1;
    s.frame.objects = (0..FIGURES)
        .map(|i| {
            let mut o = figure(
                i + 1,
                [(i % 8) as f32 * 2.0 - 7.0, (i / 8) as f32 * 2.0, 0.0],
            );
            o.stamp = stamp.map(|s| s + u64::from(i));
            o
        })
        .collect();
    if burst {
        s.frame.particles = vec![(10.0, particles())];
        s.frame.particles_generation += 1;
    }
    s.frame.eye = Some(look_at([0.0, -12.0, 5.0], [0.0, 4.0, 0.0]));
    s.frame.fov = 70;
    s.frame.size = SIZE;
    s
}
