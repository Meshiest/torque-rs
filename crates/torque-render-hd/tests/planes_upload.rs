//! A window uploads the loose geometry every frame and keeps its planes in place while they are as they were: planes built
//! again (`planes_generation` counted up) are written again, even when their lists are the same length at the same places.

use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{HdCamera, HdDraw, material};
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (160, 120);

fn quads(label: &str, gray: u8, quads: &[([[f32; 3]; 4], [f32; 3])]) -> Draw {
    let mut vertices = Vec::new();
    for (corners, normal) in quads {
        let v = |p: [f32; 3]| Vertex {
            position: p,
            normal: *normal,
            color: [gray, gray, gray, 255],
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

fn floor(gray: u8) -> HdDraw {
    let f = quads(
        "floor",
        gray,
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
    HdDraw::from_fixed(&f, material::KIND_PLANE, &[])
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
fn planes_built_again_in_the_same_place_are_written_again() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let mut s = PlainScene::default();
    s.sun.towards = [-0.3, -0.2, 0.93];
    s.sun.diffuse = [0.9; 3];
    s.sun.ambient = [0.4; 3];
    s.planes = vec![floor(30)];
    s.planes_generation += 1;
    let eye = look_at([0.0, -8.0, 4.0], [0.0, 0.0, 0.0]);
    s.eye = Some(eye);
    s.fov = 70;
    s.size = SIZE;
    let camera = HdCamera::new(eye, 70.0f32, s.visible_distance, SIZE.0, SIZE.1);
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
    let format = target.color.texture().format();
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    // A static sync, then control syncs (a window's frames).
    let frame = |world: &mut HdWorld<NoExtension>, s: &mut PlainScene, control: bool| {
        s.generation += 1;
        if control {
            s.control_generation += 1;
        }
        world.upload(s);
        world
            .draw(s, &camera, Some((&target.color, format, [0, 0])))
            .expect("a frame");
        let f = gpu.read_back(&target).expect("read back");
        luminance_at(&f, &camera, [0.0, 0.0, 0.0])
    };
    frame(&mut world, &mut s, false);
    let mut dark = 0.0;
    for _ in 0..8 {
        dark = frame(&mut world, &mut s, true);
    }
    // The floor built again in its own lists (the same places and lengths), brighter, counted: written again.
    let bright = floor(230);
    for (v, b) in s.planes[0].vertices.iter_mut().zip(&bright.vertices) {
        *v = *b;
    }
    s.planes_generation += 1;
    let mut lit = 0.0;
    for _ in 0..8 {
        lit = frame(&mut world, &mut s, true);
    }
    assert!(
        lit > dark + 40.0,
        "the floor stayed as it was: {dark} then {lit}"
    );
    // Kept as it is while nothing changes.
    let still = frame(&mut world, &mut s, true);
    assert!((still - lit).abs() < 2.0, "{lit} then {still}");
}
