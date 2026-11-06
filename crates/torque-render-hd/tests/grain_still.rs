//! The film grain holds still: two frames at effect times a second apart (24 of the old 24 Hz steps) give the same pixels,
//! and the grain is there to see (the frame without it differs).

use std::sync::Arc;

use torque_render_core::Gpu;
use torque_render_hd::HdWorld;
use torque_render_hd::ext::{NoExtension, PlainScene};
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::hd::{HdCamera, HdDraw, HdMode, HdScene, Quality, material};
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Texture, Vertex};

const SIZE: (u32, u32) = (160, 90);

fn ground() -> Draw {
    let half = 200.0;
    let corner = |x: f32, y: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        color: [120, 120, 120, 255],
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

/// The frame drawn fresh (no history) at effect time `time` with grain `grain`.
fn frame(gpu: &Gpu, time: f32, grain: f32) -> Vec<u8> {
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.grain = grain;
    let mut scene = PlainScene {
        frame: HdScene::new(StaticCells::default()),
        // Low: no temporal anti-aliasing, so no jitter between the frames.
        mode: Some(HdMode::new(Quality::Low)),
        time,
    };
    let s = &mut scene.frame;
    let _ = s.textures.intern(Texture {
        path: "white".into(),
        width: 1,
        height: 1,
        levels: Arc::new(vec![vec![255; 4]]),
    });
    s.planes = vec![HdDraw::from_fixed(&ground(), material::KIND_PLANE, &[])];
    let eye = look_at([0.0, 0.0, 5.0], [0.0, 40.0, 0.0]);
    s.eye = Some(eye);
    s.fov = 75;
    s.size = SIZE;
    s.generation += 1;
    let camera = HdCamera::new(eye, 75u8, s.visible_distance, SIZE.0, SIZE.1);
    let target = gpu.offscreen(SIZE.0, SIZE.1).expect("target");
    world.upload(&scene);
    world
        .draw(
            &scene,
            &camera,
            Some((&target.color, target.color.texture().format(), [0, 0])),
        )
        .expect("draw");
    gpu.read_back(&target).expect("read back").rgba
}

#[test]
fn the_grain_is_the_same_at_every_effect_time() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            return;
        }
    };
    let early = frame(&gpu, 0.0, 0.5);
    let later = frame(&gpu, 1.0, 0.5);
    let plain = frame(&gpu, 0.0, 0.0);
    let differ = |a: &[u8], b: &[u8]| a.iter().zip(b).filter(|(x, y)| x != y).count();
    assert!(differ(&early, &plain) > early.len() / 20, "the grain shows");
    assert_eq!(
        differ(&early, &later),
        0,
        "the grain moved with the effect time"
    );
}
