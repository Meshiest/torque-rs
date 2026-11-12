//! Shadow casters outside the camera's view still shadow what it sees: a box beside the view, lit by a point light that is also
//! outside it, and by a low sun, darkens the floor in view where its shadow falls. Casters are culled against each shadow map's
//! own volume (a cube face, a cascade with its side toward the sun open), never against the camera's.

use torque_render_core::{Frame, Gpu};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{
    DynamicObject, HdCamera, HdDraw, HdLight, bounding_sphere, material,
};
use torque_render_scene::math::Mat;
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (320, 240);
const FOV: f32 = 50.0;

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

/// What lights the floor: a point light, or the sun from `towards`.
#[derive(Clone, Copy)]
enum Light {
    Point([f32; 3]),
    Sun([f32; 3]),
}

/// The floor, lit by `light`, with the box `caster` (when there is one), seen from `eye` toward `at`.
fn draw(
    gpu: &Gpu,
    light: Light,
    caster: Option<([f32; 3], [f32; 3])>,
    eye: [f32; 3],
    at: [f32; 3],
) -> (Frame, HdCamera) {
    let mut s = PlainScene::default();
    s.sun.ambient = [0.08, 0.08, 0.08];
    s.sun.shadow = [0.08, 0.08, 0.08];
    s.ao_radius = 0.0;
    match light {
        Light::Point(p) => {
            s.sun.diffuse = [0.0; 3];
            s.sun.towards = [0.0, 0.0, -1.0];
            s.lights = vec![HdLight {
                position: p,
                color: [6.0, 6.0, 6.0],
                radius: 14.0,
                carrier: None,
            }];
            s.lights_generation += 1;
        }
        Light::Sun(t) => {
            s.sun.towards = t;
            s.sun.diffuse = [1.0, 1.0, 1.0];
        }
    }
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
    if let Some((l, h)) = caster {
        let draws = vec![HdDraw::from_fixed(
            &box_draw(l, h),
            material::KIND_SHAPE,
            &[],
        )];
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
            takes_decals: false,
        }];
    }
    let view = look_at(eye, at);
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

/// Where `p` is seen (pixels), if it is in the frame.
fn pixel(camera: &HdCamera, p: [f32; 3]) -> Option<(i32, i32)> {
    let m = camera.view_proj;
    let clip = [0, 1, 3].map(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r]);
    if clip[2] <= 0.0 {
        return None;
    }
    let (x, y) = (clip[0] / clip[2], clip[1] / clip[2]);
    ((-1.0..=1.0).contains(&x) && (-1.0..=1.0).contains(&y)).then_some((
        ((x + 1.0) * 0.5 * SIZE.0 as f32) as i32,
        ((1.0 - y) * 0.5 * SIZE.1 as f32) as i32,
    ))
}

/// Whether any corner of the box `l`..`h` is in the frame.
fn any_corner_seen(camera: &HdCamera, l: [f32; 3], h: [f32; 3]) -> bool {
    (0..8).any(|i| {
        pixel(
            camera,
            [0, 1, 2].map(|k| if i >> k & 1 == 0 { l[k] } else { h[k] }),
        )
        .is_some()
    })
}

/// The mean luminance (0 to 255) of the 5 x 5 pixels about where `p` is seen.
fn luminance_at(frame: &Frame, camera: &HdCamera, p: [f32; 3]) -> f32 {
    let (px, py) = pixel(camera, p).expect("the point is in view");
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

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

/// The floor at `shadowed` is darker with the box than without it; the floor at `lit` is not.
fn check(
    gpu: &Gpu,
    light: Light,
    caster: ([f32; 3], [f32; 3]),
    eye: [f32; 3],
    at: [f32; 3],
    shadowed: [f32; 3],
    lit: [f32; 3],
) {
    let (bare, camera) = draw(gpu, light, None, eye, at);
    let (with, _) = draw(gpu, light, Some(caster), eye, at);
    assert!(
        !any_corner_seen(&camera, caster.0, caster.1),
        "the box is outside the view"
    );
    if let Light::Point(p) = light {
        assert!(pixel(&camera, p).is_none(), "the light is outside the view");
    }
    let (a, b) = (
        luminance_at(&bare, &camera, shadowed),
        luminance_at(&with, &camera, shadowed),
    );
    assert!(a > 20.0, "the floor is lit there: {a}");
    assert!(
        b < a * 0.6,
        "the box's shadow in view: {a} without it, {b} with it"
    );
    let (c, d) = (
        luminance_at(&bare, &camera, lit),
        luminance_at(&with, &camera, lit),
    );
    assert!(
        (c - d).abs() <= 2.0,
        "the floor out of the shadow: {c} without, {d} with"
    );
}

#[test]
fn a_point_light_and_a_box_outside_the_view_shadow_the_floor_in_it() {
    let Some(gpu) = gpu() else { return };
    // The light at (0, 0, 3) over the box; the shadow reaches about x 4.3 along the floor, the view starts past x 3.
    check(
        &gpu,
        Light::Point([0.0, 0.0, 3.0]),
        ([1.0, -0.5, 0.8], [2.0, 0.5, 1.6]),
        [5.5, -4.0, 2.0],
        [5.5, 0.0, 0.0],
        [3.9, 0.0, 0.0],
        [5.5, -1.6, 0.0],
    );
}

#[test]
fn a_low_sun_and_a_box_outside_the_view_shadow_the_floor_in_it() {
    let Some(gpu) = gpu() else { return };
    // The sun low in the west: the box's shadow runs some 4 units east along the floor, into the view.
    let t = [-0.9f32, 0.0, 0.436];
    let n = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
    check(
        &gpu,
        Light::Sun(t.map(|v| v / n)),
        ([-3.0, -0.5, 0.0], [-2.0, 0.5, 2.0]),
        [2.5, -4.0, 2.0],
        [2.5, 0.0, 0.0],
        [0.6, 0.0, 0.0],
        [2.5, -2.0, 0.0],
    );
}
