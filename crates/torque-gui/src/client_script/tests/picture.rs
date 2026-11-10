//! Offscreen pictures of a realm's controls: a draw list rasterized in software, so that how the controls look is compared to
//! a recorded picture and a frame drawn from the layer's kept draws is compared to one drawn from nothing.

use super::*;
use crate::draw::{GuiBlend, GuiDraws};

/// The hash pinned for the picture of [`scene`] drawn at the screen's 640 x 480 (see [`the_realm_controls_look_as_pinned`]).
const PINNED_HASH: u64 = 0x69558109d6da7353;
/// The same for [`container_scene`] (see [`the_realm_containers_look_as_pinned`]).
const PINNED_CONTAINERS_HASH: u64 = 0x14581a845abcc78d;

/// The pixels (RGBA, rows top to bottom) of `draws`: every quad in order, nearest-sampled and cut by its batch's clip, blended
/// over opaque black.
fn rasterize(draws: &GuiDraws) -> Vec<u8> {
    let (w, h) = (draws.size.0 as i32, draws.size.1 as i32);
    let mut px = vec![[0u8, 0, 0, 255]; (w * h) as usize];
    for batch in &draws.batches {
        let [cx, cy, cw, ch] = batch.clip;
        for q in &batch.quads {
            let [x0, y0, x1, y1] = q.rect;
            let left = (x0.max(cx as f32).ceil() as i32).max(0);
            let right = (x1.min((cx + cw) as f32).ceil() as i32).min(w);
            let top = (y0.max(cy as f32).ceil() as i32).max(0);
            let bottom = (y1.min((cy + ch) as f32).ceil() as i32).min(h);
            for y in top..bottom {
                for x in left..right {
                    let (fx, fy) = (
                        (x as f32 + 0.5 - x0) / (x1 - x0),
                        (y as f32 + 0.5 - y0) / (y1 - y0),
                    );
                    let mut src = q.color.map(f32::from);
                    if let Some(t) = &batch.texture {
                        let u = q.uv[0] + (q.uv[2] - q.uv[0]) * fx;
                        let v = q.uv[1] + (q.uv[3] - q.uv[1]) * fy;
                        let tx =
                            ((u * t.size.0 as f32).floor() as i32).clamp(0, t.size.0 as i32 - 1);
                        let ty =
                            ((v * t.size.1 as f32).floor() as i32).clamp(0, t.size.1 as i32 - 1);
                        let at = 4 * (ty as usize * t.size.0 as usize + tx as usize);
                        for (s, texel) in src.iter_mut().zip(&t.rgba[at..at + 4]) {
                            *s = *s * f32::from(*texel) / 255.0;
                        }
                    }
                    let dst = &mut px[(y * w + x) as usize];
                    let a = src[3] / 255.0;
                    for c in 0..3 {
                        let d = f32::from(dst[c]);
                        let blended = match batch.blend {
                            GuiBlend::Alpha => src[c] * a + d * (1.0 - a),
                            GuiBlend::Multiply => d * src[c] / 255.0 * a + d * (1.0 - a),
                        };
                        dst[c] = blended.round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
    }
    px.concat()
}

/// FNV-1a over `bytes`.
fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What a batch is drawn with: its clip, its blend and its texture by name (each cache gives its textures ids of its own).
type BatchState = ([i32; 4], GuiBlend, Option<String>);

/// What a draw list draws: each batch's state and quads.
pub(super) fn drawn(draws: &GuiDraws) -> Vec<(BatchState, Vec<crate::draw::GuiQuad>)> {
    draws
        .batches
        .iter()
        .map(|b| {
            let state = (b.clip, b.blend, b.texture.as_ref().map(|t| t.name.clone()));
            (state, b.quads.clone())
        })
        .collect()
}

fn picture(rig: &mut Rig) -> (GuiDraws, Vec<u8>) {
    let draws = draw_canvas(&rig.vm, &rig.gui, &mut rig.cache).draws;
    let pixels = rasterize(&draws);
    if let Some(dir) = std::env::var_os("TORQUE_PICTURE_DIR") {
        let mut png = Vec::new();
        {
            let mut e = ::png::Encoder::new(&mut png, draws.size.0, draws.size.1);
            e.set_color(::png::ColorType::Rgba);
            e.set_depth(::png::BitDepth::Eight);
            e.write_header()
                .and_then(|mut w| w.write_image_data(&pixels))
                .expect("the picture encodes");
        }
        std::fs::write(std::path::Path::new(&dir).join("realm-controls.png"), png)
            .expect("the picture is written");
    }
    (draws, pixels)
}

/// A realm with one of each kind of control, in states that change how they look.
fn scene() -> Rig {
    scene_in(Rig::new(&[Tier::Ui]))
}

fn scene_in(mut rig: Rig) -> Rig {
    rig.realm.set_content_check(store("mods/mod/"));
    let mut o = Ops::default();
    o.root(1, "mod");
    o.create(
        2,
        1,
        Kind::Panel,
        &[
            (Prop::Pos, vec2(10.0, 10.0)),
            (Prop::Size, vec2(200.0, 120.0)),
            (Prop::Fill, PropValue::Color([30, 60, 90, 200])),
            (Prop::Border, PropValue::Color([200, 200, 40, 255])),
        ],
    );
    o.create(
        3,
        1,
        Kind::Panel,
        &[
            (Prop::Pos, vec2(220.0, 108.0)),
            (Prop::Size, vec2(100.0, 60.0)),
            (Prop::Fill, PropValue::Color([200, 40, 60, 255])),
            (Prop::Radius, PropValue::Number(6.0)),
            (Prop::Opacity, PropValue::Number(0.5)),
        ],
    );
    o.create(
        4,
        2,
        Kind::Text,
        &[
            (Prop::Pos, vec2(8.0, 8.0)),
            (Prop::Size, vec2(120.0, 40.0)),
            (Prop::Text, text("Hello realm, a wrapped line")),
            (Prop::Wrap, PropValue::Bool(true)),
            (Prop::Color, PropValue::Color([250, 250, 250, 255])),
        ],
    );
    o.create(
        5,
        2,
        Kind::Text,
        &[
            (Prop::Pos, vec2(8.0, 60.0)),
            (Prop::Size, vec2(180.0, 20.0)),
            (Prop::Text, text("centered")),
            (Prop::Align, PropValue::Enum(1)),
        ],
    );
    o.create(
        6,
        2,
        Kind::RichText,
        &[
            (Prop::Pos, vec2(8.0, 90.0)),
            (Prop::Size, vec2(180.0, 24.0)),
            (
                Prop::RichText,
                text("a<b>bold</b> <color:ff4020>red</color>"),
            ),
        ],
    );
    o.create(
        7,
        1,
        Kind::Image,
        &[
            (Prop::Pos, vec2(330.0, 108.0)),
            (Prop::Size, vec2(40.0, 40.0)),
            (Prop::Image, text("mods/mod/a.png")),
            (Prop::Fit, PropValue::Enum(1)),
        ],
    );
    o.create(
        8,
        1,
        Kind::Button,
        &[
            (Prop::Pos, vec2(10.0, 140.0)),
            (Prop::Size, vec2(90.0, 26.0)),
            (Prop::Text, text("Go")),
        ],
    );
    o.create(
        9,
        1,
        Kind::Button,
        &[
            (Prop::Pos, vec2(110.0, 140.0)),
            (Prop::Size, vec2(90.0, 26.0)),
            (Prop::Text, text("Stop")),
            (Prop::Style, PropValue::Enum(2)),
            (Prop::Enabled, PropValue::Bool(false)),
        ],
    );
    o.create(
        10,
        1,
        Kind::Toggle,
        &[
            (Prop::Pos, vec2(10.0, 176.0)),
            (Prop::Size, vec2(120.0, 20.0)),
            (Prop::Text, text("On")),
            (Prop::On, PropValue::Bool(true)),
        ],
    );
    o.create(
        11,
        1,
        Kind::Toggle,
        &[
            (Prop::Pos, vec2(140.0, 176.0)),
            (Prop::Size, vec2(120.0, 20.0)),
            (Prop::Text, text("Off")),
        ],
    );
    o.create(
        12,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(10.0, 206.0)),
            (Prop::Size, vec2(150.0, 24.0)),
            (Prop::Text, text("typed text")),
        ],
    );
    o.create(
        13,
        1,
        Kind::TextField,
        &[
            (Prop::Pos, vec2(170.0, 206.0)),
            (Prop::Size, vec2(150.0, 24.0)),
            (Prop::Placeholder, text("hint")),
        ],
    );
    o.create(
        14,
        1,
        Kind::List,
        &[
            (Prop::Pos, vec2(10.0, 240.0)),
            (Prop::Size, vec2(120.0, 60.0)),
            (
                Prop::Rows,
                PropValue::Rows(
                    ["one", "two", "three", "four", "five", "six"]
                        .map(String::from)
                        .to_vec(),
                ),
            ),
            (Prop::Selected, PropValue::Int(2)),
        ],
    );
    o.create(
        15,
        1,
        Kind::Scroll,
        &[
            (Prop::Pos, vec2(140.0, 240.0)),
            (Prop::Size, vec2(120.0, 60.0)),
            (Prop::ContentSize, vec2(120.0, 200.0)),
        ],
    );
    o.create(
        16,
        15,
        Kind::Panel,
        &[
            (Prop::Pos, vec2(0.0, 40.0)),
            (Prop::Size, vec2(100.0, 100.0)),
            (Prop::Fill, PropValue::Color([40, 160, 80, 255])),
        ],
    );
    o.create(
        17,
        1,
        Kind::Slider,
        &[
            (Prop::Pos, vec2(10.0, 310.0)),
            (Prop::Size, vec2(160.0, 18.0)),
            (Prop::Range, vec2(0.0, 10.0)),
            (Prop::SliderValue, PropValue::Number(3.0)),
        ],
    );
    o.create(
        18,
        1,
        Kind::Progress,
        &[
            (Prop::Pos, vec2(10.0, 340.0)),
            (Prop::Size, vec2(160.0, 12.0)),
            (Prop::Fill, PropValue::Color([20, 20, 20, 255])),
            (Prop::Color, PropValue::Color([240, 180, 20, 255])),
            (Prop::Progress, PropValue::Number(0.4)),
        ],
    );
    o.create(
        19,
        2,
        Kind::Panel,
        &[
            (Prop::Pos, vec2(150.0, 100.0)),
            (Prop::Size, vec2(120.0, 60.0)),
            (Prop::Fill, PropValue::Color([255, 255, 255, 255])),
            (Prop::Tooltip, text("clipped by its parent")),
        ],
    );
    rig.apply_ok(&o);
    // The first button hovered, the focus in the first field, the list scrolled by a wheel notch.
    rig.move_to(30, 150);
    let mut focus = Ops::default();
    focus.focus(12);
    rig.click_at(30, 150);
    rig.apply_ok(&focus);
    rig
}

#[test]
fn the_realm_controls_look_as_pinned() {
    let mut rig = scene();
    let (draws, pixels) = picture(&mut rig);
    assert!(draws.report.realm_controls >= 18, "{:?}", draws.report);
    let seen = hash(&pixels);
    assert_eq!(seen, PINNED_HASH, "the picture changed: {seen:#018x}");
}

/// Containers that lay their children out: a column with a nested grid, and a grid with a hidden item.
fn container_scene() -> Rig {
    let mut rig = super::laid::laid_rig(&[Tier::Ui]);
    let item = |color: [u8; 4], w: f64, h: f64| {
        [
            (Prop::Size, vec2(w, h)),
            (Prop::Fill, PropValue::Color(color)),
            (Prop::Border, PropValue::Color([255, 255, 255, 255])),
        ]
    };
    let mut o = Ops::default();
    o.root(1, "mod")
        .create(
            2,
            1,
            Kind::Flex,
            &[
                (Prop::Pos, vec2(10.0, 110.0)),
                (Prop::Size, vec2(200.0, 300.0)),
            ],
        )
        .create(3, 2, Kind::Panel, &item([200, 40, 40, 255], 60.0, 20.0))
        .create(
            4,
            2,
            Kind::Text,
            &[
                (Prop::Size, vec2(90.0, 14.0)),
                (Prop::Text, text("in a column")),
            ],
        )
        .create(5, 2, Kind::Grid, &[(Prop::Size, vec2(140.0, 100.0))])
        .create(6, 5, Kind::Panel, &item([40, 200, 40, 255], 50.0, 30.0))
        .create(
            7,
            5,
            Kind::Button,
            &[(Prop::Size, vec2(60.0, 24.0)), (Prop::Text, text("Go"))],
        )
        .create(8, 5, Kind::Panel, &item([40, 40, 200, 255], 50.0, 20.0))
        .create(9, 5, Kind::Panel, &item([200, 200, 40, 255], 50.0, 20.0))
        .create(10, 2, Kind::Panel, &item([160, 40, 160, 255], 80.0, 16.0))
        .create(
            11,
            1,
            Kind::Grid,
            &[
                (Prop::Pos, vec2(260.0, 110.0)),
                (Prop::Size, vec2(160.0, 120.0)),
            ],
        )
        .create(12, 11, Kind::Panel, &item([90, 150, 220, 255], 40.0, 40.0))
        .create(13, 11, Kind::Panel, &item([220, 150, 90, 255], 40.0, 20.0))
        .create(14, 11, Kind::Slider, &[(Prop::Size, vec2(60.0, 16.0))]);
    rig.apply_ok(&o);
    rig
}

#[test]
fn the_realm_containers_look_as_pinned() {
    let mut rig = container_scene();
    let (draws, pixels) = picture(&mut rig);
    assert!(draws.report.realm_controls >= 10, "{:?}", draws.report);
    // The layout put the second panel under the first, not where it was made.
    assert_ne!(bounds(&rig.vm, rig.object(10)).0, (0, 0));
    let seen = hash(&pixels);
    assert_eq!(
        seen, PINNED_CONTAINERS_HASH,
        "the picture changed: {seen:#018x}"
    );
}

#[test]
fn a_frame_drawn_again_is_the_same_picture() {
    let mut rig = scene();
    let (first, pixels) = picture(&mut rig);
    for _ in 0..3 {
        let (again, more) = picture(&mut rig);
        assert_eq!(drawn(&again), drawn(&first), "the draw list");
        assert_eq!(more, pixels, "the pixels");
    }
}

/// A frame drawn by a cache that has drawn before, against one drawn by a cache that has not: the same list, whatever happened
/// to the realm in between.
#[test]
fn kept_draws_never_differ_from_drawing_from_nothing() {
    check_kept(scene());
}

/// A host with no fonts: its text is not drawn, and a kept draw says so as the first did.
#[test]
fn kept_draws_of_text_with_no_font_say_so_again() {
    let mut rig = Rig::new(&[Tier::Ui]);
    rig.gui.set_extension(Rc::new(Bare));
    rig.cache = DrawCache::for_gui(&rig.gui);
    let mut rig = scene_in(rig);
    let first = draw_canvas(&rig.vm, &rig.gui, &mut rig.cache).draws;
    assert!(first.report.no_font > 0 && !first.report.missing_fonts.is_empty());
    check_kept(rig);
}

/// A change to the scene, or to what is around it.
type Step = Box<dyn Fn(&mut Rig)>;

struct Bare;

impl crate::GuiExtension for Bare {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn layer_stamp(&self, _gui: &Gui, _now: u32) -> Option<u64> {
        Some(0)
    }
}

/// The frame drawn now, and drawn again from what it kept, are the list a cache that has drawn nothing draws.
pub(super) fn check(rig: &mut Rig, what: &str) {
    check_with(rig, what, true);
}

/// [`check`], where the second frame is drawn from kept draws only if `kept_draws` (a layer whose text a frame's font quota cut is not
/// kept).
fn check_with(rig: &mut Rig, what: &str, kept_draws: bool) {
    let kept = draw_canvas(&rig.vm, &rig.gui, &mut rig.cache).draws;
    // Fonts the engine layer loads after the server layer in this frame move the fonts' revision, so the frame after it draws
    // the layer once more and keeps that.
    let settled = draw_canvas(&rig.vm, &rig.gui, &mut rig.cache).draws;
    let replays = rig.cache.layer_replays;
    let again = draw_canvas(&rig.vm, &rig.gui, &mut rig.cache).draws;
    let mut cold = DrawCache::for_gui(&rig.gui);
    let fresh = draw_canvas(&rig.vm, &rig.gui, &mut cold).draws;
    if kept_draws {
        assert_eq!(rig.cache.layer_replays, replays + 1, "{what}: kept draws");
        assert_eq!(again.report, settled.report, "{what}: the report");
    }
    assert_eq!(drawn(&kept), drawn(&fresh), "{what}: after a change");
    assert_eq!(drawn(&settled), drawn(&fresh), "{what}: settled");
    assert_eq!(drawn(&again), drawn(&fresh), "{what}: drawn again");
}

fn check_kept(mut rig: Rig) {
    let (_, _) = picture(&mut rig);
    check(&mut rig, "as built");
    let steps: Vec<Step> = vec![
        Box::new(|r| r.move_to(160, 120)),
        Box::new(|r| r.move_to(175, 124)),
        Box::new(|r| {
            let mut o = Ops::default();
            o.set(4, &[(Prop::Text, text("changed words"))]);
            r.apply_ok(&o);
        }),
        Box::new(|r| {
            let mut o = Ops::default();
            o.set(2, &[(Prop::Pos, vec2(30.0, 14.0))]);
            r.apply_ok(&o);
        }),
        Box::new(|r| {
            let mut o = Ops::default();
            o.set(3, &[(Prop::Visible, PropValue::Bool(false))]);
            r.apply_ok(&o);
        }),
        Box::new(|r| {
            let mut o = Ops::default();
            o.order(2, true);
            r.apply_ok(&o);
        }),
        Box::new(|r| {
            let mut o = Ops::default();
            o.parent(16, 2);
            r.apply_ok(&o);
        }),
        Box::new(|r| {
            r.move_to(20, 270);
            r.wheel(false);
        }),
        Box::new(|r| {
            r.wheel(false);
        }),
        Box::new(|r| {
            r.vm.advance_time(500);
        }),
        Box::new(|r| {
            r.vm.advance_time(1000);
        }),
        Box::new(|r| r.move_to(230, 20)),
        Box::new(|r| r.move_to(160, 120)),
        Box::new(|r| {
            r.click_at(20, 260);
        }),
        Box::new(|r| {
            r.click_at(240, 20);
        }),
        Box::new(|r| {
            r.gui.set_canvas_size((800, 600));
            r.frame();
        }),
        Box::new(|r| {
            let mut o = Ops::default();
            o.destroy(2);
            r.apply_ok(&o);
        }),
    ];
    for (i, step) in steps.iter().enumerate() {
        step(&mut rig);
        check(&mut rig, &format!("step {i}"));
    }
}

impl Rig {
    /// A wheel notch over the pointer: up, or down.
    pub(super) fn wheel(&mut self, up: bool) {
        let ev = InputEvent {
            device: crate::keys::MOUSE_DEVICE,
            obj_type: crate::keys::SI_ZAXIS,
            value: if up { 1.0 } else { -1.0 },
            ..InputEvent::default()
        };
        let (_, error) = self.gui.input_event(&mut self.vm, &ev);
        assert!(error.is_none());
    }
}

/// Containers that lay their children out, changed by the realm and by the player, with frames drawn between a flush and the
/// frame that lays out what it changed.
#[test]
fn kept_draws_of_laid_out_containers_never_differ_from_drawing_from_nothing() {
    let mut rig = container_scene();
    let (_, _) = picture(&mut rig);
    check(&mut rig, "as built");
    // A flush, a frame drawn, then the frame that lays out.
    fn late(rig: &mut Rig, o: Ops) {
        rig.clock += 1000;
        rig.realm
            .apply_flush(&mut rig.vm, &rig.gui, o.bytes(), rig.clock)
            .expect("the flush applies");
        draw_canvas(&rig.vm, &rig.gui, &mut rig.cache);
        rig.frame();
    }
    let steps: Vec<Step> = vec![
        Box::new(|r| {
            let mut o = Ops::default();
            o.set(3, &[(Prop::Size, vec2(60.0, 50.0))]);
            late(r, o);
        }),
        Box::new(|r| {
            let mut o = Ops::default();
            o.set(6, &[(Prop::Size, vec2(10.0, 10.0))]);
            late(r, o);
        }),
        Box::new(|r| {
            let mut o = Ops::default();
            o.set(8, &[(Prop::Visible, PropValue::Bool(false))]);
            late(r, o);
        }),
        Box::new(|r| {
            let mut o = Ops::default();
            o.parent(12, 2);
            late(r, o);
        }),
        Box::new(|r| {
            let mut o = Ops::default();
            o.set(2, &[(Prop::Size, vec2(120.0, 300.0))]);
            late(r, o);
        }),
        Box::new(|r| {
            let mut o = Ops::default();
            o.destroy(5);
            late(r, o);
        }),
        Box::new(|r| {
            let mut o = Ops::default();
            o.set(11, &[(Prop::Pos, vec2(300.0, 130.0))]);
            r.apply_ok(&o);
        }),
        Box::new(|r| {
            r.gui.set_canvas_size((800, 600));
            r.frame();
            r.frame();
        }),
    ];
    for (i, step) in steps.iter().enumerate() {
        step(&mut rig);
        check(&mut rig, &format!("container step {i}"));
    }
}

/// What a key of the kept layer holds besides the controls: the fonts and the extension, changed in the middle of a run.
mod keys {
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

    use super::*;

    /// Fonts a test changes while the realm is on screen: whether a face is made at all, and how far apart its glyphs are.
    #[derive(Clone, Default)]
    struct Tunable {
        made: Arc<AtomicBool>,
        advance: Arc<AtomicI32>,
    }

    impl Fallback for Tunable {
        fn make(&self, _face: &str, _size: i32) -> Option<Font> {
            if !self.made.load(Ordering::Relaxed) {
                return None;
            }
            let glyph = Glyph {
                width: 5,
                height: 8,
                y_origin: 8,
                x_increment: self.advance.load(Ordering::Relaxed) as i8,
                ..Glyph::default()
            };
            Some(Font {
                height: 10,
                baseline: 8,
                ascent: 8,
                descent: 2,
                glyphs: vec![glyph; 95],
                sheets: vec![torque_images::Bitmap {
                    width: 8,
                    height: 8,
                    format: torque_images::Format::Alpha,
                    bits: vec![255; 64],
                }],
                remap: Remap::new(32, (0..95).collect()),
                units: Units::Utf16,
                tab: Tab::Spaces,
            })
        }

        fn name(&self) -> &'static str {
            "tunable"
        }
    }

    struct Host(Tunable);

    impl crate::GuiExtension for Host {
        fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
            self
        }

        fn fonts(&self) -> Option<Box<dyn crate::FontSource>> {
            Some(Box::new(FontCache::new("fonts", Box::new(self.0.clone()))))
        }

        fn layer_stamp(&self, _gui: &Gui, _now: u32) -> Option<u64> {
            Some(0)
        }
    }

    /// The scene, with fonts the test holds the controls of.
    fn tunable_scene(made: bool) -> (Rig, Tunable) {
        let fonts = Tunable::default();
        fonts.made.store(made, Ordering::Relaxed);
        fonts.advance.store(6, Ordering::Relaxed);
        let mut rig = Rig::new(&[Tier::Ui]);
        rig.gui.set_extension(Rc::new(Host(fonts.clone())));
        rig.cache = DrawCache::for_gui(&rig.gui);
        (scene_in(rig), fonts)
    }

    /// What the host does to the fonts: forgets each face (a face registered, or dropped), so the next lookup asks again.
    fn forget_faces(rig: &mut Rig) {
        let cache = rig
            .cache
            .fonts
            .as_any_mut()
            .downcast_mut::<FontCache>()
            .expect("the cache");
        for face in crate::client_script::DEFAULT_FACES {
            cache.remove_face(face);
        }
    }

    fn textured(rig: &mut Rig) -> usize {
        let draws = draw_canvas(&rig.vm, &rig.gui, &mut rig.cache).draws;
        draws
            .batches
            .iter()
            .filter(|b| {
                b.texture
                    .as_ref()
                    .is_some_and(|t| t.name.starts_with("font:"))
            })
            .map(|b| b.quads.len())
            .sum()
    }

    #[test]
    fn a_font_made_in_the_middle_of_a_run_is_drawn_at_once() {
        let (mut rig, fonts) = tunable_scene(false);
        check_with(&mut rig, "no font", false);
        assert_eq!(textured(&mut rig), 0);
        fonts.made.store(true, Ordering::Relaxed);
        forget_faces(&mut rig);
        check(&mut rig, "the faces registered");
        assert!(textured(&mut rig) > 0, "the text is drawn");
    }

    #[test]
    fn a_font_that_changes_in_the_middle_of_a_run_is_drawn_as_it_is() {
        let (mut rig, fonts) = tunable_scene(true);
        check(&mut rig, "as made");
        let narrow = draw_canvas(&rig.vm, &rig.gui, &mut rig.cache).draws;
        fonts.advance.store(9, Ordering::Relaxed);
        forget_faces(&mut rig);
        check(&mut rig, "the faces dropped");
        let wide = draw_canvas(&rig.vm, &rig.gui, &mut rig.cache).draws;
        assert_ne!(drawn(&narrow), drawn(&wide), "the glyphs moved");
    }

    #[test]
    fn a_font_refused_for_its_budget_is_drawn_once_the_budget_is_there() {
        let (mut rig, _fonts) = tunable_scene(true);
        let set_cap = |rig: &mut Rig, cap: u64| {
            rig.cache
                .fonts
                .as_any_mut()
                .downcast_mut::<FontCache>()
                .expect("the cache")
                .server_pixel_cap = cap;
        };
        set_cap(&mut rig, 0);
        let refused = textured(&mut rig);
        set_cap(&mut rig, torque_fonts::MAX_SERVER_SHEET_PIXELS);
        // Something else gives a face up: the fonts' revision moves, and the layer asks the cache again.
        rig.cache
            .fonts
            .as_any_mut()
            .downcast_mut::<FontCache>()
            .expect("the cache")
            .remove_face("another");
        check(&mut rig, "the budget back");
        assert!(
            textured(&mut rig) > refused,
            "the layer's text is drawn once the budget is there"
        );
    }

    /// An extension whose hooks change what a realm control draws: it fades every control by `opacity`, and says so in its stamp.
    struct Fading {
        opacity: Arc<AtomicU64>,
        stamp: Arc<AtomicU64>,
        says: bool,
    }

    impl crate::GuiExtension for Fading {
        fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
            self
        }

        fn opacity(&self, _vm: &Vm, _id: ObjId) -> Option<f32> {
            Some(self.opacity.load(Ordering::Relaxed) as f32 / 100.0)
        }

        fn layer_stamp(&self, _gui: &Gui, _now: u32) -> Option<u64> {
            self.says.then(|| self.stamp.load(Ordering::Relaxed))
        }
    }

    #[test]
    fn an_extension_whose_stamp_changes_has_the_layer_drawn_again() {
        let (opacity, stamp) = (Arc::new(AtomicU64::new(100)), Arc::new(AtomicU64::new(1)));
        let mut rig = Rig::new(&[Tier::Ui]);
        rig.gui.set_extension(Rc::new(Fading {
            opacity: Arc::clone(&opacity),
            stamp: Arc::clone(&stamp),
            says: true,
        }));
        rig.cache = DrawCache::for_gui(&rig.gui);
        let mut rig = scene_in(rig);
        check(&mut rig, "opaque");
        let opaque = draw_canvas(&rig.vm, &rig.gui, &mut rig.cache).draws;
        opacity.store(40, Ordering::Relaxed);
        stamp.store(2, Ordering::Relaxed);
        check(&mut rig, "faded");
        let faded = draw_canvas(&rig.vm, &rig.gui, &mut rig.cache).draws;
        assert_ne!(drawn(&opaque), drawn(&faded), "the hook shows");
        // A stamp that goes back is a draw that was kept, or one made again: the list is the same.
        opacity.store(100, Ordering::Relaxed);
        stamp.store(1, Ordering::Relaxed);
        check(&mut rig, "opaque again");
    }

    #[test]
    fn an_extension_with_no_stamp_has_the_layer_drawn_every_frame() {
        let mut rig = Rig::new(&[Tier::Ui]);
        rig.gui.set_extension(Rc::new(Fading {
            opacity: Arc::new(AtomicU64::new(70)),
            stamp: Arc::new(AtomicU64::new(0)),
            says: false,
        }));
        rig.cache = DrawCache::for_gui(&rig.gui);
        let mut rig = scene_in(rig);
        for _ in 0..4 {
            draw_canvas(&rig.vm, &rig.gui, &mut rig.cache);
        }
        assert_eq!(rig.cache.layer_replays, 0);
        assert!(!rig.cache.keeps_layer());
    }
}

#[test]
fn a_layer_that_is_not_drawn_keeps_nothing() {
    let mut rig = scene();
    let (_, _) = picture(&mut rig);
    draw_canvas(&rig.vm, &rig.gui, &mut rig.cache);
    assert!(rig.cache.keeps_layer());
    // Another screen: the layer is not drawn, and what it drew is let go with its textures.
    rig.game("Canvas.setContent(MenuGui);");
    draw_canvas(&rig.vm, &rig.gui, &mut rig.cache);
    assert!(!rig.cache.keeps_layer());
    rig.game("Canvas.setContent(PlayGui);");
    check(&mut rig, "back");
    assert!(rig.cache.keeps_layer());
}
