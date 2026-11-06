//! The fallback in a browser (feature `system-fonts` on the web, where there is no fonts folder): a face and size rasterized by
//! a canvas of the page with the browser's font of that name, its sans-serif when it has none. The glyphs are those
//! [`crate::system::SystemFonts`] makes (the ANSI charset, packed into the same sheets), measured by the canvas's text metrics.

use wasm_bindgen::JsCast as _;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::system::{GlyphImage, MAX_SIZE, pack};
use crate::{Fallback, Font};

/// The browser's fonts.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrowserFonts;

impl BrowserFonts {
    /// The browser's fonts, when the page has a document to draw them on.
    pub fn new() -> Option<BrowserFonts> {
        web_sys::window()?.document().map(|_| BrowserFonts)
    }
}

/// A canvas of `w` x `h` pixels and its 2D context, read back often.
fn canvas(w: u32, h: u32) -> Option<(HtmlCanvasElement, CanvasRenderingContext2d)> {
    let document = web_sys::window()?.document()?;
    let canvas: HtmlCanvasElement = document.create_element("canvas").ok()?.dyn_into().ok()?;
    canvas.set_width(w);
    canvas.set_height(h);
    let options = js_sys::Object::new();
    js_sys::Reflect::set(&options, &"willReadFrequently".into(), &true.into()).ok()?;
    let ctx: CanvasRenderingContext2d = canvas
        .get_context_with_context_options("2d", &options)
        .ok()??
        .dyn_into()
        .ok()?;
    Some((canvas, ctx))
}

impl Fallback for BrowserFonts {
    fn make(&self, face: &str, size: i32) -> Option<Font> {
        if !(1..=MAX_SIZE).contains(&size) {
            return None;
        }
        // A glyph is drawn inside a cell of three times the size, its origin a size in from the left and two down.
        let cell = (size as u32) * 3;
        let (_canvas, ctx) = canvas(cell, cell)?;
        let quoted = face.replace(['"', '\\', ';'], "");
        ctx.set_font(&format!("{size}px \"{quoted}\", sans-serif"));
        ctx.set_text_baseline("alphabetic");
        ctx.set_fill_style_str("#fff");
        let line = ctx.measure_text("Mg").ok()?;
        let ascent = line.font_bounding_box_ascent().ceil() as i32;
        let height = (line.font_bounding_box_ascent() + line.font_bounding_box_descent())
            .ceil()
            .max(1.0) as u32;
        let (ox, oy) = (f64::from(size), f64::from(size) * 2.0);
        Some(pack(ascent, height, |c| {
            let text = c.to_string();
            let empty = GlyphImage {
                width: 0,
                height: 0,
                coverage: Vec::new(),
                xmin: 0,
                ymin: 0,
                advance: 0.0,
            };
            let Ok(m) = ctx.measure_text(&text) else {
                return empty;
            };
            let advance = m.width() as f32;
            let left = m.actual_bounding_box_left().ceil().min(ox) as i32;
            let right = m
                .actual_bounding_box_right()
                .ceil()
                .min(f64::from(cell) - ox) as i32;
            let up = m.actual_bounding_box_ascent().ceil().min(oy) as i32;
            let down = m
                .actual_bounding_box_descent()
                .ceil()
                .min(f64::from(cell) - oy) as i32;
            let (w, h) = ((left + right).max(0), (up + down).max(0));
            if w == 0 || h == 0 || c == ' ' {
                return GlyphImage { advance, ..empty };
            }
            ctx.clear_rect(0.0, 0.0, f64::from(cell), f64::from(cell));
            if ctx.fill_text(&text, ox, oy).is_err() {
                return GlyphImage { advance, ..empty };
            }
            let Ok(image) = ctx.get_image_data(
                ox - f64::from(left),
                oy - f64::from(up),
                f64::from(w),
                f64::from(h),
            ) else {
                return GlyphImage { advance, ..empty };
            };
            let coverage = image
                .data()
                .0
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[3])
                .collect();
            GlyphImage {
                width: w as usize,
                height: h as usize,
                coverage,
                xmin: -left,
                ymin: up - h,
                advance,
            }
        }))
    }

    fn name(&self) -> &'static str {
        "the browser's fonts (canvas)"
    }
}
