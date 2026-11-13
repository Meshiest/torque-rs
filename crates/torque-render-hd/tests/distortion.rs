//! The screen-space distortion: a card whose normal map is one color bends a barcode wall behind it by the pixels the look's
//! strength and distance give; a bar in front of the card is neither bent nor pulled into the bend; a distorter out of sight
//! leaves the frame as it is without one, byte for byte.

use std::sync::Arc;

use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::distortion::{DistortionLook, HdDistorter, sphere};
use torque_render_scene::hd::kit::{Lcg, look_at};
use torque_render_scene::hd::{
    HdCamera, HdDraw, HdMode, HdScene, HdVertex, Quality, material, oct_encode,
};
use torque_render_scene::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Texture, Vertex};

const SIZE: (u32, u32) = (160, 96);
/// The card's distance from the eye (along the view), the wall's and the bar's.
const CARD: f32 = 10.0;
const WALL: f32 = 20.0;
const BAR: f32 = 5.0;
/// The look's strength: at 90 degrees across 160 pixels, `strength / 2 / distance` of the half width.
const STRENGTH: f32 = 0.5;
const SHIFT_PX: f32 = STRENGTH * 0.5 / CARD * SIZE.0 as f32;

fn quad(corners: [[f32; 3]; 4], normal: [f32; 3], color: [u8; 4]) -> Draw {
    let v = |p: [f32; 3]| Vertex {
        position: p,
        normal,
        color,
        uv: [0.0; 2],
    };
    let [a, b, c, d] = corners.map(v);
    Draw {
        label: "quad".into(),
        vertices: vec![a, b, c, a, c, d],
        lighting: false,
        tex_env: TexEnv::None,
        texture: None,
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: true,
        depth_write: true,
        fog: false,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// A wall of vertical strips of uneven widths, black and white in turn, across the view at `WALL`.
fn barcode() -> Vec<HdDraw> {
    let mut rng = Lcg::new(7);
    let mut y = -WALL;
    let mut white = false;
    let mut out = Vec::new();
    while y < WALL {
        let w = 0.3 + 1.2 * rng.unit();
        let c = if white { 255 } else { 20 };
        let q = quad(
            [
                [WALL, -y, -WALL],
                [WALL, -(y + w), -WALL],
                [WALL, -(y + w), WALL],
                [WALL, -y, WALL],
            ],
            [-1.0, 0.0, 0.0],
            [c, c, c, 255],
        );
        out.push(HdDraw::from_fixed(&q, material::KIND_PLANE, &[]));
        y += w;
        white = !white;
    }
    out
}

/// A card at `CARD` facing the eye, `half` wide and high, two-sided.
fn card(half: f32) -> HdDraw {
    let mut d = HdDraw::empty();
    let n = oct_encode([-1.0, 0.0, 0.0]);
    for (p, uv) in [
        ([CARD, half, -half], [0.0, 1.0]),
        ([CARD, -half, -half], [1.0, 1.0]),
        ([CARD, -half, half], [1.0, 0.0]),
        ([CARD, half, half], [0.0, 0.0]),
    ] {
        d.vertices.push(HdVertex {
            position: p,
            normal: n,
            uv,
            color: [255; 4],
            material: 0,
            extra: 0,
        });
    }
    d.indices = vec![0, 1, 2, 0, 2, 3];
    d
}

struct Setup {
    scene: PlainScene,
    map: usize,
}

fn setup(bar: bool) -> Setup {
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(HdMode::new(Quality::High)),
        time: 0.0,
    };
    let s = &mut scene.frame;
    // The map: every texel bends to screen +x (r 1, g 0.5: the normal map's xy (1, 0)).
    let map = s.textures.intern(Texture {
        format: torque_render_scene::TexelFormat::Rgba8,
        path: "bend".into(),
        width: 1,
        height: 1,
        levels: Arc::new(vec![vec![255, 128, 255, 255]]),
    });
    s.planes = barcode();
    if bar {
        let q = quad(
            [
                [BAR, 0.3, -1.0],
                [BAR, -0.3, -1.0],
                [BAR, -0.3, 1.0],
                [BAR, 0.3, 1.0],
            ],
            [-1.0, 0.0, 0.0],
            [255, 0, 0, 255],
        );
        s.planes
            .push(HdDraw::from_fixed(&q, material::KIND_PLANE, &[]));
    }
    s.visible_distance = 200.0;
    (s.fog.start, s.fog.end) = (300.0, 400.0);
    s.fog_distance = 300.0;
    s.clear = [0.3, 0.3, 0.3];
    Setup { scene, map }
}

fn bend(map: usize) -> DistortionLook {
    DistortionLook {
        strength: STRENGTH,
        map: Some((
            map,
            Sampler {
                repeat: true,
                mag: Filter::Nearest,
                min: Filter::Nearest,
                mip: Filter::Nearest,
                mipmaps: false,
            },
        )),
        ..DistortionLook::default()
    }
}

fn draw(world: &mut HdWorld<NoExtension>, scene: &mut PlainScene) -> Vec<u8> {
    let eye = look_at([0.0, 0.0, 0.0], [10.0, 0.0, 0.0]);
    scene.frame.eye = Some(eye);
    scene.frame.fov = 90;
    scene.frame.size = SIZE;
    let camera = HdCamera::new(eye, 90.0f32, 200.0, SIZE.0, SIZE.1);
    world.reset_history();
    for _ in 0..6 {
        scene.frame.generation += 1;
        world.upload(scene);
        world.draw(scene, &camera, None).expect("draw");
    }
    world.read_ldr().expect("read").rgba
}

fn gray_row(px: &[u8], y: u32, x0: u32, x1: u32) -> Vec<f32> {
    (x0..x1)
        .map(|x| f32::from(px[((y * SIZE.0 + x) * 4) as usize]))
        .collect()
}

/// The shift (pixels) that best lays `moved` over `plain` (`moved[x]` = `plain[x + k]`).
fn best_shift(plain: &[f32], moved: &[f32], reach: i32) -> i32 {
    let n = plain.len() as i32;
    (-reach..=reach)
        .min_by(|&a, &b| {
            let err = |k: i32| -> f32 {
                (reach..n - reach)
                    .map(|x| (moved[x as usize] - plain[(x + k) as usize]).abs())
                    .sum()
            };
            err(a).total_cmp(&err(b))
        })
        .expect("a shift")
}

fn gpu() -> Option<torque_render_core::Gpu> {
    match torque_render_core::Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            None
        }
    }
}

#[test]
fn a_known_bend_moves_the_wall_behind_by_the_expected_pixels() {
    let Some(gpu) = gpu() else { return };
    let mut world = HdWorld::<NoExtension>::new(&gpu);
    let Setup { mut scene, map } = setup(false);
    let plain = draw(&mut world, &mut scene);
    scene.frame.distorters = vec![HdDistorter::new(card(4.0), bend(map))];
    let bent = draw(&mut world, &mut scene);
    // The card covers the middle 64 pixels across (x 48..112); a row through it, its edges' fade left out.
    let (y, x0, x1) = (SIZE.1 / 2, 56, 104);
    let k = best_shift(&gray_row(&plain, y, x0, x1), &gray_row(&bent, y, x0, x1), 8);
    assert!(
        (k as f32 - SHIFT_PX).abs() <= 1.0,
        "the wall moved {k} pixels, not {SHIFT_PX}"
    );
    // Outside the card the frame is the plain one.
    let outside = gray_row(&plain, 8, 0, SIZE.0)
        .iter()
        .zip(gray_row(&bent, 8, 0, SIZE.0))
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(outside <= 2.0, "a row above the card changed by {outside}");
}

#[test]
fn what_is_in_front_is_neither_bent_nor_pulled_into_the_bend() {
    let Some(gpu) = gpu() else { return };
    let mut world = HdWorld::<NoExtension>::new(&gpu);
    let Setup { mut scene, map } = setup(true);
    let plain = draw(&mut world, &mut scene);
    scene.frame.distorters = vec![HdDistorter::new(card(4.0), bend(map))];
    let bent = draw(&mut world, &mut scene);
    let red = |px: &[u8], i: usize| px[i] > 150 && px[i + 1] < 60 && px[i + 2] < 60;
    let (mut bar, mut moved, mut pulled) = (0, 0, 0);
    let bent_wall = (0..plain.len())
        .step_by(4)
        .filter(|&i| !red(&plain, i) && plain[i].abs_diff(bent[i]) > 40)
        .count();
    assert!(
        bent_wall > 300,
        "the wall about the bar is bent ({bent_wall} pixels changed)"
    );
    for i in (0..plain.len()).step_by(4) {
        match (red(&plain, i), red(&bent, i)) {
            (true, true) => bar += 1,
            (true, false) => moved += 1,
            (false, true) => pulled += 1,
            (false, false) => {}
        }
    }
    assert!(bar > 100, "the bar is seen ({bar} pixels)");
    assert_eq!(
        moved, 0,
        "pixels of the bar in front of the card were bent away"
    );
    assert_eq!(pulled, 0, "pixels beside the bar took its color");
}

#[test]
fn a_distorter_out_of_sight_leaves_the_frame_as_it_is_without_one() {
    let Some(gpu) = gpu() else { return };
    let mut world = HdWorld::<NoExtension>::new(&gpu);
    let Setup { mut scene, map } = setup(true);
    let none = draw(&mut world, &mut scene);
    scene.frame.distorters = vec![HdDistorter::new(
        sphere([-30.0, 0.0, 0.0], 2.0, 8, 12, 255),
        bend(map),
    )];
    let behind = draw(&mut world, &mut scene);
    assert!(
        none == behind,
        "a distorter behind the eye changed the frame"
    );
}
