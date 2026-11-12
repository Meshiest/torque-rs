//! A dissolving shape (`HdScene::dissolves`): the share of its pixels drawn tracks the share of its surface left, the depth
//! prepass's coverage of it is the opaque pass's pixel for pixel (no ghost of the parts gone in the motion or the normals), its
//! shadow shrinks with both of its faces between the sun and the floor and is gone with it, its edge glows past the frame's light, and a dissolve that eats nothing draws
//! the frame a scene without one draws, byte for byte.

use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::dissolve::{Look, TextureMeans, sample_surface};
use torque_render_scene::hd::dissolve::HdDissolve;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::{DynamicObject, HdCamera, HdDraw, material};
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (320, 240);
const FOV: f32 = 50.0;
const EYE: [f32; 3] = [6.0, -7.0, 3.5];
const AT: [f32; 3] = [0.0, 0.0, 1.0];

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn floor() -> HdDraw {
    let f = 40.0;
    let v = |p: [f32; 3]| Vertex {
        position: p,
        normal: [0.0, 0.0, 1.0],
        color: [150, 150, 150, 255],
        uv: [0.0; 2],
    };
    let [a, b, c, d] = [[-f, -f, 0.0], [f, -f, 0.0], [f, f, 0.0], [-f, f, 0.0]].map(v);
    let draw = Draw {
        label: "floor".to_string(),
        vertices: vec![a, b, c, a, c, d],
        lighting: true,
        tex_env: TexEnv::None,
        texture: None,
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: true,
        depth_write: true,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    };
    HdDraw::from_fixed(&draw, material::KIND_PLANE, &[])
}

/// A red box standing on the floor, its back faces culled (through a hole the floor shows, not the far side's inside).
fn statue() -> DynamicObject {
    let mut o = box_object(1, [0.0, 0.0, 1.0], [0.7, 0.7, 1.0]);
    for d in &mut o.draws {
        for v in &mut d.vertices {
            v.color = [220, 30, 30, 255];
        }
        // Each triangle wound counter-clockwise seen from outside.
        for t in d.indices.as_chunks_mut::<3>().0 {
            let p = |i: u32| glam::Vec3::from(d.vertices[i as usize].position);
            let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            if n.dot(p(t[0]) - glam::Vec3::new(0.0, 0.0, 1.0)) < 0.0 {
                t.swap(1, 2);
            }
        }
        d.cull = Cull::BackCcw;
    }
    o
}

fn scene(dissolve: Option<HdDissolve>) -> PlainScene {
    let mut s = PlainScene::default();
    let t = [-0.5f32, 0.3, 0.8];
    let n = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
    s.sun.towards = t.map(|v| v / n);
    s.sun.diffuse = [1.0; 3];
    s.sun.ambient = [0.25; 3];
    s.sun.shadow = [0.1; 3];
    s.ao_radius = 0.0;
    s.planes = vec![floor()];
    s.planes_generation += 1;
    s.objects = vec![statue()];
    s.dissolves = dissolve.into_iter().collect();
    let view = look_at(EYE, AT);
    s.eye = Some(view);
    s.fov = FOV as u8;
    s.size = SIZE;
    s.generation += 1;
    s
}

/// The dissolve of `eaten` of the statue's surface (its edge `edge` of the surface wide, of light `glow`).
fn dissolve(eaten: f32, edge: f32, glow: f32) -> HdDissolve {
    let look = Look {
        density: 2000.0,
        ..Look::default()
    };
    let surface = sample_surface(&statue(), &[], &mut TextureMeans::default(), &look);
    let threshold = surface.quantile(eaten);
    HdDissolve {
        ghost: 1,
        threshold,
        edge_to: surface.quantile(eaten + edge).max(threshold),
        edge_color: [glow, glow * 0.4, 0.0],
        cell: look.cell,
        sweep: 0.0,
        z_range: surface.z_range,
        eaten,
    }
}

/// A frame of `s`: its HDR target before the anti-aliasing, its prepass motion, and its shown bytes.
struct Shot {
    hdr: Vec<[f32; 4]>,
    motion: Vec<[f32; 4]>,
    ldr: Vec<u8>,
}

fn draw(gpu: &Gpu, s: &PlainScene) -> Shot {
    let camera = HdCamera::new(
        s.eye.expect("an eye"),
        FOV,
        s.visible_distance,
        SIZE.0,
        SIZE.1,
    );
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.upload(s);
    let format = target.color.texture().format();
    for _ in 0..3 {
        world
            .draw(s, &camera, Some((&target.color, format, [0, 0])))
            .expect("a frame");
    }
    let (_, _, hdr) = world.read_hdr().expect("the HDR target");
    let (_, _, motion) = world.read_motion().expect("the motion");
    let ldr = gpu.read_back(&target).expect("read back").rgba;
    Shot { hdr, motion, ldr }
}

/// Whether an HDR pixel is the red statue's.
fn red(c: &[f32; 4]) -> bool {
    c[0] > 0.02 && c[0] > 3.0 * c[1] && c[0] > 3.0 * c[2]
}

/// The pixels of the floor in the statue's shadow: dark ones (the floor lit is far brighter) below the statue's foot.
fn shadowed(shot: &Shot) -> usize {
    let floor_lit = shot
        .hdr
        .iter()
        .zip(&shot.motion)
        .filter(|(_, m)| (m[3] - 1.0).abs() < 0.01)
        .map(|(c, _)| c[1])
        .fold(0.0f32, f32::max);
    shot.hdr
        .iter()
        .zip(&shot.motion)
        .filter(|(c, m)| (m[3] - 1.0).abs() < 0.01 && c[1] < 0.5 * floor_lit)
        .count()
}

#[test]
fn a_dissolving_shape_draws_the_share_left_and_its_depth_shadow_and_edge_agree() {
    let Some(gpu) = gpu() else { return };
    let whole = draw(&gpu, &scene(None));
    let total = whole.hdr.iter().filter(|c| red(c)).count();
    assert!(total > 3000, "the statue fills {total} pixels");
    let shadow = shadowed(&whole);
    assert!(shadow > 1000, "its shadow fills {shadow} pixels");
    // Nothing eaten: the frame of a scene without the dissolve, byte for byte.
    let none = draw(&gpu, &scene(Some(dissolve(0.0, 0.0, 0.0))));
    assert!(
        none.ldr == whole.ldr,
        "a dissolve that eats nothing changed the frame"
    );
    let mut seen = Vec::new();
    for eaten in [0.25f32, 0.5, 0.75] {
        let shot = draw(&gpu, &scene(Some(dissolve(eaten, 0.0, 0.0))));
        let drawn = shot.hdr.iter().filter(|c| red(c)).count() as f32 / total as f32;
        // The prepass's coverage: its object pixels (w is 1 plus the id: over 1 for a shape) against the colour's.
        let object = |m: &[f32; 4]| m[3] > 1.5;
        let depth = shot.motion.iter().filter(|m| object(m)).count();
        let disagree = shot
            .hdr
            .iter()
            .zip(&shot.motion)
            .filter(|(c, m)| red(c) != object(m))
            .count();
        let shade = shadowed(&shot) as f32 / shadow as f32;
        seen.push((eaten, drawn, depth, disagree, shade));
        assert!(
            (drawn - (1.0 - eaten)).abs() < 0.08,
            "{eaten} eaten draws {drawn} of the statue ({seen:?})"
        );
        assert!(
            (disagree as f32) < 0.01 * total as f32,
            "{eaten} eaten: {disagree} pixels where the prepass and the colour disagree ({seen:?})"
        );
        // A point of the floor is in shadow while either of the box's two faces over it holds there.
        assert!(
            (shade - (1.0 - eaten * eaten)).abs() < 0.15,
            "{eaten} eaten: the shadow keeps {shade} ({seen:?})"
        );
    }
    let gone = draw(&gpu, &scene(Some(dissolve(1.0, 0.0, 0.0))));
    assert_eq!(gone.hdr.iter().filter(|c| red(c)).count(), 0);
    assert_eq!(gone.motion.iter().filter(|m| m[3] > 1.5).count(), 0);
    assert!(
        shadowed(&gone) < shadow / 50,
        "a statue gone leaves its shadow"
    );
    // The edge glows: its pixels past any lit surface's light.
    let lit_max = whole
        .hdr
        .iter()
        .map(|c| c[0].max(c[1]).max(c[2]))
        .fold(0.0f32, f32::max);
    let edge = draw(&gpu, &scene(Some(dissolve(0.5, 0.06, 8.0))));
    let glowing = edge
        .hdr
        .iter()
        .filter(|c| c[0] > 2.0 * lit_max.max(1.0))
        .count();
    assert!(
        glowing > total / 50,
        "{glowing} pixels of the edge glow (the lit frame's brightest {lit_max})"
    );
}
