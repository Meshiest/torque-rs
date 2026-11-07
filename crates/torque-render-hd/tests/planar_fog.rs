//! The fog in the water's planar reflection: a wall far across the water seen in it is fogged as the eye sees it along the
//! reflected path (the eye down to the water, then up to the wall): the distance fog over the path's length and the height fog
//! along its two legs, toward the sky cube's color (the sky those views draw).

use std::sync::Arc;

use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::sky::DEFAULT_HEIGHT_FOG;
use torque_render_scene::hd::{
    HdCamera, HdDraw, HdMode, HdScene, Quality, material, srgb_to_linear,
};
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Texture, Vertex};

const SIZE: (u32, u32) = (160, 96);
const FOV: u8 = 30;
/// The eye's height over the water (at height 0), the view's pitch down (degrees) and the wall's distance along x.
const EYE: f32 = 40.0;
const PITCH: f32 = 8.0;
const WALL: f32 = 900.0;
const FOG: (f32, f32) = (50.0, 4000.0);
/// The sky cube holds the clear color alone (the scene draws no sky), unlike the Sky's fog color.
const CLEAR: [f32; 3] = [0.2, 0.5, 0.9];
const FOG_COLOR: [f32; 3] = [0.9, 0.6, 0.3];

fn quad(
    label: &str,
    corners: [[f32; 3]; 4],
    normal: [f32; 3],
    color: [u8; 4],
    blend: Blend,
) -> Draw {
    let v = |p: [f32; 3]| Vertex {
        position: p,
        normal,
        color,
        uv: [0.0; 2],
    };
    let [a, b, c, d] = corners.map(v);
    Draw {
        label: label.into(),
        vertices: vec![a, b, c, a, c, d],
        lighting: false,
        tex_env: TexEnv::None,
        texture: None,
        blend,
        cull: Cull::None,
        depth_test: true,
        depth_write: blend == Blend::Opaque,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

fn scene(wall: u8, fog: bool) -> PlainScene {
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(HdMode::new(Quality::High)),
        time: 0.0,
    };
    let s = &mut scene.frame;
    let _ = s.textures.intern(Texture {
        path: "white".into(),
        width: 1,
        height: 1,
        levels: Arc::new(vec![vec![255; 4]]),
    });
    let h = 3000.0;
    let wall = quad(
        "wall",
        [[WALL, -h, 0.0], [WALL, h, 0.0], [WALL, h, h], [WALL, -h, h]],
        [-1.0, 0.0, 0.0],
        [wall, wall, wall, 255],
        Blend::Opaque,
    );
    s.planes = vec![HdDraw::from_fixed(&wall, material::KIND_PLANE, &[])];
    let w = 2000.0;
    let water = quad(
        "water",
        [[-w, -w, 0.0], [w, -w, 0.0], [w, w, 0.0], [-w, w, 0.0]],
        [0.0, 0.0, 1.0],
        [40, 80, 90, 128],
        Blend::Alpha,
    );
    s.water = vec![HdDraw::from_fixed(&water, material::KIND_PLANE, &[])];
    s.visible_distance = FOG.1;
    s.clear = CLEAR;
    s.fog.color = FOG_COLOR;
    s.fog.linear = false;
    // Without fog: a range that starts past the visible distance.
    (s.fog.start, s.fog.end) = if fog { FOG } else { (FOG.1 + 1.0, FOG.1 + 2.0) };
    s.fog_distance = s.fog.start;
    // The sun behind the eye (the fog is not held near it).
    let t = [-0.8f32, 0.0, 0.6];
    s.sun.towards = t;
    scene
}

/// The planar reflection's mean over its center 2 x 2 pixels (linear).
fn planar_center(world: &mut HdWorld<NoExtension>, scene: &mut PlainScene) -> [f32; 3] {
    let at = [0.0, 0.0, EYE];
    let p = PITCH.to_radians();
    let eye = look_at(at, [100.0 * p.cos(), 0.0, EYE - 100.0 * p.sin()]);
    scene.frame.eye = Some(eye);
    scene.frame.fov = FOV;
    scene.frame.size = SIZE;
    let camera = HdCamera::new(eye, FOV, FOG.1, SIZE.0, SIZE.1);
    world.reset_history();
    for _ in 0..3 {
        scene.frame.generation += 1;
        world.upload(scene);
        world.draw(scene, &camera, None).expect("draw");
    }
    let (w, h, px) = world.read_planar().expect("planar");
    let mut sum = [0.0; 3];
    for y in h / 2 - 1..=h / 2 {
        for x in w / 2 - 1..=w / 2 {
            let c = px[(y * w + x) as usize];
            for (s, c) in sum.iter_mut().zip(c) {
                *s += c / 4.0;
            }
        }
    }
    sum
}

/// The height fog's optical depth along a segment `d` long from height `z0` to `z1` (skyfog.wgsl's).
fn height_depth(z0: f32, z1: f32, d: f32) -> f32 {
    let f = DEFAULT_HEIGHT_FOG;
    let (a, b) = (z0 - f.base, z1 - f.base);
    let e0 = (-a.max(0.0) / f.falloff).exp();
    if (b - a).abs() < 1e-3 {
        return f.density * d * e0;
    }
    let e1 = (-b.max(0.0) / f.falloff).exp();
    f.density * d * f.falloff * (e0 - e1) / (b - a)
}

/// The share of the wall's color the fog leaves along the view's center ray, reflected at the water: the distance fog over the
/// whole path, the height fog over its legs (`reflected`), or over the straight line from the mirrored eye (the old form).
fn expected_keep(reflected: bool) -> f32 {
    let p = PITCH.to_radians();
    let length = WALL / p.cos();
    let down = EYE / p.sin();
    let up = length - down;
    let height = up * p.sin();
    let near = ((FOG.1 - length) / (FOG.1 - FOG.0)).clamp(0.0, 1.0);
    let od = if reflected {
        height_depth(EYE, 0.0, down) + height_depth(0.0, height, up)
    } else {
        height_depth(-EYE, height, length)
    };
    near * near * (-od).exp()
}

#[test]
fn the_planar_reflection_is_fogged_along_the_reflected_path() {
    let gpu = match torque_render_core::Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            return;
        }
    };
    let mut world = HdWorld::<NoExtension>::new(&gpu);
    let mut run = |wall: u8, fog: bool| planar_center(&mut world, &mut scene(wall, fog));
    let white = run(255, true);
    let black = run(0, true);
    let bare = run(255, false);
    let keep = [0, 1, 2].map(|k| (white[k] - black[k]) / bare[k]);
    let want = expected_keep(true);
    let old = expected_keep(false);
    assert!(
        (want - old).abs() > 0.03,
        "the paths' fogs differ ({want} {old})"
    );
    for (k, keep) in keep.iter().enumerate() {
        assert!(
            (keep - want).abs() < 0.01,
            "channel {k}: keep {keep} against the reflected path's {want} (the mirrored eye's straight line: {old})"
        );
    }
    // The fog's color: the sky cube's (the clear color here, within its half floats and mips), not the Sky's fog color.
    for (k, (black, keep)) in black.iter().zip(keep).enumerate() {
        let fog = black / (1.0 - keep);
        let clear = srgb_to_linear(CLEAR[k]);
        assert!(
            (fog - clear).abs() < 0.05 * clear.max(0.1),
            "channel {k}: the fog's color {fog} against the sky cube's {clear}"
        );
    }
}
