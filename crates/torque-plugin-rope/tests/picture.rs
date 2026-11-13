//! A rope drawn offscreen: hung between two posts over a floor under the default day, it sags, is lit, and casts a shadow on the
//! floor. `TORQUE_ROPE_PICTURES=<folder>` also writes the pictures there as PNGs (the picture of the sag, the same without the rope,
//! the shadow's difference, and a sequence of the rope swinging).

use torque_app::renderers::RendererPlugin;
use torque_engine::{Engine, EngineBuilder};
use torque_plugin_rope::{RopeEnd, RopeParams, RopePlugin, RopeShared, RopeStyle};
use torque_render_core::Frame;

use crate::common;
use common::*;

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

/// The tube of `make` (casting), and with `strip` also a ribbon of many segments pulled tight high above it (casting no shadow).
fn make_tube_and_strip(ropes: &RopeShared, strip: bool) {
    let mut world = ropes.lock();
    world.clear();
    world
        .create(
            RopeParams {
                segments: 24,
                length: 9.0,
                thickness: 0.14,
                color: [190, 120, 60, 255],
                cast_shadow: true,
                ..RopeParams::default()
            },
            [-4.0, 0.0, 3.0],
            [4.0, 0.0, 3.0],
        )
        .unwrap();
    if strip {
        world
            .create(
                RopeParams {
                    style: RopeStyle::Ribbon,
                    segments: 64,
                    length: 8.05,
                    thickness: 0.2,
                    color: [60, 200, 90, 255],
                    cast_shadow: false,
                    ..RopeParams::default()
                },
                [-4.0, 0.0, 7.0],
                [4.0, 0.0, 7.0],
            )
            .unwrap();
    }
    for _ in 0..300 {
        world.advance(0.032);
    }
}

/// One [`RopePass`] that draws at every stage it is added at (the plugin adds a pass of its own at each).
#[derive(Clone)]
struct SharedPass(std::sync::Arc<std::sync::Mutex<torque_plugin_rope::RopePass>>);

impl torque_app::render::HdPass for SharedPass {
    fn draw(&mut self, cx: &mut torque_app::render::PassCx<'_>) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .draw(cx);
    }
}

/// [`SharedPass`] over a world of its own, at the shadows, opaque and translucent stages.
struct OnePass(RopeShared);

impl torque_engine::PluginType for OnePass {
    const ID: torque_engine::PluginId = torque_engine::PluginId::new("test.rope-one-pass");
}

impl torque_engine::Plugin for OnePass {
    fn build(&self, engine: &mut EngineBuilder) {
        use torque_app::render::{EngineRender, Stage};
        let pass = SharedPass(std::sync::Arc::new(std::sync::Mutex::new(
            torque_plugin_rope::RopePass::new(self.0.0.clone()),
        )));
        let mut render = engine.render();
        for stage in [Stage::Shadows, Stage::Opaque, Stage::Translucent] {
            let pass = pass.clone();
            render.pass(stage, move || pass.clone());
        }
    }
}

/// A frame is one submit, so its queue writes all land before its commands: one pass drawing the strips at the translucent stage
/// keeps them apart from the tubes it drew at the shadows and opaque stages. Below the ribbon the picture is the tube's alone.
#[test]
fn a_ribbon_beside_a_tube_leaves_the_tube_and_its_shadow_as_they_are() {
    let Some(gpu) = gpu() else { return };
    let ropes = RopeShared::new();
    let engine = EngineBuilder::new()
        .add_plugins((
            RendererPlugin,
            torque_app::renderers::RenderFeaturePlugins,
            OnePass(ropes.clone()),
        ))
        .build()
        .expect("the engine is built");
    let mut v = view(&gpu, &engine);
    make_tube_and_strip(&ropes, false);
    let tube = shot(&mut v, &gpu);
    make_tube_and_strip(&ropes, true);
    let both = shot(&mut v, &gpu);
    save(&both, "tube-and-strip");
    let (_, top) = project([0.0, 0.0, 5.5]);
    let below = [0, top, SIZE.0, SIZE.1 - top];
    assert!(
        changed(&tube, &both, [0, 0, SIZE.0, top], 40) > 100,
        "the ribbon is drawn above"
    );
    let differ = changed(&tube, &both, below, 8);
    assert!(differ < 20, "{differ} pixels differ from the tube alone");
    ropes.lock().clear();
}
