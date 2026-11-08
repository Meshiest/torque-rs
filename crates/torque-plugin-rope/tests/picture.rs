//! A rope drawn offscreen: hung between two posts over a floor under the default day, it sags, is lit, and casts a shadow on the
//! floor. `TORQUE_ROPE_PICTURES=<folder>` also writes the pictures there as PNGs (the picture of the sag, the same without the rope,
//! the shadow's difference, and a sequence of the rope swinging).

use std::path::PathBuf;
use std::rc::Rc;

use torque_app::render::StaticBatch;
use torque_app::renderers::{RenderFeatures, RendererEntry, RendererPlugin, RendererSlot};
use torque_app::shots::write_png;
use torque_app::view::{Daylight, WorldView};
use torque_engine::{Engine, EngineBuilder};
use torque_plugin_rope::{RopeEnd, RopeParams, RopePlugin, RopeShared, RopeStyle};
use torque_render_core::{Frame, Gpu};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::math::Mat;
use torque_render_scene::source::Eye;
use torque_render_scene::{Blend, Cull, Draw, TexEnv, Vertex};

const SIZE: (u32, u32) = (960, 540);
/// Frames each picture settles for (the anti-aliasing's history).
const FRAMES: u32 = 10;

fn gpu() -> Option<Gpu> {
    match torque_app::window::offscreen_gpu() {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn save(frame: &Frame, name: &str) {
    if let Some(dir) = std::env::var_os("TORQUE_ROPE_PICTURES").map(PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        write_png(frame, &dir.join(format!("{name}.png"))).unwrap();
    }
}

fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins((
            RendererPlugin,
            torque_app::renderers::RenderFeaturePlugins,
            RopePlugin::default(),
        ))
        .build()
        .expect("the engine is built")
}

/// A box standing on the floor: its middle at `(x, y)`, `size` across and `height` tall, in `color`.
fn post(x: f32, y: f32, size: f32, height: f32, color: [u8; 3]) -> StaticBatch {
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
fn view(gpu: &Gpu, engine: &Engine) -> WorldView {
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

fn eye() -> Eye {
    look_at([0.0, -10.5, 3.4], [0.0, 0.0, 1.6])
}

fn shot(view: &mut WorldView, gpu: &Gpu) -> Frame {
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
fn changed(a: &Frame, b: &Frame, rect: [u32; 4], threshold: i32) -> usize {
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
fn project(p: [f32; 3]) -> (u32, u32) {
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

fn make(ropes: &RopeShared, cast_shadow: bool) -> u32 {
    let mut world = ropes.lock();
    world.clear();
    let id = world
        .create(
            RopeParams {
                segments: 24,
                length: 9.0,
                thickness: 0.14,
                color: [190, 120, 60, 255],
                cast_shadow,
                ..RopeParams::default()
            },
            [-4.0, 0.0, 3.0],
            [4.0, 0.0, 3.0],
        )
        .unwrap();
    for _ in 0..300 {
        world.advance(0.032);
    }
    id
}

const WHOLE: [u32; 4] = [0, 0, SIZE.0, SIZE.1];

#[test]
fn a_rope_hangs_between_two_posts_lit_and_casting_a_shadow() {
    let Some(gpu) = gpu() else { return };
    let engine = engine();
    let ropes = engine.resource::<RopeShared>().unwrap().clone();
    let mut v = view(&gpu, &engine);
    let plain = shot(&mut v, &gpu);

    let id = make(&ropes, true);
    let drawn = shot(&mut v, &gpu);
    save(&plain, "posts");
    save(&drawn, "rope");

    // The rope is on the picture where it sags, in its own colors, and the two pictures differ only along it.
    let lowest = {
        let world = ropes.lock();
        let points: Vec<[f32; 3]> = world.rope(id).unwrap().points().collect();
        *points.iter().min_by(|a, b| a[2].total_cmp(&b[2])).unwrap()
    };
    assert!(lowest[2] < 1.5, "it sags: {lowest:?}");
    let (cx, cy) = project(lowest);
    assert!(
        changed(&plain, &drawn, [cx - 6, cy - 6, 12, 12], 40) > 20,
        "the rope's sag is drawn at {cx},{cy}"
    );
    let i = 4 * (cy * SIZE.0 + cx) as usize;
    let p = &drawn.rgba[i..i + 3];
    assert!(p[0] > p[2] + 40, "it is the rope's brown, lit: {p:?}");
    // Above the posts nothing changed.
    let (_, top) = project([0.0, 0.0, 3.6]);
    assert_eq!(
        changed(&plain, &drawn, [0, 0, SIZE.0, top.saturating_sub(4)], 8),
        0
    );

    // The same rope not casting a shadow: the floor under it is lighter.
    make(&ropes, false);
    let unshadowed = shot(&mut v, &gpu);
    save(&unshadowed, "rope-no-shadow");
    let floor = [
        0,
        project([0.0, 0.0, 0.0]).1.saturating_sub(120),
        SIZE.0,
        260,
    ];
    let shadow = changed(&drawn, &unshadowed, floor, 12);
    assert!(shadow > 150, "{shadow} floor pixels darker for the shadow");
    // A rope that casts no shadow costs the shadow pass nothing: it still draws.
    assert!(changed(&plain, &unshadowed, [cx - 6, cy - 6, 12, 12], 40) > 20);
    // Everything off the floor strip is the same with or without the shadow.
    let (_, below) = project([0.0, -3.0, 0.0]);
    assert!(
        changed(
            &drawn,
            &unshadowed,
            [0, below + 60, SIZE.0, SIZE.1 - below - 60],
            8
        ) < 20
    );
    ropes.lock().clear();
}

/// The rope pushed to the side and let swing: a short sequence, each picture a quarter second on.
#[test]
fn a_swinging_rope_moves_between_pictures() {
    let Some(gpu) = gpu() else { return };
    let engine = engine();
    let ropes = engine.resource::<RopeShared>().unwrap().clone();
    let mut v = view(&gpu, &engine);
    let id = make(&ropes, true);
    // The right end lets go and the rope is pushed along: it falls and swings from the left post.
    {
        let mut world = ropes.lock();
        world.release(id, RopeEnd::End).unwrap();
        world.impulse(id, [0.0, 6.0, 0.0]).unwrap();
    }
    let mut frames = Vec::new();
    for n in 0..8 {
        for _ in 0..6 {
            ropes.lock().advance(0.032);
        }
        let frame = shot(&mut v, &gpu);
        save(&frame, &format!("swing-{n}"));
        frames.push(frame);
    }
    for pair in frames.windows(2) {
        assert!(
            changed(&pair[0], &pair[1], WHOLE, 30) > 100,
            "the rope moved between pictures"
        );
    }
    ropes.lock().clear();
}

/// Both ends held and the rope thrown up: it leaps, falls and bounces on its ends.
#[test]
fn a_rope_thrown_up_bounces_on_its_ends() {
    let Some(gpu) = gpu() else { return };
    let engine = engine();
    let ropes = engine.resource::<RopeShared>().unwrap().clone();
    let mut v = view(&gpu, &engine);
    let id = make(&ropes, true);
    ropes.lock().impulse(id, [0.0, 0.0, 12.0]).unwrap();
    let mut frames = Vec::new();
    for n in 0..6 {
        for _ in 0..4 {
            ropes.lock().advance(0.032);
        }
        let frame = shot(&mut v, &gpu);
        save(&frame, &format!("bounce-{n}"));
        frames.push(frame);
    }
    for pair in frames.windows(2) {
        assert!(
            changed(&pair[0], &pair[1], WHOLE, 30) > 100,
            "the rope moved between pictures"
        );
    }
    ropes.lock().clear();
}

/// A ribbon (green) hung between the posts and a line (a red laser) straight between them, `y` apart, with a third post
/// standing in front of both.
fn make_strips(ropes: &RopeShared) -> (u32, u32) {
    let mut world = ropes.lock();
    world.clear();
    let ribbon = world
        .create(
            RopeParams {
                style: RopeStyle::Ribbon,
                segments: 8,
                length: 9.0,
                thickness: 0.2,
                color: [60, 200, 90, 255],
                ..RopeParams::default()
            },
            [-4.0, 0.0, 3.0],
            [4.0, 0.0, 3.0],
        )
        .unwrap();
    let line = world
        .create(
            RopeParams {
                style: RopeStyle::Line,
                thickness: 0.08,
                color: [255, 20, 20, 255],
                emissive: true,
                ..RopeParams::default()
            },
            [-4.0, 0.0, 0.8],
            [4.0, 0.0, 0.8],
        )
        .unwrap();
    for _ in 0..300 {
        world.advance(0.032);
    }
    (ribbon, line)
}

/// The pixel of `frame` at `(x, y)`.
fn pixel(frame: &Frame, (x, y): (u32, u32)) -> [u8; 3] {
    let i = 4 * (y * frame.width + x) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

#[test]
fn a_ribbon_and_a_line_are_drawn_and_hidden_by_what_is_in_front() {
    let Some(gpu) = gpu() else { return };
    let engine = engine();
    let ropes = engine.resource::<RopeShared>().unwrap().clone();
    let mut v = view(&gpu, &engine);
    let plain = shot(&mut v, &gpu);
    let (ribbon, _) = make_strips(&ropes);
    let drawn = shot(&mut v, &gpu);
    save(&drawn, "strips");

    let lowest = {
        let world = ropes.lock();
        let points: Vec<[f32; 3]> = world.rope(ribbon).unwrap().points().collect();
        *points.iter().min_by(|a, b| a[2].total_cmp(&b[2])).unwrap()
    };
    // The ribbon is green where it sags and the laser red along its straight run.
    let sag = project(lowest);
    let g = pixel(&drawn, sag);
    assert!(
        g[1] > g[0] + 40 && g[1] > g[2] + 40,
        "the ribbon is green: {g:?}"
    );
    let laser = project([0.0, 0.0, 0.8]);
    let r = pixel(&drawn, laser);
    assert!(r[0] > 150 && r[0] > r[1] + 80, "the laser is red: {r:?}");
    // Above the posts nothing changed (and a strip casts no shadow: the floor is as it was but under the strips' own pixels).
    let (_, top) = project([0.0, 0.0, 3.6]);
    assert_eq!(
        changed(&plain, &drawn, [0, 0, SIZE.0, top.saturating_sub(4)], 8),
        0
    );
    let (_, floor) = project([0.0, 0.0, 0.0]);
    let (_, below) = project([0.0, -3.0, 0.0]);
    assert!(
        changed(
            &plain,
            &drawn,
            [0, below + 60, SIZE.0, SIZE.1 - below - 60],
            8
        ) < 20,
        "no shadow on the floor in front"
    );
    assert!(floor > 0);

    // A post in front of both: where it covers them the picture is the post's, with the strips or without; beside it they show.
    assert_eq!(
        v.set_static_batch(3, &post(0.0, -3.0, 0.7, 3.2, [140, 90, 160])),
        None
    );
    ropes.lock().clear();
    let without = shot(&mut v, &gpu);
    make_strips(&ropes);
    let with = shot(&mut v, &gpu);
    save(&without, "strips-post");
    save(&with, "strips-post-crossed");
    for at in [sag, laser] {
        assert_eq!(
            changed(&without, &with, [at.0 - 3, at.1 - 3, 6, 6], 6),
            0,
            "the post hides the strip at {at:?}"
        );
    }
    assert!(
        changed(&without, &with, WHOLE, 40) > 200,
        "the strips show beside the post"
    );
    ropes.lock().clear();
}

/// A ribbon and a line hung over the floor, casting a shadow or not.
fn make_casters(ropes: &RopeShared, cast: bool) {
    let mut world = ropes.lock();
    world.clear();
    for (style, y) in [(RopeStyle::Ribbon, -1.0), (RopeStyle::Line, 1.0)] {
        world
            .create(
                RopeParams {
                    style,
                    segments: 6,
                    thickness: 0.3,
                    color: [60, 200, 90, 255],
                    cast_shadow: cast,
                    receive_shadow: false,
                    ..RopeParams::default()
                },
                [-3.0, y, 1.6],
                [3.0, y, 1.6],
            )
            .unwrap();
    }
    for _ in 0..200 {
        world.advance(0.032);
    }
}

#[test]
fn a_ribbon_and_a_line_that_cast_darken_the_floor_under_them_and_the_shadow_stays_put() {
    let Some(gpu) = gpu() else { return };
    let engine = engine();
    let ropes = engine.resource::<RopeShared>().unwrap().clone();
    let mut v = view(&gpu, &engine);
    make_casters(&ropes, false);
    let plain = shot(&mut v, &gpu);
    make_casters(&ropes, true);
    let cast = shot(&mut v, &gpu);
    save(&cast, "strip-shadows");
    // The floor between the posts is darker with the shadows than without, and nothing above the strips changed.
    let (_, floor) = project([0.0, 0.0, 0.0]);
    let band = [0, floor.saturating_sub(120), SIZE.0, 240];
    assert!(
        changed(&plain, &cast, band, 12) > 500,
        "the shadows are on the floor"
    );
    // A second camera: the shadow band is at the same place on the ground, so the pictures still differ along the same floor line.
    let mut other = eye();
    other.position[0] += 2.0;
    other.position[1] -= 1.0;
    v.reset_history();
    let mut moved = None;
    let mut moved_plain = None;
    for (cast, slot) in [(false, &mut moved_plain), (true, &mut moved)] {
        make_casters(&ropes, cast);
        v.reset_history();
        for _ in 0..FRAMES {
            *slot = Some(
                v.render_world(&gpu, SIZE, other)
                    .expect("the world is drawn"),
            );
        }
    }
    let (moved, moved_plain) = (moved.unwrap(), moved_plain.unwrap());
    save(&moved, "strip-shadows-moved");
    assert!(
        changed(&moved_plain, &moved, band, 12) > 500,
        "and from elsewhere"
    );
    ropes.lock().clear();
}

/// A tube and a ribbon hung between the posts one behind the other, each with `textures` (the tube's, the ribbon's: 0 for none).
fn make_textured(ropes: &RopeShared, textures: (u32, u32)) {
    let mut world = ropes.lock();
    world.clear();
    for (style, texture, y, color) in [
        (RopeStyle::Tube, textures.0, 0.0, [255, 255, 255, 255]),
        (RopeStyle::Ribbon, textures.1, 1.0, [255, 255, 255, 255]),
    ] {
        world
            .create(
                RopeParams {
                    style,
                    segments: 12,
                    length: 8.6,
                    thickness: 0.25,
                    color,
                    texture,
                    texture_length: 0.5,
                    texture_nearest: true,
                    cast_shadow: false,
                    ..RopeParams::default()
                },
                [-4.0, y, 3.0],
                [4.0, y, 3.0],
            )
            .unwrap();
    }
    for _ in 0..300 {
        world.advance(0.032);
    }
}

/// How many pixels of `frame` in `rect` are mostly red, and how many mostly blue.
fn red_and_blue(frame: &Frame, rect: [u32; 4]) -> (usize, usize) {
    let [x0, y0, w, h] = rect;
    let (mut red, mut blue) = (0, 0);
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let p = pixel(frame, (x, y)).map(i32::from);
            if p[0] > p[2] + 60 && p[0] > p[1] + 60 {
                red += 1;
            }
            if p[2] > p[0] + 60 && p[2] > p[1] + 20 {
                blue += 1;
            }
        }
    }
    (red, blue)
}

#[test]
fn a_textured_tube_shows_the_texture_and_a_ribbon_blends_by_its_alpha() {
    use std::sync::Arc;
    use torque_plugin_rope::ImageData;
    let Some(gpu) = gpu() else { return };
    let engine = engine();
    let ropes = engine.resource::<RopeShared>().unwrap().clone();
    let mut v = view(&gpu, &engine);
    let plain = shot(&mut v, &gpu);
    // Red then blue along the rope for the tube; opaque green then a hole for the ribbon.
    let (stripes, holes) = {
        let mut world = ropes.lock();
        let a = ImageData::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        let b = ImageData::from_rgba(2, 1, vec![60, 220, 90, 255, 0, 0, 0, 0]).unwrap();
        (
            world.add_texture("stripes.png", true, Arc::new(a)).unwrap(),
            world.add_texture("holes.png", true, Arc::new(b)).unwrap(),
        )
    };
    make_textured(&ropes, (0, 0));
    let untextured = shot(&mut v, &gpu);
    make_textured(&ropes, (stripes, holes));
    let textured = shot(&mut v, &gpu);
    save(&textured, "textured");
    let (lowest, _) = {
        let world = ropes.lock();
        let rope = world.ropes().next().unwrap().1;
        (
            rope.drawn_points()
                .iter()
                .map(|p| p.z)
                .fold(f32::MAX, f32::min),
            0,
        )
    };
    let sag = project([0.0, 0.0, lowest]);
    // The tube's sag, a strip of pixels along it: both colors of the texture, and the plain white tube has neither.
    let rect = [sag.0 - 150, sag.1 - 10, 300, 20];
    let (red, blue) = red_and_blue(&textured, rect);
    assert!(red > 40 && blue > 40, "red {red}, blue {blue}");
    let (red0, blue0) = red_and_blue(&untextured, rect);
    assert!(red0 < 10 && blue0 < 10, "{red0} {blue0}");
    // The ribbon behind it: where the texture has holes the floor shows, so fewer pixels differ from the empty scene than for the
    // opaque ribbon.
    let strip_rect = [0, 0, SIZE.0, project([0.0, 1.0, 1.0]).1 + 80];
    let solid = changed(&plain, &untextured, strip_rect, 20);
    let holed = changed(&plain, &textured, strip_rect, 20);
    assert!(holed < solid, "{holed} against {solid}");
    ropes.lock().clear();
}
