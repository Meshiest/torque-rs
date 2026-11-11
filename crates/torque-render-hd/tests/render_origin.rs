//! Camera-relative drawing through the unchanged renderer: a frame far from the world's origin, drawn in world coordinates, puts
//! its edges a pixel or so off where they are and differently each frame as the eye moves; the same frame measured from the
//! render origin near the eye ([`torque_render_scene::hd::origin`]) draws the same pixels as a frame built around the world's
//! origin. Set `TORQUE_RENDER_ORIGIN_PICTURES` to a directory to keep the pictures.

use std::path::PathBuf;

use torque_render_core::{Frame, Gpu};
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::origin::{RenderOrigin, Shift, shifted};
use torque_render_scene::hd::{HdCamera, HdDraw, material};
use torque_render_scene::source::Eye;
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (480, 270);
/// The step of an `f32` between 32,768 and 65,536 units: geometry on this grid is where it is at every origin here.
const GRID: f32 = 1.0 / 256.0;
const FRAMES: usize = 8;
/// How far a luminance must move (of 255) for a pixel to count as changed.
const CHANGED: i32 = 24;

fn on_grid(v: f32) -> f32 {
    (v / GRID).round() * GRID
}

fn quads(label: &str, gray: u8, quads: &[([[f32; 3]; 4], [f32; 3])]) -> HdDraw {
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
    let d = Draw {
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
    };
    HdDraw::from_fixed(&d, material::KIND_PLANE, &[])
}

/// The check scene about `at`: a floor, a fence of thin slats 3 m ahead (edges every 10 cm) and a box beside it. Every position
/// is a grid point plus `at`, exact at the origins tested.
fn scene_at(at: [f32; 3]) -> PlainScene {
    let p = |x: f32, y: f32, z: f32| shifted([on_grid(x), on_grid(y), on_grid(z)], at);
    let mut s = PlainScene::default();
    s.frame.sun.towards = [-0.3, -0.2, 0.93];
    s.frame.sun.diffuse = [0.9; 3];
    s.frame.sun.ambient = [0.4; 3];
    s.frame.fov = 90;
    s.frame.size = SIZE;
    s.frame.planes.push(quads(
        "floor",
        90,
        &[(
            [
                p(-20.0, -20.0, 0.0),
                p(20.0, -20.0, 0.0),
                p(20.0, 20.0, 0.0),
                p(-20.0, 20.0, 0.0),
            ],
            [0.0, 0.0, 1.0],
        )],
    ));
    let slats: Vec<_> = (0..25)
        .map(|i| {
            let x = -1.2 + 0.1 * i as f32;
            (
                [
                    p(x, 3.0, 0.0),
                    p(x + 0.02, 3.0, 0.0),
                    p(x + 0.02, 3.0, 1.0),
                    p(x, 3.0, 1.0),
                ],
                [0.0, -1.0, 0.0],
            )
        })
        .collect();
    s.frame.planes.push(quads("fence", 230, &slats));
    s.frame
        .objects
        .push(box_object(1, p(1.6, 4.0, 0.5), [0.5; 3]));
    s
}

/// The eye of frame `step` about `at`: a grid point moving one grid step a frame, looking at the fence's middle (the angles are
/// those of the positions about `at`, the same at every origin).
fn eye_at(at: [f32; 3], step: usize) -> Eye {
    let rel = [on_grid(0.5) + step as f32 * GRID, -3.0, 1.2];
    Eye {
        position: shifted(rel, at),
        ..look_at(rel, [0.0, 3.0, 0.5])
    }
}

fn frames(gpu: &Gpu, mut s: PlainScene, eye: impl Fn(usize) -> Eye) -> Vec<Frame> {
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
    let format = target.color.texture().format();
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    (0..FRAMES)
        .map(|i| {
            let eye = eye(i);
            s.frame.eye = Some(eye);
            s.frame.generation += 1;
            world.upload(&s);
            let camera = HdCamera::new(eye, 90u8, s.frame.visible_distance, SIZE.0, SIZE.1);
            world
                .draw(&s, &camera, Some((&target.color, format, [0, 0])))
                .expect("a frame");
            gpu.read_back(&target).expect("read back")
        })
        .collect()
}

fn luminance(px: &[u8; 4]) -> i32 {
    (54 * i32::from(px[0]) + 183 * i32::from(px[1]) + 19 * i32::from(px[2])) / 256
}

/// How many pixels changed between the two pictures.
fn differing(a: &Frame, b: &Frame) -> usize {
    a.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
        .filter(|(p, q)| (luminance(p) - luminance(q)).abs() > CHANGED)
        .count()
}

fn save(dir: Option<&PathBuf>, name: &str, frame: &Frame) {
    let Some(dir) = dir else { return };
    std::fs::create_dir_all(dir).expect("the pictures' directory");
    let file = std::fs::File::create(dir.join(format!("{name}.png"))).expect("a picture file");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("a header")
        .write_image_data(&frame.rgb())
        .expect("the picture");
}

/// The pixels that changed between `a` and `b`, white on black.
fn changes(a: &Frame, b: &Frame) -> Frame {
    let rgba = a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
        .flat_map(|(p, q)| {
            let v = if (luminance(p) - luminance(q)).abs() > CHANGED {
                255
            } else {
                0
            };
            [v, v, v, 255]
        })
        .collect();
    Frame {
        width: a.width,
        height: a.height,
        rgba,
    }
}

#[test]
fn a_frame_far_out_wobbles_in_world_coordinates_and_draws_as_at_the_origin_from_a_near_one() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let pictures = std::env::var_os("TORQUE_RENDER_ORIGIN_PICTURES").map(PathBuf::from);
    let small = frames(&gpu, scene_at([0.0; 3]), |i| eye_at([0.0; 3], i));
    save(pictures.as_ref(), "origin_world_0km", &small[0]);
    let mut off_by_km = Vec::new();
    for km in [1u32, 10, 50] {
        // A grid point of the origins (a multiple of 1,024) about `km` kilometers out.
        let at = [
            (f64::from(km) * 1000.0 / 1024.0).round() as f32 * 1024.0,
            0.0,
            0.0,
        ];
        let world = frames(&gpu, scene_at(at), |i| eye_at(at, i));
        let origin = RenderOrigin::near(eye_at(at, 0).position);
        assert_eq!(origin.position, at);
        let mut near = scene_at(at);
        origin.apply(&mut near.frame);
        let near = frames(&gpu, near, |i| {
            let mut e = eye_at(at, i);
            e.shift(origin.shift());
            e
        });
        let off: usize = world.iter().zip(&small).map(|(a, b)| differing(a, b)).sum();
        let kept: usize = near.iter().zip(&small).map(|(a, b)| differing(a, b)).sum();
        eprintln!(
            "{km:>3} km: world coordinates {off} pixels off over {FRAMES} frames; from the near origin {kept}"
        );
        if km == 50 {
            save(pictures.as_ref(), "origin_world_50km", &world[0]);
            save(pictures.as_ref(), "origin_near_50km", &near[0]);
            save(
                pictures.as_ref(),
                "origin_world_50km_changes",
                &changes(&world[0], &small[0]),
            );
        }
        assert_eq!(kept, 0, "{km} km from the near origin");
        off_by_km.push(off);
    }
    assert!(
        off_by_km[2] > 20 && off_by_km[2] > off_by_km[0],
        "pixels off at 1, 10, 50 km: {off_by_km:?}"
    );
    assert_eq!(gpu.device_errors().validation, 0);
}
