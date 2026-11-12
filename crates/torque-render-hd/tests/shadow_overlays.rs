//! A shadow map with moving shapes in it is kept while nothing it shows changed (`torque_render_hd::shadows`): thirty frames of a
//! walking figure (its vertices in its mesh's own space, placed on the GPU) beside a sliding box (its vertices in the world), a
//! point light and a spot moving then still, the eye still then moving, the client's own Player coming and going (its
//! first-person image lit by the cascades kept without it), and a pass from outside drawing into the cascades for a few frames,
//! drawn twice: with the working maps kept, and with every map copied and drawn over each frame. Every frame must be the same
//! picture, pixel for pixel, and keeping must have drawn fewer overlays.

use torque_render_core::wgpu;
use torque_render_core::{Frame, Gpu};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::passes::{PassCx, Stage};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdLight, HdSpot, NodePlace, bounding_sphere, material,
};
use torque_render_scene::math::Mat;
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (320, 240);
const FOV: f32 = 60.0;
const FRAMES: usize = 30;

fn quads(label: &str, quads: &[([[f32; 3]; 4], [f32; 3])]) -> Draw {
    let mut vertices = Vec::new();
    for (corners, normal) in quads {
        let v = |p: [f32; 3]| Vertex {
            position: p,
            normal: *normal,
            color: [180, 180, 180, 255],
            uv: [0.0; 2],
        };
        let [a, b, c, d] = corners.map(v);
        vertices.extend([a, b, c, a, c, d]);
    }
    Draw {
        label: label.to_string(),
        vertices,
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
    }
}

/// A box from `l` to `h`.
fn box_draw(l: [f32; 3], h: [f32; 3]) -> Draw {
    let c = |x: usize, y: usize, z: usize| [[l[0], h[0]][x], [l[1], h[1]][y], [l[2], h[2]][z]];
    quads(
        "box",
        &[
            (
                [c(0, 0, 0), c(0, 1, 0), c(0, 1, 1), c(0, 0, 1)],
                [-1.0, 0.0, 0.0],
            ),
            (
                [c(1, 0, 0), c(1, 1, 0), c(1, 1, 1), c(1, 0, 1)],
                [1.0, 0.0, 0.0],
            ),
            (
                [c(0, 0, 0), c(1, 0, 0), c(1, 0, 1), c(0, 0, 1)],
                [0.0, -1.0, 0.0],
            ),
            (
                [c(0, 1, 0), c(1, 1, 0), c(1, 1, 1), c(0, 1, 1)],
                [0.0, 1.0, 0.0],
            ),
            (
                [c(0, 0, 1), c(1, 0, 1), c(1, 1, 1), c(0, 1, 1)],
                [0.0, 0.0, 1.0],
            ),
            (
                [c(0, 0, 0), c(1, 0, 0), c(1, 1, 0), c(0, 1, 0)],
                [0.0, 0.0, -1.0],
            ),
        ],
    )
}

/// A placement `at` (no turn).
fn placed(at: [f32; 3]) -> Mat {
    let mut m = Mat::IDENTITY;
    m.m[3] = at[0];
    m.m[7] = at[1];
    m.m[11] = at[2];
    m
}

/// A box's object whose vertices are in the world (no stamp: written anew each upload). `own`: the client's own Player, its body
/// drawn into the shadow maps only and a first-person image before the eye cast into none (lit by the cascades kept without it).
fn world_box(ghost: u32, l: [f32; 3], h: [f32; 3], own: bool) -> DynamicObject {
    let mut draws = vec![HdDraw::from_fixed(
        &box_draw(l, h),
        material::KIND_SHAPE,
        &[],
    )];
    if own {
        for v in &mut draws[0].vertices {
            v.material |= material::SHADOW_ONLY;
        }
        let mut image = HdDraw::from_fixed(
            &box_draw(
                [l[0] + 0.2, l[1] - 0.6, h[2] - 0.6],
                [l[0] + 0.5, l[1] - 0.3, h[2] - 0.3],
            ),
            material::KIND_SHAPE,
            &[],
        );
        for v in &mut image.vertices {
            v.material |= material::VIEW_ONLY;
        }
        draws.push(image);
    }
    let (centre, radius) = bounding_sphere(&draws);
    DynamicObject {
        ghost,
        transform: Mat::IDENTITY,
        prev_transform: Mat::IDENTITY,
        translucent: false,
        centre,
        radius,
        draws,
        previous: None,
        stamp: None,
        nodes: Vec::new(),
        takes_decals: false,
    }
}

/// The figure: a box in its mesh's own space (`material::NODE`, place 0), placed `at` now and `before` the frame before, its
/// draws stamped (kept where the renderer wrote them; only its place moves).
fn figure(at: [f32; 3], before: [f32; 3]) -> DynamicObject {
    let mut draw = HdDraw::from_fixed(
        &box_draw([-0.3, -0.3, 0.0], [0.3, 0.3, 1.8]),
        material::KIND_SHAPE,
        &[],
    );
    for v in &mut draw.vertices {
        v.material |= material::NODE;
        v.extra = 0;
    }
    let nodes = vec![NodePlace {
        now: placed(at),
        before: placed(before),
        normal: Mat::IDENTITY,
        sphere: [0.0, 0.0, 0.9, 1.0],
        build: 0,
    }];
    let (centre, radius) = nodes[0].world_sphere();
    DynamicObject {
        ghost: 1,
        transform: placed(at),
        prev_transform: placed(before),
        translucent: false,
        centre,
        radius,
        draws: vec![draw],
        previous: None,
        stamp: Some(7),
        nodes,
        takes_decals: false,
    }
}

/// The scene at frame `f`: the figure walks, the box slides and the lights move over frames 0 to 4; over 5 to 12 no shape is
/// there, a pass from outside clearing every cascade over 5 to 9 and gone from 10; over 13 to 16 the figure, the box and the
/// client's own Player stand still; over 17 and 18 nothing is there, the sun has turned and the eye moves; over 19 to 23 the own
/// Player stands alone, the eye still; from 24 the figure and the box are back and the eye moves every frame.
fn scene_at(s: &mut PlainScene, f: usize) -> HdCamera {
    let t = f as f32;
    let moving = t.min(5.0);
    let walk = |k: f32| [-1.0 + 0.2 * k, 1.0, 0.0];
    let slide = moving * 0.08;
    let mut objects = Vec::new();
    if f < 5 || (13..17).contains(&f) || f >= 24 {
        objects.push(figure(walk(moving), walk((moving - 1.0).max(0.0))));
        objects.push(world_box(
            2,
            [1.5 + slide, -1.0, 0.0],
            [2.3 + slide, -0.2, 1.0],
            false,
        ));
    }
    if (13..17).contains(&f) || (19..24).contains(&f) {
        objects.push(world_box(3, [-0.4, -2.6, 0.0], [0.4, -1.8, 1.8], true));
    }
    s.objects = objects;
    s.lights = vec![HdLight {
        position: [2.5 - 0.15 * moving, -2.5 + 0.15 * moving, 3.0],
        color: [5.0, 5.0, 5.0],
        radius: 12.0,
        carrier: None,
    }];
    s.spots = vec![HdSpot {
        light: HdLight {
            position: [-3.0 + 0.15 * moving, 3.0, 4.0],
            color: [6.0, 5.0, 4.0],
            radius: 14.0,
            carrier: None,
        },
        direction: [0.3, -0.3, -0.9],
        inner: 0.5,
        outer: 0.8,
        shadows: true,
    }];
    s.lights_generation += 1;
    // (Toward the sun: up.)
    s.sun.towards = if f < 17 {
        [-0.45, -0.3, 0.84]
    } else {
        [-0.3, -0.5, 0.81]
    };
    let steps = match f {
        0..=16 => 0.0,
        17 => 1.0,
        18..=23 => 2.0,
        _ => 2.0 + 0.2 * (t - 23.0),
    };
    let view = look_at([4.0 + 1.5 * steps, -7.0 + steps, 4.0], [0.0, 0.0, 0.8]);
    s.eye = Some(view);
    s.fov = FOV as u8;
    s.size = SIZE;
    s.generation += 1;
    s.control_generation += 1;
    HdCamera::new(view, FOV, s.visible_distance, SIZE.0, SIZE.1)
}

fn scene() -> PlainScene {
    let mut s = PlainScene::default();
    s.sun.ambient = [0.1, 0.1, 0.1];
    s.sun.shadow = [0.1, 0.1, 0.1];
    s.sun.diffuse = [1.0, 1.0, 1.0];
    s.sun.towards = [-0.45, -0.3, 0.84];
    s.ao_radius = 0.0;
    let floor = quads(
        "floor",
        &[(
            [
                [-30.0, -30.0, 0.0],
                [30.0, -30.0, 0.0],
                [30.0, 30.0, 0.0],
                [-30.0, 30.0, 0.0],
            ],
            [0.0, 0.0, 1.0],
        )],
    );
    s.planes = vec![HdDraw::from_fixed(&floor, material::KIND_PLANE, &[])];
    s.planes_generation += 1;
    s
}

/// A pass from outside at the shadows' stage: every cascade's working map cleared to the nearest depth (everything in shadow).
fn clearing_pass(cx: &mut PassCx<'_>) {
    for c in cx.cascades {
        cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("test shadow clear"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: c.target,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
}

/// The thirty frames drawn with the working maps kept (`keep`) or not: each frame's picture, and the overlays drawn in all.
fn frames(gpu: &Gpu, keep: bool) -> (Vec<Frame>, usize) {
    let mut s = scene();
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
    let format = target.color.texture().format();
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.set_keep_shadow_overlays(keep);
    let mut out = Vec::with_capacity(FRAMES);
    let mut overlays = 0;
    for f in 0..FRAMES {
        let camera = scene_at(&mut s, f);
        if f == 5 {
            world.add_pass(Stage::Shadows, clearing_pass);
        }
        if f == 10 {
            world.passes_mut().clear();
        }
        world.upload(&s);
        world
            .draw(&s, &camera, Some((&target.color, format, [0, 0])))
            .expect("a frame");
        overlays += world.shadow_stats().overlays;
        out.push(gpu.read_back(&target).expect("read back"));
    }
    (out, overlays)
}

/// The pixels of `a` and `b` that differ in any channel.
fn differing(a: &Frame, b: &Frame) -> usize {
    a.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
        .filter(|(p, q)| p != q)
        .count()
}

#[test]
fn kept_shadow_overlays_draw_the_same_frames_as_overlays_drawn_every_frame() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let (kept, kept_overlays) = frames(&gpu, true);
    let (every, every_overlays) = frames(&gpu, false);
    let off: Vec<(usize, usize)> = kept
        .iter()
        .zip(&every)
        .enumerate()
        .map(|(f, (a, b))| (f, differing(a, b)))
        .filter(|(_, n)| *n > 0)
        .collect();
    assert!(
        off.is_empty(),
        "frames whose kept shadow maps differ from maps drawn every frame (frame, pixels off): {off:?}"
    );
    assert!(
        kept_overlays < every_overlays,
        "keeping the maps drew {kept_overlays} overlays, as many as drawing them every frame ({every_overlays})"
    );
}
