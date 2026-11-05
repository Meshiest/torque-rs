//! The sun disk against the sky about it: the camera aimed at the sun over a plain ground under the physical sky, from high noon to
//! just over the horizon. At every elevation the disk is the brightest thing in the sky on screen (the tonemap's path to white:
//! a saturated disk far past white no longer shows darker than the paler sky about it), it reddens and dims toward the horizon,
//! nothing drawn is non-finite, and the TAA holds it steady over the frames.

use std::sync::Arc;

use torque_render_core::{Frame, Gpu};
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::{HdWorld, non_finite};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::scatter::{ScatterLook, ScatterTable};
use torque_render_scene::hd::{HdCamera, HdDraw, HdMode, HdScene, Quality, material};
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Texture, Vertex};
use torque_scene::time_of_day::{DayFields, TimeOfDay};

const SIZE: (u32, u32) = (320, 180);
/// A narrow view (degrees): the disk is about 9 pixels across.
const FOV: u8 = 12;
const FRAMES: usize = 24;
/// The frames over which the disk must hold steady (after the history has filled).
const STEADY_FROM: usize = 10;

fn ground() -> Draw {
    let half = 600.0;
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: [90, 120, 60, 255],
        uv: [0.0; 2],
    };
    let (a, b, c, d) = (
        corner(-half, -half),
        corner(half, -half),
        corner(half, half),
        corner(-half, half),
    );
    Draw {
        label: "ground".into(),
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

fn scene() -> PlainScene {
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(HdMode::new(Quality::High)),
        time: 0.0,
    };
    let s = &mut scene.frame;
    s.atmosphere = true;
    // (An unused texture keeps the table's first slot as the examples have it.)
    let _ = s.textures.intern(Texture {
        path: "white".into(),
        width: 1,
        height: 1,
        levels: Arc::new(vec![vec![255; 4]]),
    });
    s.planes = vec![HdDraw::from_fixed(&ground(), material::KIND_PLANE, &[])];
    s.visible_distance = 800.0;
    s.fog_distance = 300.0;
    s.fog.start = 300.0;
    s.fog.end = 800.0;
    scene
}

fn srgb_to_linear(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// The pixels within `r` of the center, and those between `r1` and `r2`.
fn disk_and_ring<T: Copy>(w: u32, h: u32, px: &[T], r: f32, r1: f32, r2: f32) -> (Vec<T>, Vec<T>) {
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let (mut disk, mut ring) = (Vec::new(), Vec::new());
    for y in 0..h {
        for x in 0..w {
            let d = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy);
            let p = px[(y * w + x) as usize];
            if d <= r {
                disk.push(p);
            } else if (r1..=r2).contains(&d) {
                ring.push(p);
            }
        }
    }
    (disk, ring)
}

fn mean(v: &[[f32; 3]]) -> [f32; 3] {
    let n = v.len().max(1) as f32;
    [0, 1, 2].map(|k| v.iter().map(|p| p[k]).sum::<f32>() / n)
}

/// One elevation's measures.
struct Look {
    elevation: f32,
    /// The disk's and the ring's mean display color (linear).
    disk: [f32; 3],
    ring: [f32; 3],
    /// The disk's and the ring's mean HDR luminance before the anti-aliasing.
    disk_hdr: f32,
    ring_hdr: f32,
    /// The disk's mean luminance after the TAA at each frame.
    taa: Vec<f32>,
    non_finite: usize,
}

fn look(gpu: &Gpu, world: &mut HdWorld<NoExtension>, scene: &mut PlainScene, time: f32) -> Look {
    let mut day = TimeOfDay::new(DayFields {
        azimuth_override: 200f32.to_radians(),
        play: false,
        ..DayFields::default()
    });
    day.set_time(time);
    let sun = day.towards();
    let at = [0.0, 0.0, 3.0];
    let eye = look_at(at, [0, 1, 2].map(|k| at[k] + sun[k] * 100.0));
    let camera = HdCamera::new(eye, FOV, scene.frame.visible_distance, SIZE.0, SIZE.1);
    let light = ScatterTable::earth().light(sun, &ScatterLook::default());
    let s = &mut scene.frame;
    s.sun = light.sun();
    s.fog = light.fog(s.fog);
    s.clear = light.fog;
    s.atmosphere_sun = Some(sun);
    s.clouds = None;
    s.lights_generation += 1;
    s.eye = Some(eye);
    s.fov = FOV;
    s.size = SIZE;
    world.reset_history();
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("target");
    // The disk's radius in pixels (the settings' 0.0093 radians over the vertical view).
    let r = 0.0093 / (f32::from(FOV).to_radians() / SIZE.1 as f32);
    let (inner, r1, r2) = (0.5 * r, 2.5 * r, 4.0 * r);
    let mut taa = Vec::new();
    let mut non_finite_px = 0;
    let mut frame: Option<Frame> = None;
    for _ in 0..FRAMES {
        scene.frame.generation += 1;
        scene.time += 1.0 / 30.0;
        world.upload(scene);
        world
            .draw(
                scene,
                &camera,
                Some((&target.color, target.color.texture().format(), [0, 0])),
            )
            .expect("draw");
        let (w, h, out) = world.read_output().expect("output");
        non_finite_px += non_finite(&out);
        let (d, _) = disk_and_ring(w, h, &out, inner, r1, r2);
        let d: Vec<[f32; 3]> = d.iter().map(|p| [p[0], p[1], p[2]]).collect();
        taa.push(luminance(mean(&d)));
        frame = Some(gpu.read_back(&target).expect("read back"));
    }
    let (w, h, hdr) = world.read_hdr().expect("hdr");
    non_finite_px += non_finite(&hdr);
    let hdr: Vec<[f32; 3]> = hdr.iter().map(|p| [p[0], p[1], p[2]]).collect();
    let (dh, rh) = disk_and_ring(w, h, &hdr, inner, r1, r2);
    let frame = frame.expect("a frame");
    let px: Vec<[f32; 3]> = frame
        .rgba
        .chunks_exact(4)
        .map(|c| [c[0], c[1], c[2]].map(srgb_to_linear))
        .collect();
    let (d, ring) = disk_and_ring(frame.width, frame.height, &px, inner, r1, r2);
    Look {
        elevation: sun[2].asin().to_degrees(),
        disk: mean(&d),
        ring: mean(&ring),
        disk_hdr: luminance(mean(&dh)),
        ring_hdr: luminance(mean(&rh)),
        taa,
        non_finite: non_finite_px,
    }
}

#[test]
fn the_sun_disk_is_the_brightest_thing_in_the_sky_at_every_elevation() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            return;
        }
    };
    let mut world: HdWorld<NoExtension> = HdWorld::new(&gpu);
    let mut scene = scene();
    // From high in the sky to just over the horizon (the day's 0.25 is noon, 0.5 sunset).
    let looks: Vec<Look> = [0.35, 0.44, 0.48, 0.4945, 0.498]
        .into_iter()
        .map(|t| look(&gpu, &mut world, &mut scene, t))
        .collect();
    for l in &looks {
        let el = l.elevation;
        assert_eq!(l.non_finite, 0, "{el:.1} deg: non-finite pixels");
        assert!(
            l.disk_hdr > 4.0 * l.ring_hdr,
            "{el:.1} deg: the disk's light {} is not well over the sky's {}",
            l.disk_hdr,
            l.ring_hdr
        );
        let (dl, rl) = (luminance(l.disk), luminance(l.ring));
        assert!(
            dl > rl * 1.15 + 0.02,
            "{el:.1} deg: the disk shows {dl:.3} ({:?}), the sky about it {rl:.3} ({:?})",
            l.disk,
            l.ring
        );
        let steady = &l.taa[STEADY_FROM..];
        let lo = steady.iter().copied().fold(f32::MAX, f32::min);
        let hi = steady.iter().copied().fold(0.0, f32::max);
        assert!(
            hi - lo <= 0.03 * hi,
            "{el:.1} deg: the disk flickers through the TAA: {steady:?}"
        );
        assert!(
            lo > 0.5 * l.disk_hdr,
            "{el:.1} deg: the TAA darkens the disk: {lo} of {}",
            l.disk_hdr
        );
    }
    // Toward the horizon the disk reddens and dims.
    let (high, low) = (&looks[0], &looks[looks.len() - 1]);
    assert!(
        low.disk_hdr < 0.5 * high.disk_hdr,
        "the disk at {:.1} deg ({}) is not dimmer than at {:.1} deg ({})",
        low.elevation,
        low.disk_hdr,
        high.elevation,
        high.disk_hdr
    );
    let green = |c: [f32; 3]| c[1] / c[0].max(1e-6);
    assert!(
        green(low.disk) < 0.8 * green(high.disk),
        "the disk at {:.1} deg ({:?}) is not redder than at {:.1} deg ({:?})",
        low.elevation,
        low.disk,
        high.elevation,
        high.disk
    );
}
