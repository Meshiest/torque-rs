//! The fallback in a browser (feature `system-fonts` on the web, where there is no fonts folder): a face and size rasterized by
//! a canvas of the page with the browser's font of that name, its sans-serif when it has none. The glyphs are those
//! [`crate::system::SystemFonts`] makes (the ANSI charset, packed into the same sheets), measured by the canvas's text metrics.

use wasm_bindgen::JsCast as _;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::system::{GlyphImage, MAX_SIZE, pack};
use crate::ttf::cp1252;
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

/// The widest a sheet of glyphs drawn at once is (a glyph wider than it gets a row of its own width), and the tallest a sheet
/// grows before the next one starts.
const DRAW_WIDTH: u32 = 1024;
const DRAW_HEIGHT: u32 = 2048;

/// A glyph as the canvas measured it: its advance, its ink's box around its origin (`left` and `up` of it, `w` x `h`), and
/// where on which sheet it is drawn (none: no ink).
struct Measured {
    text: String,
    advance: f32,
    left: i32,
    up: i32,
    w: u32,
    h: u32,
    at: Option<(usize, u32, u32)>,
}

impl Measured {
    /// The glyph's image from its box's alpha (`coverage`, `w` x `h`; empty: no ink).
    fn image(&self, coverage: Vec<u8>) -> GlyphImage {
        if coverage.is_empty() {
            return GlyphImage {
                width: 0,
                height: 0,
                coverage,
                xmin: 0,
                ymin: 0,
                advance: self.advance,
            };
        }
        GlyphImage {
            width: self.w as usize,
            height: self.h as usize,
            coverage,
            xmin: -self.left,
            ymin: self.up - self.h as i32,
            advance: self.advance,
        }
    }
}

/// The context set to draw `font`'s glyphs: white, on the alphabetic baseline.
fn set_face(ctx: &CanvasRenderingContext2d, font: &str) {
    ctx.set_font(font);
    ctx.set_text_baseline("alphabetic");
    ctx.set_fill_style_str("#fff");
}

impl Fallback for BrowserFonts {
    /// Every glyph is measured first; then the glyphs with ink are drawn side by side on a sheet or two, each clipped to its own
    /// box, and each sheet is read back once. A glyph's pixels are those it draws alone at its origin. Glyph by glyph (measured,
    /// drawn and read back in turn) the arena menu's five faces took 270 ms in Chrome, all in one frame; this way they take
    /// about 115 ms.
    fn make(&self, face: &str, size: i32) -> Option<Font> {
        if !(1..=MAX_SIZE).contains(&size) {
            return None;
        }
        // A glyph's box is kept inside a cell of three times the size, its origin a size in from the left and two down.
        let cell = (size as u32) * 3;
        let quoted = face.replace(['"', '\\', ';'], "");
        let font = format!("{size}px \"{quoted}\", sans-serif");
        let (_canvas, ctx) = canvas(1, 1)?;
        set_face(&ctx, &font);
        let line = ctx.measure_text("Mg").ok()?;
        let ascent = line.font_bounding_box_ascent().ceil() as i32;
        let height = (line.font_bounding_box_ascent() + line.font_bounding_box_descent())
            .ceil()
            .max(1.0) as u32;
        let (ox, oy) = (f64::from(size), f64::from(size) * 2.0);
        // The glyphs measured, their boxes laid out in rows on sheets (a pixel apart); each sheet's size.
        let mut sheets: Vec<(u32, u32)> = Vec::new();
        let (mut x, mut y, mut row_h) = (0u32, 0u32, 0u32);
        let glyphs: Vec<Measured> = (32u8..=255)
            .map(|b| {
                let c = cp1252(b);
                let text = c.to_string();
                let Ok(m) = ctx.measure_text(&text) else {
                    return Measured {
                        text,
                        advance: 0.0,
                        left: 0,
                        up: 0,
                        w: 0,
                        h: 0,
                        at: None,
                    };
                };
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
                let (w, h) = ((left + right).max(0) as u32, (up + down).max(0) as u32);
                let mut at = None;
                if w > 0 && h > 0 && c != ' ' {
                    if x > 0 && x + w > DRAW_WIDTH {
                        (x, y, row_h) = (0, y + row_h + 1, 0);
                    }
                    if sheets.is_empty() || (y > 0 && y + h > DRAW_HEIGHT) {
                        sheets.push((0, 0));
                        (x, y, row_h) = (0, 0, 0);
                    }
                    let sheet = sheets.len() - 1;
                    at = Some((sheet, x, y));
                    let size = &mut sheets[sheet];
                    *size = (size.0.max(x + w), size.1.max(y + h));
                    x += w + 1;
                    row_h = row_h.max(h);
                }
                Measured {
                    text,
                    advance: m.width() as f32,
                    left,
                    up,
                    w,
                    h,
                    at,
                }
            })
            .collect();
        // Each sheet drawn and read back; a glyph's coverage is its box's alpha (a sheet the canvas refused leaves its glyphs
        // without ink).
        let mut coverage: Vec<Vec<u8>> = vec![Vec::new(); glyphs.len()];
        for (index, &(width, height)) in sheets.iter().enumerate() {
            let on_sheet = || {
                glyphs
                    .iter()
                    .enumerate()
                    .filter_map(move |(i, g)| match g.at {
                        Some((s, x, y)) if s == index => Some((i, g, x, y)),
                        _ => None,
                    })
            };
            let Some((_sheet, ctx)) = canvas(width, height) else {
                continue;
            };
            set_face(&ctx, &font);
            for (_, g, x, y) in on_sheet() {
                let (x, y) = (f64::from(x), f64::from(y));
                ctx.save();
                ctx.begin_path();
                ctx.rect(x, y, f64::from(g.w), f64::from(g.h));
                ctx.clip();
                let _ = ctx.fill_text(&g.text, x + f64::from(g.left), y + f64::from(g.up));
                ctx.restore();
            }
            let Ok(image) = ctx.get_image_data(0.0, 0.0, f64::from(width), f64::from(height))
            else {
                continue;
            };
            let pixels = image.data().0;
            for (i, g, x, y) in on_sheet() {
                let mut alpha = Vec::with_capacity((g.w * g.h) as usize);
                for r in y..y + g.h {
                    let at = ((r * width + x) * 4) as usize;
                    let row = &pixels[at..at + (g.w * 4) as usize];
                    alpha.extend(row.as_chunks::<4>().0.iter().map(|p| p[3]));
                }
                coverage[i] = alpha;
            }
        }
        let mut images = glyphs
            .iter()
            .zip(coverage)
            .map(|(g, coverage)| g.image(coverage));
        // `pack` asks for the glyphs in the order they were measured (bytes 32 to 255), once each.
        Some(pack(ascent, height, |_| {
            images.next().unwrap_or(GlyphImage {
                width: 0,
                height: 0,
                coverage: Vec::new(),
                xmin: 0,
                ymin: 0,
                advance: 0.0,
            })
        }))
    }

    fn name(&self) -> &'static str {
        "the browser's fonts (canvas)"
    }
}
