use glam::{Mat4, Vec3, Vec4};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::{HdCamera, HdLight, HdMode, HdScene, Quality};

use super::*;
use crate::HdWorld;
use crate::ext::{NoExtension, PlainScene};

const SIZE: (u32, u32) = (192, 108);
const FOV: f32 = 70.0;
const FAR: f32 = 300.0;

fn gpu() -> Option<torque_render_core::Gpu> {
    match torque_render_core::Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn scene() -> PlainScene {
    let mut s = HdScene::new(StaticCells::default());
    s.visible_distance = FAR;
    s.size = SIZE;
    s.fov = FOV as u8;
    PlainScene {
        frame: s,
        mode: Some(HdMode::new(Quality::Ultra)),
        time: 0.0,
    }
}

/// What a test reads after drawing: the gathered grid and the camera (the grid's size, far end and texels), and the march's
/// texture (the sun's god rays: rgb).
struct Drawn {
    camera: HdCamera,
    dims: [u32; 3],
    far: f32,
    texels: Vec<[f32; 4]>,
    march: Vec<[f32; 4]>,
}

/// The scene drawn from `at` toward `target` for a few frames.
fn drawn(
    gpu: &torque_render_core::Gpu,
    scene: &mut PlainScene,
    at: [f32; 3],
    target: [f32; 3],
) -> Drawn {
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    let eye = look_at(at, target);
    scene.frame.eye = Some(eye);
    let camera = HdCamera::new(eye, FOV, FAR, SIZE.0, SIZE.1);
    for _ in 0..6 {
        scene.frame.generation += 1;
        world.upload(scene);
        world.draw(scene, &camera, None).expect("a frame");
    }
    let (dims, texels) = world
        .read_froxels()
        .expect("read")
        .expect("Ultra gathers the froxel grid");
    let (_, _, march) = world
        .read_volumetric()
        .expect("read")
        .expect("Ultra marches the god rays");
    let far = torque_render_scene::hd::settings::ImageSettings::DEFAULT
        .volumetric
        .reach
        .min(FAR);
    Drawn {
        camera,
        dims,
        far,
        texels,
        march,
    }
}

/// The world point at view depth `z` on the ray through froxel column (`x`, `y`) of a `dims` grid (the shaders' mapping).
fn column_point(camera: &HdCamera, dims: [u32; 3], x: u32, y: u32, z: f32) -> Vec3 {
    let inv = Mat4::from_cols_array(&camera.view_proj).inverse();
    let u = (x as f32 + 0.5) / dims[0] as f32;
    let v = (y as f32 + 0.5) / dims[1] as f32;
    let (n, f) = (camera.near, camera.far);
    let d = n * (f - z) / (z * (f - n));
    let w = inv * Vec4::new(u * 2.0 - 1.0, 1.0 - v * 2.0, d, 1.0);
    w.truncate() / w.w
}

/// Each froxel with the ends of its column's segment (its slice's near and far depth) and its texel.
fn froxels<'a>(
    camera: &'a HdCamera,
    dims: [u32; 3],
    far: f32,
    texels: &'a [[f32; 4]],
) -> impl Iterator<Item = (Vec3, Vec3, [f32; 4])> + 'a {
    (0..dims[2]).flat_map(move |k| {
        (0..dims[1]).flat_map(move |y| {
            (0..dims[0]).map(move |x| {
                let near = slice_depth(k as f32, far).max(camera.near);
                let back = slice_depth(k as f32 + 1.0, far);
                let i = (x + dims[0] * (y + dims[1] * k)) as usize;
                (
                    column_point(camera, dims, x, y, near),
                    column_point(camera, dims, x, y, back),
                    texels[i],
                )
            })
        })
    })
}

fn brightest(t: [f32; 4]) -> f32 {
    t[0].max(t[1]).max(t[2])
}

/// A room 10 x 10 x 4 about the eye (walls half a unit thick, their insides at |x|, |y| = 5, the floor at 0 and the ceiling at
/// 4), its ceiling left out when `open`.
fn room(open: bool) -> Vec<torque_render_scene::hd::DynamicObject> {
    let w = 0.5;
    let mut parts = vec![
        box_object(1, [5.0 + w, 0.0, 2.0], [w, 5.0 + 2.0 * w, 2.0 + 2.0 * w]),
        box_object(2, [-5.0 - w, 0.0, 2.0], [w, 5.0 + 2.0 * w, 2.0 + 2.0 * w]),
        box_object(3, [0.0, 5.0 + w, 2.0], [5.0, w, 2.0 + 2.0 * w]),
        box_object(4, [0.0, -5.0 - w, 2.0], [5.0, w, 2.0 + 2.0 * w]),
        box_object(5, [0.0, 0.0, -w], [5.0, 5.0, w]),
    ];
    if !open {
        parts.push(box_object(6, [0.0, 0.0, 4.0 + w], [5.0, 5.0, w]));
    }
    parts
}

#[test]
fn a_closed_room_holds_none_of_the_light_outside_it() {
    let Some(gpu) = gpu() else {
        return;
    };
    // A high sun over the room and a lamp beside it above the walls, its reach well into the room, in a fog; the room's
    // inside shrunk by a margin (the shadow maps' texels and bias at the walls).
    let inside = |p: Vec3| p.x.abs() < 4.5 && p.y.abs() < 4.5 && p.z > 0.5 && p.z < 3.5;
    let mut lit = [0usize; 2];
    let mut held = [0usize; 2];
    let mut sun = [0usize; 2];
    for (k, open) in [false, true].into_iter().enumerate() {
        let mut s = scene();
        s.frame.sun.towards = Vec3::new(0.25, 0.3, 0.92).normalize().to_array();
        s.frame.sun.diffuse = [1.0; 3];
        s.frame.sun.ambient = [0.2; 3];
        (s.frame.fog.start, s.frame.fog.end) = (0.0, 100.0);
        s.frame.fog_distance = 0.0;
        s.frame.objects = room(open);
        s.frame.lights = vec![HdLight {
            position: [7.0, 0.0, 7.0],
            color: [40.0, 30.0, 20.0],
            radius: 14.0,
            carrier: None,
        }];
        s.frame.lights_generation += 1;
        let d = drawn(&gpu, &mut s, [0.0, -3.5, 2.0], [0.0, 3.0, 1.8]);
        for (a, b, t) in froxels(&d.camera, d.dims, d.far, &d.texels) {
            if !(inside(a) && inside(b)) {
                continue;
            }
            held[k] += 1;
            if brightest(t) > 1e-4 {
                lit[k] += 1;
            }
        }
        sun[k] = d.march.iter().filter(|t| brightest(**t) > 1e-4).count();
    }
    assert!(held[0] > 500, "the froxels inside the room: {}", held[0]);
    assert_eq!(
        lit[0], 0,
        "froxels of the closed room lit by the lamp outside, of {}",
        held[0]
    );
    assert_eq!(sun[0], 0, "god ray texels in the closed room");
    // The same room open to the sky takes the lamp's light (over the wall) and the sun's in.
    assert!(lit[1] > 0, "{} of {} lit in the open room", lit[1], held[1]);
    assert!(sun[1] > 0, "no god rays in the open room");
}

#[test]
fn a_point_light_in_fog_adds_light_within_its_radius_only() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut s = scene();
    // Night: no sun light; a fog from the eye to 100 units.
    s.frame.sun.diffuse = [0.0; 3];
    s.frame.sun.ambient = [0.05; 3];
    (s.frame.fog.start, s.frame.fog.end) = (0.0, 100.0);
    s.frame.fog_distance = 0.0;
    let at = Vec3::new(1.0, 12.0, 2.5);
    let radius = 5.0;
    s.frame.lights = vec![HdLight {
        position: at.to_array(),
        color: [4.0, 3.0, 2.0],
        radius,
        carrier: None,
    }];
    s.frame.lights_generation += 1;
    let Drawn {
        camera,
        dims,
        far,
        texels,
        ..
    } = drawn(&gpu, &mut s, [0.0, 0.0, 2.0], [0.0, 10.0, 2.0]);
    // The light's distance to a froxel's segment (its column's ray between its slice's depths).
    let reach = |a: Vec3, b: Vec3| {
        let ab = b - a;
        let t = ((at - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
        (a + ab * t).distance(at)
    };
    let (mut within, mut outside, mut lit_outside) = (0usize, 0usize, 0usize);
    for (a, b, t) in froxels(&camera, dims, far, &texels) {
        let d = reach(a, b);
        if d < radius * 0.5 && brightest(t) > 1e-3 {
            within += 1;
        }
        if d > radius + 0.05 {
            outside += 1;
            if brightest(t) > 1e-6 {
                lit_outside += 1;
            }
        }
    }
    assert!(within > 20, "froxels lit near the light: {within}");
    assert_eq!(
        lit_outside, 0,
        "froxels past the light's radius lit, of {outside}"
    );
}

/// A fog and a lamp, so that Ultra has a grid to gather.
fn lit_scene(quality: Quality) -> PlainScene {
    let mut s = scene();
    s.mode = Some(HdMode::new(quality));
    (s.frame.fog.start, s.frame.fog.end) = (0.0, 100.0);
    s.frame.fog_distance = 0.0;
    s.frame.lights = vec![HdLight {
        position: [1.0, 8.0, 2.5],
        color: [4.0, 3.0, 2.0],
        radius: 6.0,
        carrier: None,
    }];
    s.frame.lights_generation += 1;
    s
}

/// A few frames of `scene` from the origin at `size`; whether the world then holds a gathered froxel grid.
fn gathers(
    world: &mut HdWorld<NoExtension>,
    scene: &mut PlainScene,
    size: (u32, u32),
    frames: usize,
) -> bool {
    scene.frame.size = size;
    let eye = look_at([0.0, 0.0, 2.0], [0.0, 10.0, 2.0]);
    scene.frame.eye = Some(eye);
    let camera = HdCamera::new(eye, FOV, FAR, size.0, size.1);
    for _ in 0..frames {
        scene.frame.generation += 1;
        world.upload(scene);
        world.draw(scene, &camera, None).expect("a frame");
    }
    world.read_froxels().expect("read").is_some()
}

#[test]
fn the_grid_is_gathered_at_ultra_only_and_made_again_after_a_resize() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    let mut ultra = lit_scene(Quality::Ultra);
    let mut high = lit_scene(Quality::High);
    assert!(
        world.read_froxels().expect("read").is_none(),
        "a grid before any frame"
    );
    assert!(!gathers(&mut world, &mut high, SIZE, 3), "a grid at High");
    assert!(gathers(&mut world, &mut ultra, SIZE, 3), "no grid at Ultra");
    let (dims, _) = world.read_froxels().expect("read").expect("a grid");
    assert_eq!(dims, grid_dims(SIZE.0, SIZE.1));
    assert!(
        !gathers(&mut world, &mut high, SIZE, 3),
        "a grid at High after Ultra"
    );
    assert!(
        gathers(&mut world, &mut ultra, SIZE, 3),
        "no grid back at Ultra"
    );
    let bigger = (SIZE.0 * 2, SIZE.1 * 2);
    assert!(
        gathers(&mut world, &mut ultra, bigger, 3),
        "no grid after a resize"
    );
    let (dims, sums) = world.read_froxel_sums().expect("read").expect("sums");
    assert_eq!(dims, grid_dims(bigger.0, bigger.1));
    assert_eq!(sums.len(), (dims[0] * dims[1] * dims[2]) as usize);

    world.features.froxel_fog = false;
    assert!(
        !gathers(&mut world, &mut ultra, bigger, 3),
        "a grid with the feature off"
    );
    world.features.froxel_fog = true;
    assert!(
        gathers(&mut world, &mut ultra, bigger, 3),
        "no grid with the feature on"
    );
}

#[test]
fn a_frame_without_lights_gathers_no_grid_and_keeps_the_marchs_size() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    let mut s = lit_scene(Quality::Ultra);
    assert!(gathers(&mut world, &mut s, SIZE, 3));
    let size = |w: &HdWorld<NoExtension>| {
        let (w, h, _) = w.read_volumetric().expect("read").expect("a march");
        (w, h)
    };
    let beside_grid = size(&world);
    s.frame.lights.clear();
    s.frame.lights_generation += 1;
    assert!(
        !gathers(&mut world, &mut s, SIZE, 3),
        "a grid with no lights"
    );
    assert_eq!(size(&world), beside_grid, "the march's size without lights");
}
