//! The scene's share of the ambient occlusion on the sun's light (`HdScene::ao_sun`): a box on a floor in the sun, drawn with
//! none and with all of it. The sunlit floor at the box's foot darkens; the floor far from it does not change.

use torque_render_core::Gpu;
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

/// A box from (-1, -1, 0) to (1, 1, 2).
fn box_draw() -> Draw {
    let (l, h) = (-1.0, 1.0);
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
            (
                [[l, l, 2.0], [h, l, 2.0], [h, h, 2.0], [l, h, 2.0]],
                [0.0, 0.0, 1.0],
            ),
        ],
    )
}

fn scene(ao_sun: f32) -> PlainScene {
    let mut s = PlainScene::default();
    s.sun.towards = [-0.6, -0.2, 0.775];
    s.sun.diffuse = [0.95, 0.92, 0.85];
    s.sun.ambient = [0.42, 0.45, 0.52];
    s.sun.shadow = [0.35, 0.38, 0.45];
    s.ao_radius = 1.0;
    s.ao_sun = ao_sun;
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
    let draws = vec![HdDraw::from_fixed(&box_draw(), material::KIND_SHAPE, &[])];
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

/// The frame of `ao_sun` and its camera.
fn draw(gpu: &Gpu, ao_sun: f32) -> (torque_render_core::Frame, HdCamera) {
    let mut s = scene(ao_sun);
    let eye = look_at([-7.0, -3.0, 3.0], [-1.0, 0.0, 0.3]);
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

/// The mean luminance (0 to 255) of the 5 x 5 pixels about where `p` is seen.
fn luminance_at(frame: &torque_render_core::Frame, camera: &HdCamera, p: [f32; 3]) -> f32 {
    let m = camera.view_proj;
    let clip = [0, 1, 3].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r]);
    let (x, y) = (clip[0] / clip[2], clip[1] / clip[2]);
    let px = ((x + 1.0) * 0.5 * frame.width as f32) as i32;
    let py = ((1.0 - y) * 0.5 * frame.height as f32) as i32;
    let mut sum = 0.0;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let (u, v) = ((px + dx) as usize, (py + dy) as usize);
            let i = (v * frame.width as usize + u) * 4;
            let c = &frame.rgba[i..i + 3];
            sum += 0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2]);
        }
    }
    sum / 25.0
}

#[test]
fn the_sunlit_floor_at_a_box_s_foot_darkens_with_the_occlusion_on_the_sun() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let (off, camera) = draw(&gpu, 0.0);
    let (on, _) = draw(&gpu, 1.0);
    // The floor beside the box's sunlit face, a tenth of a unit out; and the floor three units from it.
    let foot = [-1.1, 0.0, 0.0];
    let far = [-4.0, -3.0, 0.0];
    let (foot_off, foot_on) = (
        luminance_at(&off, &camera, foot),
        luminance_at(&on, &camera, foot),
    );
    let (far_off, far_on) = (
        luminance_at(&off, &camera, far),
        luminance_at(&on, &camera, far),
    );
    assert!(
        foot_on < foot_off * 0.9,
        "the foot: {foot_off} without, {foot_on} with"
    );
    assert!(
        (far_on - far_off).abs() <= 1.0,
        "far off: {far_off} without, {far_on} with"
    );
}
