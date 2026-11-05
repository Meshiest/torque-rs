//! The scene's smooth occlusion (`HdScene::smooth_occlusion`): a box on a floor close up, its occlusion strong and in the sun's
//! light too, drawn as a history's first frame without and with it: the floor at the box's foot loses its grain (the high
//! frequencies over a flat patch) and keeps its darkening. And the box sliding along the floor: the floor it has left keeps no
//! trail of its occlusion.

use torque_render_core::{Frame, Gpu};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{DynamicObject, HdCamera, HdDraw, bounding_sphere, material};
use torque_render_scene::math::Mat;
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (320, 240);

fn quads(label: &str, quads: &[([[f32; 3]; 4], [f32; 3])]) -> Draw {
    let mut vertices = Vec::new();
    for (corners, normal) in quads {
        let v = |p: [f32; 3]| Vertex {
            position: p,
            normal: *normal,
            color: [200, 180, 160, 255],
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

/// The four sides of a box from (-1, -1 + `dy`, 0) to (1, 1 + `dy`, 2).
fn box_draw(dy: f32) -> Draw {
    let (l, h) = (-1.0, 1.0);
    let mut d = box_sides(l, h);
    for v in &mut d.vertices {
        v.position[1] += dy;
    }
    d
}

fn box_sides(l: f32, h: f32) -> Draw {
    quads(
        "box",
        &[
            (
                [[l, l, 0.0], [l, h, 0.0], [l, h, 2.0], [l, l, 2.0]],
                [-1.0, 0.0, 0.0],
            ),
            (
                [[h, l, 0.0], [h, h, 0.0], [h, h, 2.0], [h, l, 2.0]],
                [1.0, 0.0, 0.0],
            ),
            (
                [[l, l, 0.0], [h, l, 0.0], [h, l, 2.0], [l, l, 2.0]],
                [0.0, -1.0, 0.0],
            ),
            (
                [[l, h, 0.0], [h, h, 0.0], [h, h, 2.0], [l, h, 2.0]],
                [0.0, 1.0, 0.0],
            ),
        ],
    )
}

fn scene(smooth: bool) -> PlainScene {
    let mut s = PlainScene::default();
    s.sun.towards = [-0.6, -0.2, 0.775];
    s.sun.diffuse = [0.95, 0.92, 0.85];
    s.sun.ambient = [0.42, 0.45, 0.52];
    s.sun.shadow = [0.25, 0.27, 0.32];
    s.ao_radius = 1.0;
    s.ao_sun = 0.5;
    s.smooth_occlusion = smooth;
    let floor = quads(
        "floor",
        &[(
            [
                [-20.0, -20.0, 0.0],
                [20.0, -20.0, 0.0],
                [20.0, 20.0, 0.0],
                [-20.0, 20.0, 0.0],
            ],
            [0.0, 0.0, 1.0],
        )],
    );
    s.planes = vec![HdDraw::from_fixed(&floor, material::KIND_PLANE, &[])];
    s.objects = vec![box_object(0.0, 0.0)];
    s
}

/// The box moved by `dy`, `moved` since the last frame.
fn box_object(dy: f32, moved: f32) -> DynamicObject {
    let draws = vec![HdDraw::from_fixed(&box_draw(dy), material::KIND_SHAPE, &[])];
    let (center, radius) = bounding_sphere(&draws);
    let mut prev_transform = Mat::IDENTITY;
    prev_transform.set_column3([0.0, -moved, 0.0]);
    DynamicObject {
        ghost: 1,
        transform: Mat::IDENTITY,
        prev_transform,
        translucent: false,
        centre: center,
        radius,
        draws,
        previous: None,
        stamp: None,
        nodes: Vec::new(),
    }
}

/// The first frame of a history with `smooth`, and its camera: close to the box's foot.
fn draw(gpu: &Gpu, smooth: bool) -> (Frame, HdCamera) {
    let mut s = scene(smooth);
    let eye = look_at([-3.2, -1.6, 1.2], [-1.2, 0.0, 0.2]);
    s.eye = Some(eye);
    s.fov = 70;
    s.size = SIZE;
    s.generation += 1;
    let camera = HdCamera::new(eye, 70.0f32, s.visible_distance, SIZE.0, SIZE.1);
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
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

fn luma(frame: &Frame, x: i32, y: i32) -> f32 {
    let i = (y as usize * frame.width as usize + x as usize) * 4;
    let c = &frame.rgba[i..i + 3];
    0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2])
}

/// Over the 9 x 9 pixels about (x, y): the mean luminance and the RMS of each pixel's luminance less its 5 x 5 mean (the grain).
fn grain(frame: &Frame, (x, y): (i32, i32)) -> (f32, f32) {
    let (mut sum, mut sq) = (0.0, 0.0);
    for v in y - 4..=y + 4 {
        for u in x - 4..=x + 4 {
            let mut local = 0.0;
            for dv in -2..=2 {
                for du in -2..=2 {
                    local += luma(frame, u + du, v + dv);
                }
            }
            let l = luma(frame, u, v);
            let r = l - local / 25.0;
            sum += l;
            sq += r * r;
        }
    }
    (sum / 81.0, (sq / 81.0).sqrt())
}

#[test]
fn the_occlusion_at_a_box_s_foot_loses_its_grain_and_keeps_its_darkening() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let (rough, camera) = draw(&gpu, false);
    let (smooth, _) = draw(&gpu, true);
    // The floor a fifth of a unit out from the box's sunlit face, and 1.3 units out (past the occlusion's reach).
    let foot = pixel(&rough, &camera, [-1.2, -0.3, 0.0]);
    let open = pixel(&rough, &camera, [-2.3, -0.2, 0.0]);
    let (rough_mean, rough_grain) = grain(&rough, foot);
    let (smooth_mean, smooth_grain) = grain(&smooth, foot);
    let (open_mean, _) = grain(&smooth, open);
    eprintln!(
        "foot: {rough_mean:.1} grain {rough_grain:.2} without, {smooth_mean:.1} grain {smooth_grain:.2} with; open {open_mean:.1}"
    );
    assert!(rough_grain > 1.0, "the foot's grain without: {rough_grain}");
    assert!(
        smooth_grain < 0.4 * rough_grain,
        "the foot's grain: {rough_grain} without, {smooth_grain} with"
    );
    assert!(
        (smooth_mean - rough_mean).abs() < 3.0,
        "the foot's mean: {rough_mean} without, {smooth_mean} with"
    );
    assert!(
        smooth_mean < open_mean - 10.0,
        "the foot {smooth_mean} against the open floor {open_mean}"
    );
}

/// The box slid by `step` a frame for `frames` frames to its place in [`scene`], with `smooth`: the last frame, and the camera.
fn slide(gpu: &Gpu, smooth: bool, step: f32, frames: u32) -> (Frame, HdCamera) {
    let mut s = scene(smooth);
    let eye = look_at([-3.2, -1.6, 1.2], [-1.2, 0.0, 0.2]);
    s.eye = Some(eye);
    s.fov = 70;
    s.size = SIZE;
    let camera = HdCamera::new(eye, 70.0f32, s.visible_distance, SIZE.0, SIZE.1);
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    let format = target.color.texture().format();
    for k in 0..frames {
        let moved = if k == 0 { 0.0 } else { step };
        s.objects = vec![box_object(-step * (frames - 1 - k) as f32, moved)];
        s.generation += 1;
        world.upload(&s);
        world
            .draw(&s, &camera, Some((&target.color, format, [0, 0])))
            .expect("a frame");
    }
    (gpu.read_back(&target).expect("read back"), camera)
}

#[test]
fn a_sliding_box_leaves_no_trail_of_its_occlusion() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    // The floor beside where the box's back corner was a few frames before: what a history holds there is the trail.
    let left = [-1.3, -1.5, 0.0];
    let mut trail = [0.0; 2];
    for (i, smooth) in [false, true].into_iter().enumerate() {
        let (moving, camera) = slide(&gpu, smooth, 0.08, 12);
        let (still, _) = slide(&gpu, smooth, 0.0, 12);
        let p = pixel(&moving, &camera, left);
        trail[i] = grain(&still, p).0 - grain(&moving, p).0;
    }
    eprintln!("the trail: {} without, {} with", trail[0], trail[1]);
    assert!(trail[0] > 2.0, "the trail without: {}", trail[0]);
    assert!(
        trail[1].abs() < 0.3 * trail[0],
        "the trail: {} without, {} with",
        trail[0],
        trail[1]
    );
}
