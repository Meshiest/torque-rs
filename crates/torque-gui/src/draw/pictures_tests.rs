//! Offscreen pictures of the new looks, rasterized in software from the draw list: a scroll list mid-glide, rounded boxes and a
//! half-transparent panel. They are written to the folder `TORQUE_PICTURE_DIR` names (each test says what it saw either way).

use std::path::PathBuf;

use torque_ts_vm::vm::Vm;

use super::{DrawCache, GuiBlend, GuiDraws, draw_canvas};
use crate::Gui;
use crate::test_rig::{point_at, wheel};

/// The picture is drawn this many times larger, each pixel a square (nearest), so that corners can be seen.
const ZOOM: usize = 4;

/// The pixels (RGBA, rows top to bottom) of `draws`: every quad in order, nearest-sampled and cut by its batch's clip, blended over
/// `background`.
fn rasterize(draws: &GuiDraws, background: [u8; 3]) -> Vec<[u8; 4]> {
    let (w, h) = (draws.size.0 as i32, draws.size.1 as i32);
    let mut px = vec![[background[0], background[1], background[2], 255]; (w * h) as usize];
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
                    let mut src = q.color.map(f32::from);
                    if let Some(t) = &batch.texture {
                        let fx = (x as f32 + 0.5 - x0) / (x1 - x0);
                        let fy = (y as f32 + 0.5 - y0) / (y1 - y0);
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
    px
}

/// Pictures side by side, each of `size`, one gap of 8 pixels between them (in the first picture's colors' complement: plain white).
fn strip(pictures: &[Vec<[u8; 4]>], size: (usize, usize)) -> (usize, usize, Vec<u8>) {
    let gap = 8;
    let width = pictures.len() * size.0 + (pictures.len() - 1) * gap;
    let mut rows = vec![[255u8, 255, 255, 255]; width * size.1];
    for (i, picture) in pictures.iter().enumerate() {
        for y in 0..size.1 {
            let from = y * size.0;
            let to = y * width + i * (size.0 + gap);
            rows[to..to + size.0].copy_from_slice(&picture[from..from + size.0]);
        }
    }
    let mut zoomed = Vec::with_capacity(width * size.1 * ZOOM * ZOOM * 4);
    for row in rows.chunks(width) {
        let line: Vec<u8> = row
            .iter()
            .flat_map(|p| std::iter::repeat_n(*p, ZOOM))
            .flatten()
            .collect();
        for _ in 0..ZOOM {
            zoomed.extend_from_slice(&line);
        }
    }
    (width * ZOOM, size.1 * ZOOM, zoomed)
}

/// Writes the picture as `name` in `TORQUE_PICTURE_DIR` when it is set.
fn save(name: &str, (w, h, pixels): (usize, usize, Vec<u8>)) {
    let Some(dir) = std::env::var_os("TORQUE_PICTURE_DIR") else {
        return;
    };
    let path = PathBuf::from(dir).join(name);
    std::fs::create_dir_all(path.parent().unwrap()).expect("the folder is made");
    let mut png = Vec::new();
    {
        let mut e = ::png::Encoder::new(&mut png, w as u32, h as u32);
        e.set_color(::png::ColorType::Rgba);
        e.set_depth(::png::BitDepth::Eight);
        e.write_header()
            .and_then(|mut writer| writer.write_image_data(&pixels))
            .expect("the picture encodes");
    }
    std::fs::write(path, png).expect("the picture is written");
}

fn frame(vm: &Vm, gui: &Gui, background: [u8; 3]) -> Vec<[u8; 4]> {
    rasterize(
        &draw_canvas(vm, gui, &mut DrawCache::default()).draws,
        background,
    )
}

/// The fill of the light profile the pictures' content has.
const LIGHT: [u8; 4] = [235, 235, 235, 255];

/// A canvas of `size` whose content (a light fill) holds `body`.
fn light_rig(body: &str, size: (i32, i32)) -> (Vm, Gui) {
    let (mut vm, gui) = crate::test_rig::rig(body);
    vm.eval(b"new GuiControlProfile(LightProfile) { opaque = 1; fillColor = \"235 235 235 255\"; }; Top.setProfile(LightProfile);")
        .unwrap();
    gui.set_canvas_size(size);
    let top = crate::test_rig::id(&vm, "Top");
    crate::control::resize(&mut vm, &gui, top, (0, 0), size);
    (vm, gui)
}

/// A color on a hue wheel of `n` steps.
fn hue(i: usize, n: usize) -> [u8; 3] {
    let t = i as f32 / n as f32 * 6.0;
    let (k, f) = (t.floor() as usize % 6, t.fract());
    let (up, down) = ((255.0 * f) as u8, (255.0 * (1.0 - f)) as u8);
    let [r, g, b] = [
        [255, up, 0],
        [down, 255, 0],
        [0, 255, up],
        [0, down, 255],
        [up, 0, 255],
        [255, 0, down],
    ][k];
    [r / 2 + 100, g / 2 + 100, b / 2 + 100]
}

#[test]
fn a_scroll_list_mid_glide_is_drawn_between_where_it_was_and_where_it_is() {
    let mut profiles = String::new();
    let mut rows = String::new();
    for i in 0..10 {
        let [r, g, b] = hue(i, 10);
        profiles += &format!(
            "new GuiControlProfile(HueProfile{i}) {{ opaque = 1; fillColor = \"{r} {g} {b} 255\"; border = 1; borderColor = \"20 20 30 255\"; }};"
        );
        rows += &format!(
            "new GuiControl(Row{i}) {{ profile = HueProfile{i}; position = \"4 {}\"; extent = \"112 28\"; visible = 1; }};",
            4 + i * 34
        );
    }
    let body = format!(
        "new GuiScrollCtrl(Scroll) {{ profile = ScrollProfile; position = \"10 10\"; extent = \"120 120\"; \
           hScrollBar = \"alwaysOff\"; vScrollBar = \"alwaysOff\"; visible = 1; \
           new GuiControl(Page) {{ profile = PageProfile; position = \"0 0\"; extent = \"120 350\"; visible = 1; {rows} }}; }};"
    );
    let (mut vm, gui) = light_rig("", (140, 140));
    vm.eval(profiles.as_bytes()).unwrap();
    vm.eval(
        format!("Top.add(new GuiControl(Holder) {{ extent = \"140 140\"; visible = 1; {body} }});")
            .as_bytes(),
    )
    .unwrap();
    vm.eval(b"setSmoothScroll(1);").unwrap();
    point_at(&mut vm, &gui, (50, 50));
    let background = [250, 250, 250];
    let at_rest = frame(&vm, &gui, background);
    // Two notches down at once, the second a moment after the first.
    wheel(&mut vm, &gui, false);
    vm.advance_time(20);
    wheel(&mut vm, &gui, false);
    let mut pictures = vec![at_rest.clone()];
    // 0, 40, 80 and 150 milliseconds after the second notch.
    for ms in [0, 40, 40, 70] {
        vm.advance_time(ms);
        pictures.push(frame(&vm, &gui, background));
    }
    // The first one after the notches is where the content was; the last is where it is; the ones between differ from both.
    assert_eq!(pictures[1].len(), at_rest.len());
    assert_ne!(pictures[1], pictures[4]);
    assert_ne!(pictures[2], pictures[1]);
    assert_ne!(pictures[2], pictures[4]);
    assert_ne!(pictures[3], pictures[4]);
    save("scroll-glide.png", strip(&pictures, (140, 140)));
}

#[test]
fn rounded_corners_of_every_radius_are_drawn_on_fills_borders_and_buttons() {
    let mut profiles = String::new();
    let mut body = String::new();
    let mut at = 0;
    for (i, radius) in [0, 3, 6, 10, 16, 24].into_iter().enumerate() {
        profiles += &format!(
            "new GuiControlProfile(Card{i}) {{ opaque = 1; fillColor = \"70 130 200 255\"; border = 1; borderColor = \"20 40 80 255\"; cornerRadius = {radius}; }}; \
             new GuiControlProfile(Plain{i}) {{ opaque = 1; fillColor = \"230 170 60 255\"; cornerRadius = {radius}; }}; \
             new GuiControlProfile(Button{i}) {{ fillColor = \"60 160 110 255\"; fillColorHL = \"90 200 140 255\"; borderColor = \"20 80 50 255\"; cornerRadius = {radius}; }};"
        );
        body += &format!(
            "new GuiControl(Box{i}) {{ profile = Card{i}; position = \"{x} 6\"; extent = \"56 48\"; visible = 1; }}; \
             new GuiControl(Flat{i}) {{ profile = Plain{i}; position = \"{x} 60\"; extent = \"56 30\"; visible = 1; }}; \
             new GuiButtonCtrl(Btn{i}) {{ profile = Button{i}; position = \"{x} 96\"; extent = \"56 24\"; visible = 1; enabled = 1; }};",
            x = 6 + at
        );
        at += 62;
    }
    let (mut vm, gui) = light_rig("", (378, 130));
    vm.eval(profiles.as_bytes()).unwrap();
    vm.eval(
        format!("Top.add(new GuiControl(Holder) {{ extent = \"378 130\"; visible = 1; {body} }});")
            .as_bytes(),
    )
    .unwrap();
    let picture = frame(&vm, &gui, [0, 0, 0]);
    // The corner pixel of the radius-24 card is the background; the radius-0 card's is its fill.
    let corner = |card: usize| picture[6 * 378 + 6 + card * 62];
    assert_eq!(corner(0), [20, 40, 80, 255]);
    assert_eq!(corner(5), LIGHT);
    save("rounded-corners.png", strip(&[picture], (378, 130)));
}

#[test]
fn a_half_transparent_panel_shows_what_is_under_it() {
    let mut stripes = String::new();
    for i in 0..8 {
        let [r, g, b] = hue(i, 8);
        stripes += &format!(
            "new GuiControlProfile(Stripe{i}) {{ opaque = 1; fillColor = \"{r} {g} {b} 255\"; }}; \
             new GuiControl(Band{i}) {{ profile = Stripe{i}; position = \"0 {}\"; extent = \"240 20\"; visible = 1; }};",
            i * 20
        );
    }
    let panels = "new GuiControlProfile(Glass) { opaque = 1; fillColor = \"250 250 250 255\"; border = 1; borderColor = \"30 30 60 255\"; cornerRadius = 10; }; \
        new GuiControlProfile(Ink) { opaque = 1; fillColor = \"20 30 90 255\"; cornerRadius = 6; }; \
        new GuiControlProfile(Pill) { fillColor = \"230 90 60 255\"; borderColor = \"90 30 20 255\"; cornerRadius = 9; }; \
        new GuiControl(Solid) { profile = Glass; position = \"10 20\"; extent = \"100 120\"; visible = 1; \
            new GuiControl(SolidInk) { profile = Ink; position = \"10 10\"; extent = \"80 40\"; visible = 1; }; \
            new GuiButtonCtrl(SolidPill) { profile = Pill; position = \"10 70\"; extent = \"80 30\"; visible = 1; enabled = 1; }; }; \
        new GuiControl(Half) { profile = Glass; position = \"130 20\"; extent = \"100 120\"; visible = 1; opacity = 0.5; \
            new GuiControl(HalfInk) { profile = Ink; position = \"10 10\"; extent = \"80 40\"; visible = 1; }; \
            new GuiButtonCtrl(HalfPill) { profile = Pill; position = \"10 70\"; extent = \"80 30\"; visible = 1; enabled = 1; opacity = 0.5; }; };";
    let (mut vm, gui) = light_rig("", (240, 160));
    vm.eval(stripes.as_bytes()).unwrap();
    vm.eval(panels.as_bytes()).unwrap();
    vm.eval(b"Top.add(Band0); Top.add(Band1); Top.add(Band2); Top.add(Band3); Top.add(Band4); Top.add(Band5); Top.add(Band6); Top.add(Band7); Top.add(Solid); Top.add(Half);")
        .unwrap();
    let picture = frame(&vm, &gui, [0, 0, 0]);
    // A pixel of each panel's body (clear of its children): the solid one is its fill, the half one is that over a stripe.
    let at = |x: usize, y: usize| picture[y * 240 + x];
    assert_eq!(at(80, 24), [250, 250, 250, 255]);
    let (half, stripe) = (at(200, 24), at(236, 24));
    for c in 0..3 {
        let mixed = (250 + i32::from(stripe[c])) / 2;
        assert!(
            (i32::from(half[c]) - mixed).abs() <= 1,
            "{half:?} over {stripe:?}"
        );
    }
    save("half-transparent-panel.png", strip(&[picture], (240, 160)));
}
