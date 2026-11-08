//! What the picture tests share: a floor with two posts, a view of it over an engine's renderer and what its plugins added, and
//! helpers to compare and place pixels.

// Each picture test uses some of what is here.
#![allow(dead_code)]

use std::path::PathBuf;
use std::rc::Rc;

use torque_app::render::StaticBatch;
use torque_app::renderers::{RenderFeatures, RendererEntry, RendererSlot};
use torque_app::shots::write_png;
use torque_app::view::{Daylight, WorldView};
use torque_engine::Engine;
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::math::Mat;
use torque_render_scene::source::Eye;
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

pub const SIZE: (u32, u32) = (960, 540);
/// Frames each picture settles for (the anti-aliasing's history).
pub const FRAMES: u32 = 10;

pub fn gpu() -> Option<Gpu> {
    match torque_app::window::offscreen_gpu() {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

pub fn save(frame: &Frame, name: &str) {
    if let Some(dir) = std::env::var_os("TORQUE_ROPE_PICTURES").map(PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        write_png(frame, &dir.join(format!("{name}.png"))).unwrap();
    }
}

/// A box standing on the floor: its middle at `(x, y)`, `size` across and `height` tall, in `color`.
pub fn post(x: f32, y: f32, size: f32, height: f32, color: [u8; 3]) -> StaticBatch {
    let half = [size * 0.5, size * 0.5, height * 0.5];
    let mut vertices = Vec::new();
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    let extent = |v: [f32; 3]| (0..3).map(|k| v[k].abs() * half[k]).sum::<f32>();
    for (n, u, w) in faces {
        let corner = |a: f32, b: f32| Vertex {
            position: [0, 1, 2].map(|k| {
                [x, y, half[2]][k] + n[k] * extent(n) + a * u[k] * extent(u) + b * w[k] * extent(w)
            }),
            normal: n,
            color: [color[0], color[1], color[2], 255],
            uv: [0.0; 2],
        };
        let q = [
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ];
        vertices.extend([q[0], q[2], q[1], q[0], q[3], q[2]]);
    }
    StaticBatch {
        draws: vec![Draw {
            label: "post".to_string(),
            vertices,
            lighting: true,
            tex_env: TexEnv::None,
            texture: None,
            blend: Blend::Opaque,
            cull: Cull::BackCw,
            depth_test: true,
            depth_write: true,
            fog: true,
            lights: Vec::new(),
            vertex_key: None,
        }],
        textures: Vec::new(),
        transforms: vec![Mat::IDENTITY],
    }
}

/// A view of a floor and two posts, following a slot over `engine`'s renderers and what its plugins added.
pub fn view(gpu: &Gpu, engine: &Engine) -> WorldView {
    let mut view = WorldView::new(gpu);
    view.set_plain_floor(120.0, [128, 150, 110]);
    view.set_daylight(&Daylight::default());
    view.set_effect_frame_seconds(Some(1.0 / 60.0));
    let features = engine
        .resource::<RenderFeatures>()
        .as_deref()
        .copied()
        .unwrap_or(RenderFeatures::ALL);
    let (slot, warnings) = RendererSlot::new(engine.list::<RendererEntry>(), features);
    assert!(warnings.is_empty(), "{warnings:?}");
    let adds = torque_app::render::RenderAdds::of_engine(engine).kept(|w| panic!("{w}"));
    view.attach_slot(Rc::new(slot.with_adds(adds)))
        .expect("the engine's renderer is made");
    assert_eq!(
        view.set_static_batch(1, &post(-4.0, 0.0, 0.3, 3.2, [150, 140, 130])),
        None
    );
    assert_eq!(
        view.set_static_batch(2, &post(4.0, 0.0, 0.3, 3.2, [150, 140, 130])),
        None
    );
    view
}

pub fn eye() -> Eye {
    look_at([0.0, -10.5, 3.4], [0.0, 0.0, 1.6])
}

pub fn shot(view: &mut WorldView, gpu: &Gpu) -> Frame {
    shot_from(view, gpu, eye)
}

/// The view's picture from the eye `eye` makes.
pub fn shot_from(view: &mut WorldView, gpu: &Gpu, eye: impl Fn() -> Eye) -> Frame {
    view.reset_history();
    let mut frame = None;
    for _ in 0..FRAMES {
        frame = Some(
            view.render_world(gpu, SIZE, eye())
                .expect("the world is drawn"),
        );
    }
    frame.unwrap()
}

/// The pixels of `rect` (left, top, width, height) whose largest channel differs by more than `threshold`.
pub fn changed(a: &Frame, b: &Frame, rect: [u32; 4], threshold: i32) -> usize {
    let [x0, y0, w, h] = rect;
    let mut n = 0;
    for y in y0..(y0 + h).min(a.height) {
        for x in x0..(x0 + w).min(a.width) {
            let i = 4 * (y * a.width + x) as usize;
            let d = (0..3)
                .map(|c| (i32::from(a.rgba[i + c]) - i32::from(b.rgba[i + c])).abs())
                .max()
                .unwrap();
            if d > threshold {
                n += 1;
            }
        }
    }
    n
}

/// Where the world point `p` is in the picture.
pub fn project(p: [f32; 3]) -> (u32, u32) {
    let eye = eye();
    let fov = f32::from(eye.fov.unwrap_or(90));
    let camera = torque_render_scene::hd::HdCamera::new(eye, fov, 1000.0, SIZE.0, SIZE.1);
    let m = &camera.view_proj;
    let row = |i: usize| m[i] * p[0] + m[4 + i] * p[1] + m[8 + i] * p[2] + m[12 + i];
    let w = row(3);
    assert!(w > camera.near, "the point is in front of the eye");
    (
        ((row(0) / w * 0.5 + 0.5) * SIZE.0 as f32) as u32,
        ((0.5 - row(1) / w * 0.5) * SIZE.1 as f32) as u32,
    )
}

pub const WHOLE: [u32; 4] = [0, 0, SIZE.0, SIZE.1];
