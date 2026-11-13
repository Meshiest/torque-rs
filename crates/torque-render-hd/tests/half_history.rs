//! The temporal upscaler's history in half floats reaches the running mean the 32-bit one does: a still frame at High (a
//! busy ground under the jitter, thin poles the phases hit or miss) drawn 64 frames with each, the mean of the frames shown
//! within one level (1/255) of each other in every channel. (A single frame is not compared: on a pole a rounding apart can put the two on
//! either side of the anti-flicker's thresholds, and the one frame then shows another phase of the pole's flicker.)

use std::sync::Arc;

use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_hd::upscale::HistoryPrecision;
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::{Lcg, box_object, look_at};
use torque_render_scene::hd::post::Upscaling;
use torque_render_scene::hd::{HdCamera, HdDraw, HdMode, HdScene, Quality, material};
use torque_render_scene::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Texture, Vertex};

const SIZE: (u32, u32) = (192, 108);
const FRAMES: usize = 64;

/// A grain of value noise: every texel its own level (the clip's box as wide as on real ground).
fn grain() -> Texture {
    const N: usize = 64;
    let mut rng = Lcg::new(11);
    let rgba = (0..N * N)
        .flat_map(|_| {
            let l = 0.35 + 0.5 * rng.unit();
            [(l * 200.0) as u8, (l * 160.0) as u8, (l * 120.0) as u8, 255]
        })
        .collect();
    Texture {
        path: "half history grain".into(),
        width: N as u32,
        height: N as u32,
        levels: Arc::new(vec![rgba]),
        format: torque_render_scene::TexelFormat::Rgba8,
    }
}

fn ground() -> Draw {
    let half = 200.0;
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: [255; 4],
        uv: [x / 3.0, y / 3.0],
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
        tex_env: TexEnv::Modulate,
        texture: Some((
            0,
            Sampler {
                repeat: true,
                mag: Filter::Linear,
                min: Filter::Linear,
                mip: Filter::Linear,
                mipmaps: true,
            },
        )),
        blend: Blend::Opaque,
        cull: Cull::None,
        depth_test: true,
        depth_write: true,
        fog: true,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// The mean of the [`FRAMES`] frames shown of the still frame with the history at `precision`, per channel (0 to 255).
fn settled(gpu: &Gpu, precision: HistoryPrecision) -> Vec<f32> {
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.upscaling = Upscaling::TEMPORAL;
    world.history_precision = precision;
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        mode: Some(HdMode::new(Quality::High)),
        time: 0.0,
    };
    let s = &mut scene.frame;
    let tex = s.textures.intern(grain());
    s.planes = vec![HdDraw::from_fixed(&ground(), material::KIND_PLANE, &[tex])];
    s.objects = (0..12)
        .map(|k| {
            let x = -6.0 + k as f32;
            box_object(k + 1, [x, 30.0 + 2.0 * k as f32, 2.0], [0.04, 0.04, 2.0])
        })
        .collect();
    let eye = look_at([0.0, -4.0, 3.0], [0.0, 30.0, 1.0]);
    s.eye = Some(eye);
    s.fov = 75;
    s.size = SIZE;
    s.generation += 1;
    let camera = HdCamera::new(eye, 75u8, s.visible_distance, SIZE.0, SIZE.1);
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("a target");
    world.upload(&scene);
    let mut sum = vec![0.0f32; (SIZE.0 * SIZE.1 * 4) as usize];
    for _ in 0..FRAMES {
        world
            .draw(
                &scene,
                &camera,
                Some((&target.color, target.color.texture().format(), [0, 0])),
            )
            .expect("draw");
        let shown = gpu.read_back(&target).expect("read back").rgba;
        for (s, &c) in sum.iter_mut().zip(&shown) {
            *s += f32::from(c);
        }
    }
    let (_, _, history) = world.read_history().expect("the history");
    assert!(
        history.iter().any(|p| p[3] > 8.0),
        "some pixel flickers and holds the long history"
    );
    sum.iter().map(|s| s / FRAMES as f32).collect()
}

#[test]
fn the_half_float_history_reaches_the_32_bit_running_mean() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            return;
        }
    };
    let half = settled(&gpu, HistoryPrecision::Half);
    let full = settled(&gpu, HistoryPrecision::Full);
    let worst = half
        .iter()
        .zip(&full)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    eprintln!("half against 32-bit: the means at most {worst:.3}/255 apart");
    assert!(
        worst <= 1.0,
        "the half floats' mean is {worst}/255 off the 32-bit one"
    );
}
