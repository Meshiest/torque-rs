//! The ambient occlusion has no banding: a block held out from a wall, a hand's width off it, the wall seen at a slant at each
//! quality with AO, the occlusion alone (the AO debug view). Along lines on the wall at an even distance from the block the
//! occlusion keeps to a straight line (the stripes a noise left where the filter did not average it waved across them), and
//! the wall beside the block still darkens against the open wall. And the same wall seen at a grazing angle, along it from near
//! the floor's corner: far from the block, the wall takes no occlusion from itself and has no bands.

use torque_render_core::{Frame, Gpu};
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::{DebugView, HdWorld};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdMode, Quality, bounding_sphere, material,
};
use torque_render_scene::math::Mat;
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (320, 240);

fn quads(label: &str, quads: &[([[f32; 3]; 4], [f32; 3])]) -> Draw {
    let mut vertices = Vec::new();
    for (corners, normal) in quads {
        let v = |p: [f32; 3]| Vertex {
            position: p,
            normal: *normal,
            color: [200, 200, 200, 255],
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
fn block(l: [f32; 3], h: [f32; 3]) -> Draw {
    let c = |x: usize, y: usize, z: usize| [[l[0], h[0]][x], [l[1], h[1]][y], [l[2], h[2]][z]];
    quads(
        "block",
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
                [c(0, 0, 0), c(1, 0, 0), c(1, 1, 0), c(0, 1, 0)],
                [0.0, 0.0, -1.0],
            ),
            (
                [c(0, 0, 1), c(1, 0, 1), c(1, 1, 1), c(0, 1, 1)],
                [0.0, 0.0, 1.0],
            ),
        ],
    )
}

fn scene(quality: Quality, full_res: bool) -> PlainScene {
    let mut mode = HdMode::new(quality);
    mode.image.ao_full_res = full_res;
    let mut s = PlainScene {
        mode: Some(mode),
        ..PlainScene::default()
    };
    s.sun.towards = [-0.6, -0.2, 0.775];
    s.sun.diffuse = [0.95, 0.92, 0.85];
    s.sun.ambient = [0.42, 0.45, 0.52];
    s.sun.shadow = [0.35, 0.38, 0.45];
    s.ao_radius = 1.0;
    // The floor and the wall along x = 0 (facing +x).
    let room = quads(
        "room",
        &[
            (
                [
                    [0.0, -20.0, 0.0],
                    [20.0, -20.0, 0.0],
                    [20.0, 20.0, 0.0],
                    [0.0, 20.0, 0.0],
                ],
                [0.0, 0.0, 1.0],
            ),
            (
                [
                    [0.0, -20.0, 0.0],
                    [0.0, 20.0, 0.0],
                    [0.0, 20.0, 4.0],
                    [0.0, -20.0, 4.0],
                ],
                [1.0, 0.0, 0.0],
            ),
        ],
    );
    s.planes = vec![HdDraw::from_fixed(&room, material::KIND_PLANE, &[])];
    let draws = vec![HdDraw::from_fixed(
        &block([0.1, -0.4, 0.9], [0.9, 0.4, 1.7]),
        material::KIND_SHAPE,
        &[],
    )];
    let (center, radius) = bounding_sphere(&draws);
    s.objects = vec![DynamicObject {
        ghost: 1,
        transform: Mat::IDENTITY,
        prev_transform: Mat::IDENTITY,
        translucent: false,
        centre: center,
        radius,
        draws,
        previous: None,
        stamp: None,
        nodes: Vec::new(),
    }];
    s
}

/// The occlusion alone at `quality` (`full_res`: estimated at the whole frame's resolution) from `from` looking at `to`, and the
/// camera.
fn draw(
    gpu: &Gpu,
    (quality, full_res): (Quality, bool),
    from: [f32; 3],
    to: [f32; 3],
) -> (Frame, HdCamera) {
    let mut s = scene(quality, full_res);
    let eye = look_at(from, to);
    s.eye = Some(eye);
    s.fov = 70;
    s.size = SIZE;
    s.generation += 1;
    let camera = HdCamera::new(eye, 70.0f32, s.visible_distance, SIZE.0, SIZE.1);
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.debug = DebugView::AmbientOcclusion;
    world.upload(&s);
    let format = target.color.texture().format();
    world
        .draw(&s, &camera, Some((&target.color, format, [0, 0])))
        .expect("a frame");
    (gpu.read_back(&target).expect("read back"), camera)
}

/// The pixel where `p` is seen.
fn pixel(frame: &Frame, camera: &HdCamera, p: [f32; 3]) -> (i32, i32) {
    let m = camera.view_proj;
    let clip = [0, 1, 3].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r]);
    let (x, y) = (clip[0] / clip[2], clip[1] / clip[2]);
    let px = (
        ((x + 1.0) * 0.5 * frame.width as f32) as i32,
        ((1.0 - y) * 0.5 * frame.height as f32) as i32,
    );
    let (w, h) = (frame.width as i32, frame.height as i32);
    assert!(
        (8..w - 8).contains(&px.0) && (8..h - 8).contains(&px.1),
        "{p:?} at {px:?}, off the frame"
    );
    px
}

fn value(frame: &Frame, x: i32, y: i32) -> f32 {
    f32::from(frame.rgba[(y as usize * frame.width as usize + x as usize) * 4])
}

/// The occlusion (0 to 255) at `n` points from `a` to `b`, and its RMS about the line fitted to it (the banding along them).
fn along(frame: &Frame, camera: &HdCamera, a: [f32; 3], b: [f32; 3], n: usize) -> (f32, f32) {
    let values: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            let p = [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t);
            let (x, y) = pixel(frame, camera, p);
            value(frame, x, y)
        })
        .collect();
    let m = n as f32;
    let mean_t = (m - 1.0) / 2.0;
    let mean_v = values.iter().sum::<f32>() / m;
    let (mut cov, mut var) = (0.0, 0.0);
    for (i, v) in values.iter().enumerate() {
        cov += (i as f32 - mean_t) * (v - mean_v);
        var += (i as f32 - mean_t).powi(2);
    }
    let slope = cov / var;
    let sq: f32 = values
        .iter()
        .enumerate()
        .map(|(i, v)| (v - mean_v - slope * (i as f32 - mean_t)).powi(2))
        .sum();
    (mean_v, (sq / m).sqrt())
}

#[test]
fn the_wall_about_a_block_darkens_without_banding() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let mut failures = Vec::new();
    for quality in [
        (Quality::Medium, false),
        (Quality::High, false),
        (Quality::Ultra, false),
        (Quality::Ultra, true),
    ] {
        let (frame, camera) = draw(&gpu, quality, [3.5, -2.5, 2.0], [0.0, -0.3, 1.1]);
        // Lines on the wall at a distance from the block (beside it and over it), along which its occlusion is about even.
        let lines = |r: f32| {
            [
                ([0.0, -0.4 - r, 1.0], [0.0, -0.4 - r, 1.6]),
                ([0.0, -0.3, 1.7 + r], [0.0, 0.3, 1.7 + r]),
            ]
            .map(|(a, b)| along(&frame, &camera, a, b, 40))
        };
        let close = lines(0.1);
        let far = [lines(0.3), lines(0.4)].concat();
        // The mean swing about the lines (the banding).
        let banding = far.iter().map(|l| l.1).sum::<f32>() / far.len() as f32;
        let (open, _) = along(&frame, &camera, [0.0, 1.6, 2.4], [0.0, 2.6, 2.4], 20);
        eprintln!(
            "{quality:?}: close {:.1} and {:.1}, open {open:.1}; the banding {banding:.2}",
            close[0].0, close[1].0
        );
        if banding >= 1.1 {
            failures.push(format!(
                "{quality:?}: the occlusion swings {banding} about its lines on the wall"
            ));
        }
        if close[0].0.min(close[1].0) >= open - 10.0 {
            failures.push(format!(
                "{quality:?}: the wall beside the block {close:?} against the open wall {open}"
            ));
        }
        // The wall at a grazing angle (about 13 degrees where the line starts), out of the block's and the floor's reach.
        let (frame, camera) = draw(&gpu, quality, [0.9, -4.0, 1.4], [0.0, 0.5, 1.1]);
        let graze = along(&frame, &camera, [0.0, -2.9, 1.9], [0.0, -1.8, 1.9], 40);
        let (open, _) = along(&frame, &camera, [0.0, 2.5, 1.9], [0.0, 3.5, 1.9], 20);
        eprintln!(
            "{quality:?}: the grazing wall {:.1} (swing {:.2}) against {open:.1}",
            graze.0, graze.1
        );
        if graze.0 < open - 2.0 || graze.1 >= 1.0 {
            failures.push(format!(
                "{quality:?}: the grazing wall {graze:?} against the open wall {open}"
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
