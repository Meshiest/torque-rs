//! The sun's shadows reach as far from the eye whichever way the camera looks: a floor wholly in the shadow of a plate high over
//! it, seen from one eye turned to several yaws and pitches, darkens out to the same distance from the eye in the middle of the view
//! and toward its sides, and fades out there (no hard edge). The floor is a ground plane (as a terrain is), which the shadows'
//! range reaches over though it is not a cell.

use torque_render_core::{Frame, Gpu};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::shadow::{SHADOW_DISTANCE, SHADOW_FADE};
use torque_render_scene::hd::{DynamicObject, HdCamera, HdDraw, bounding_sphere, material};
use torque_render_scene::math::Mat;
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (480, 360);
const FOV: f32 = 60.0;
/// The eye, over the middle of the floor.
const EYE: [f32; 3] = [0.0, 0.0, 40.0];
/// The floor's and the plate's half side, and the plate's height.
const FLOOR: f32 = 800.0;
const PLATE: f32 = 700.0;
const PLATE_Z: f32 = 60.0;

fn quad(label: &str, corners: [[f32; 3]; 4], normal: [f32; 3]) -> Draw {
    let v = |p: [f32; 3]| Vertex {
        position: p,
        normal,
        color: [180, 180, 180, 255],
        uv: [0.0; 2],
    };
    let [a, b, c, d] = corners.map(v);
    Draw {
        label: label.to_string(),
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
    }
}

/// The floor under the plate, seen from [`EYE`] at `yaw` and `pitch` (degrees, down positive).
fn draw(gpu: &Gpu, yaw: f32, pitch: f32) -> (Frame, HdCamera) {
    let mut s = PlainScene::default();
    let t = [0.25f32, 0.1, 1.0];
    let n = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
    s.sun.towards = t.map(|v| v / n);
    s.sun.diffuse = [1.0; 3];
    s.sun.ambient = [0.08; 3];
    s.sun.shadow = [0.08; 3];
    s.ao_radius = 0.0;
    s.visible_distance = 2000.0;
    s.fog_distance = 2000.0;
    s.fog.start = 1900.0;
    s.fog.end = 2000.0;
    let f = FLOOR;
    let floor = quad(
        "floor",
        [[-f, -f, 0.0], [f, -f, 0.0], [f, f, 0.0], [-f, f, 0.0]],
        [0.0, 0.0, 1.0],
    );
    s.planes = vec![HdDraw::from_fixed(&floor, material::KIND_PLANE, &[])];
    s.planes_generation += 1;
    let (p, z) = (PLATE, PLATE_Z);
    let plate = quad(
        "plate",
        [[-p, -p, z], [p, -p, z], [p, p, z], [-p, p, z]],
        [0.0, 0.0, -1.0],
    );
    let draws = vec![HdDraw::from_fixed(&plate, material::KIND_SHAPE, &[])];
    let (centre, radius) = bounding_sphere(&draws);
    s.objects = vec![DynamicObject {
        ghost: 1,
        transform: Mat::IDENTITY,
        prev_transform: Mat::IDENTITY,
        translucent: false,
        centre,
        radius,
        draws,
        previous: None,
        stamp: None,
        nodes: Vec::new(),
    }];
    let (y, d) = (yaw.to_radians(), pitch.to_radians());
    let at = [
        EYE[0] + y.sin() * d.cos(),
        EYE[1] + y.cos() * d.cos(),
        EYE[2] - d.sin(),
    ];
    let view = look_at(EYE, at);
    s.eye = Some(view);
    s.fov = FOV as u8;
    s.size = SIZE;
    s.generation += 1;
    let camera = HdCamera::new(view, FOV, s.visible_distance, SIZE.0, SIZE.1);
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.upload(&s);
    let format = target.color.texture().format();
    for _ in 0..4 {
        world
            .draw(&s, &camera, Some((&target.color, format, [0, 0])))
            .expect("a frame");
    }
    (gpu.read_back(&target).expect("read back"), camera)
}

/// The floor point pixel (`px`, `py`) sees and its distance from the eye, if it sees the floor.
fn floor_at(camera: &HdCamera, px: u32, py: u32) -> Option<f32> {
    let v = camera.view;
    let axis = |k: usize| [v[k], v[4 + k], v[8 + k]];
    let (ax, ay, az) = (axis(0), axis(1), axis(2));
    let [l, r, b, t] = camera.frustum;
    let x = l + (r - l) * (px as f32 + 0.5) / SIZE.0 as f32;
    let y = t - (t - b) * (py as f32 + 0.5) / SIZE.1 as f32;
    let d = [0, 1, 2].map(|k| ax[k] * x + ay[k] * y - az[k] * camera.near);
    if d[2] >= -1e-4 {
        return None;
    }
    let s = -camera.eye[2] / d[2];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let p = [0, 1].map(|k| camera.eye[k] + d[k] * s);
    (p[0].abs() < FLOOR && p[1].abs() < FLOOR).then_some(s * len)
}

fn luminance(frame: &Frame, px: u32, py: u32) -> f32 {
    let i = ((py * frame.width + px) * 4) as usize;
    let c = &frame.rgba[i..i + 3];
    0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2])
}

/// Along the column `px`, the distance from the eye at which the floor turns from shadowed to lit (half way between the two), and
/// the distance over which it does (a quarter to three quarters of the way).
fn reach(frame: &Frame, camera: &HdCamera, px: u32) -> (f32, f32) {
    // From the nearest floor seen (the bottom row) to the farthest.
    let row: Vec<(f32, f32)> = (0..SIZE.1)
        .rev()
        .filter_map(|py| floor_at(camera, px, py).map(|d| (d, luminance(frame, px, py))))
        .collect();
    let mean = |lo: f32, hi: f32| {
        let v: Vec<f32> = row
            .iter()
            .filter(|(d, _)| (lo..hi).contains(d))
            .map(|(_, l)| *l)
            .collect();
        assert!(!v.is_empty(), "the floor between {lo} and {hi} is in view");
        v.iter().sum::<f32>() / v.len() as f32
    };
    let dark = mean(60.0, 110.0);
    let lit = mean(200.0, 320.0);
    assert!(
        lit > dark * 2.5,
        "the plate shadows the floor: {dark} in it, {lit} past it"
    );
    let at = |share: f32| {
        let level = dark + (lit - dark) * share;
        row.iter()
            .find(|(d, l)| *d > 60.0 && *l >= level)
            .map(|(d, _)| *d)
            .expect("the floor turns lit")
    };
    (at(0.5), at(0.75) - at(0.25))
}

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

#[test]
fn the_sun_shadows_reach_the_same_distance_from_the_eye_whichever_way_it_looks() {
    let Some(gpu) = gpu() else { return };
    // Half way through the fade, measured from the eye.
    let want = SHADOW_DISTANCE * (1.0 + SHADOW_FADE) * 0.5;
    let fade = SHADOW_DISTANCE * (1.0 - SHADOW_FADE);
    let mut seen = Vec::new();
    for (yaw, pitch) in [
        (0.0, 20.0),
        (35.0, 20.0),
        (120.0, 20.0),
        (250.0, 20.0),
        (35.0, 12.0),
        (35.0, 28.0),
    ] {
        let (frame, camera) = draw(&gpu, yaw, pitch);
        // The middle of the view and toward its sides (a view depth's end lies some 15 units farther there).
        for px in [SIZE.0 / 10, SIZE.0 / 2, SIZE.0 * 9 / 10] {
            let (at, width) = reach(&frame, &camera, px);
            seen.push((yaw, pitch, px, at, width));
        }
    }
    for &(yaw, pitch, px, at, width) in &seen {
        assert!(
            (at - want).abs() <= 4.0,
            "yaw {yaw} pitch {pitch} column {px}: the shadow ends {at} from the eye, not {want} ({seen:?})"
        );
        assert!(
            width >= 0.25 * fade,
            "yaw {yaw} pitch {pitch} column {px}: it fades over {width} units, a hard edge ({seen:?})"
        );
    }
}
