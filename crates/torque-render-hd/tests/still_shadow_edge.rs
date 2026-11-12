//! A still shadow's hard edge keeps its history: a thin post and a box on a floor under a sharp sun (no penumbra to speak of,
//! the contact shadows at the box's foot), the camera held. The TAA reads the sun's mask between the jittered pixels, and a
//! hard edge's mask moves a little with the jitter; if that read as a passing edge, the edge's pixels would start their
//! history again every frame and show each frame's jittered sample: the edge would shimmer. Once settled, consecutive frames
//! stay alike.
//!
//! And frames with no mask (the sun's light gone: a night) after frames whose mask moved: the TAA compares no masks left
//! over from the sunlit frames, so a checkered floor under them shimmers no more than in a world that never drew a mask.

use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdMode, Quality, bounding_sphere, material,
};
use torque_render_scene::math::Mat;
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (320, 240);
/// The frames drawn before the image counts as settled, and those compared after.
const SETTLE: usize = 40;
const COMPARED: usize = 24;
/// A pixel shimmers where its luminance moves by more than this (of 255) from one frame to the next.
const STEP: f32 = 12.0;
const PLAIN: [u8; 4] = [200, 180, 160, 255];

type Quad = ([[f32; 3]; 4], [f32; 3], [u8; 4]);

fn coloured(label: &str, quads: &[Quad]) -> Draw {
    let mut vertices = Vec::new();
    for &(corners, normal, color) in quads {
        let v = |p: [f32; 3]| Vertex {
            position: p,
            normal,
            color,
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

/// The four sides and the top of a box from `lo` to `hi`.
fn cuboid(label: &str, lo: [f32; 3], hi: [f32; 3]) -> Draw {
    let ([x0, y0, z0], [x1, y1, z1]) = (lo, hi);
    coloured(
        label,
        &[
            (
                [[x0, y0, z0], [x0, y1, z0], [x0, y1, z1], [x0, y0, z1]],
                [-1.0, 0.0, 0.0],
                PLAIN,
            ),
            (
                [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
                [1.0, 0.0, 0.0],
                PLAIN,
            ),
            (
                [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
                [0.0, -1.0, 0.0],
                PLAIN,
            ),
            (
                [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
                [0.0, 1.0, 0.0],
                PLAIN,
            ),
            (
                [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
                [0.0, 0.0, 1.0],
                PLAIN,
            ),
        ],
    )
}

const UP: [f32; 3] = [0.0, 0.0, 1.0];
const FLOOR: [[f32; 3]; 4] = [
    [-30.0, -30.0, 0.0],
    [30.0, -30.0, 0.0],
    [30.0, 30.0, 0.0],
    [-30.0, 30.0, 0.0],
];

/// The floor checkered in 0.2-unit squares within 4 units of the middle (a hard-edged pattern the history smooths).
fn checkered_floor() -> Draw {
    let below = FLOOR.map(|[x, y, _]| [x, y, -0.001]);
    let mut quads = vec![(below, UP, PLAIN)];
    let cell = 0.2;
    for i in -20..20 {
        for k in -20..20 {
            let (x, y) = (i as f32 * cell, k as f32 * cell);
            let color = if (i + k) % 2 == 0 {
                [200, 190, 175, 255]
            } else {
                [150, 140, 128, 255]
            };
            let corners = [
                [x, y, 0.0],
                [x + cell, y, 0.0],
                [x + cell, y + cell, 0.0],
                [x, y + cell, 0.0],
            ];
            quads.push((corners, UP, color));
        }
    }
    coloured("checkered floor", &quads)
}

/// `draw` moved by `at`, by `from` last frame (its draws in the world, the transforms for the motion).
fn object(ghost: u32, mut draw: Draw, at: [f32; 3], from: [f32; 3]) -> DynamicObject {
    for v in &mut draw.vertices {
        for (p, d) in v.position.iter_mut().zip(at) {
            *p += d;
        }
    }
    let draws = vec![HdDraw::from_fixed(&draw, material::KIND_SHAPE, &[])];
    let (centre, radius) = bounding_sphere(&draws);
    let mut transform = Mat::IDENTITY;
    transform.set_column3(at);
    let mut prev_transform = Mat::IDENTITY;
    prev_transform.set_column3(from);
    DynamicObject {
        ghost,
        transform,
        prev_transform,
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

fn the_box() -> Draw {
    cuboid("box", [1.0, 0.5, 0.0], [2.0, 1.5, 0.8])
}

fn scene() -> (PlainScene, HdCamera) {
    let mut s = PlainScene {
        mode: Some(HdMode::new(Quality::High)),
        ..PlainScene::default()
    };
    s.sun.towards = [-0.5, -0.45, 0.74];
    s.sun.diffuse = [0.95, 0.92, 0.85];
    s.sun.ambient = [0.3, 0.32, 0.38];
    s.sun.shadow = [0.2, 0.22, 0.27];
    s.sun_radius = 0.0;
    s.planes = vec![HdDraw::from_fixed(
        &coloured("floor", &[(FLOOR, UP, PLAIN)]),
        material::KIND_PLANE,
        &[],
    )];
    let post = cuboid("post", [-0.04, -0.04, 0.0], [0.04, 0.04, 2.5]);
    s.objects = vec![
        object(1, post, [0.0; 3], [0.0; 3]),
        object(2, the_box(), [0.0; 3], [0.0; 3]),
    ];
    let eye = look_at([-1.5, -6.0, 3.0], [0.8, 1.5, 0.0]);
    s.eye = Some(eye);
    s.fov = 70;
    s.size = SIZE;
    s.generation += 1;
    let camera = HdCamera::new(eye, 70u8, s.visible_distance, SIZE.0, SIZE.1);
    (s, camera)
}

fn luma(rgba: &[u8]) -> Vec<f32> {
    let (pixels, _) = rgba.as_chunks::<4>();
    pixels
        .iter()
        .map(|&[r, g, b, _]| 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b))
        .collect()
}

/// A world's frames of `s` into an offscreen target.
struct Drawer {
    gpu: Gpu,
    world: HdWorld<NoExtension>,
    target: torque_render_core::Offscreen,
}

impl Drawer {
    fn new() -> Option<Drawer> {
        let gpu = Gpu::new()
            .inspect_err(|e| eprintln!("skipped: no GPU ({e})"))
            .ok()?;
        let world = HdWorld::new(&gpu);
        let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
        Some(Drawer { gpu, world, target })
    }

    fn draw(&mut self, s: &PlainScene, camera: &HdCamera) {
        let format = self.target.color.texture().format();
        self.world.upload(s);
        self.world
            .draw(s, camera, Some((&self.target.color, format, [0, 0])))
            .expect("a frame");
    }

    /// The pixels whose luminance moves by more than STEP between some two consecutive frames of `s` held still, once settled.
    fn shimmering(&mut self, s: &PlainScene, camera: &HdCamera) -> usize {
        let mut frames = Vec::with_capacity(COMPARED + 1);
        for f in 0..SETTLE + COMPARED + 1 {
            self.draw(s, camera);
            if f >= SETTLE {
                let frame = self.gpu.read_back(&self.target).expect("read back");
                frames.push(luma(&frame.rgba));
            }
        }
        let mut shimmered = vec![false; frames[0].len()];
        for pair in frames.windows(2) {
            for (s, (a, b)) in shimmered.iter_mut().zip(pair[0].iter().zip(&pair[1])) {
                *s |= (a - b).abs() > STEP;
            }
        }
        shimmered.iter().filter(|&&s| s).count()
    }
}

#[test]
fn a_still_hard_shadow_edge_keeps_its_history() {
    let Some(mut d) = Drawer::new() else { return };
    let (s, camera) = scene();
    let count = d.shimmering(&s, &camera);
    eprintln!("pixels shimmering once settled: {count}");
    assert!(
        count <= 4,
        "{count} pixels of the still shadow's edges moved by over {STEP}/255 between frames"
    );
}

#[test]
fn frames_with_no_mask_compare_none_left_from_sunlit_frames() {
    let Some(mut d) = Drawer::new() else { return };
    let (mut s, camera) = scene();
    s.planes = vec![HdDraw::from_fixed(
        &checkered_floor(),
        material::KIND_PLANE,
        &[],
    )];
    // The sun behind the box: its shadow falls toward the camera.
    s.sun.towards = [0.45, 0.4, 0.8];
    // The box sliding fast toward the camera under the sun: the last two masks differ along its shadow's edges.
    let step = 0.4;
    let mut y = 0.0;
    for _ in 0..8 {
        s.objects = vec![object(2, the_box(), [0.0, y, 0.0], [0.0, y + step, 0.0])];
        s.generation += 1;
        d.draw(&s, &camera);
        y -= step;
    }
    // Then held with no sun's light (no mask is drawn; the ambient as bright), and the same in a world that never drew one.
    y += step;
    s.objects = vec![object(2, the_box(), [0.0, y, 0.0], [0.0, y, 0.0])];
    s.sun.diffuse = [0.0; 3];
    s.sun.ambient = [1.0; 3];
    s.generation += 1;
    let after_sun = d.shimmering(&s, &camera);
    let Some(mut fresh) = Drawer::new() else {
        return;
    };
    let never = fresh.shimmering(&s, &camera);
    eprintln!(
        "pixels shimmering once settled, no mask: {after_sun} after sunlit frames, {never} in a world without"
    );
    assert!(
        after_sun <= never + 4,
        "{after_sun} pixels shimmered after the sunlit frames, {never} in a world that never drew a mask"
    );
}
