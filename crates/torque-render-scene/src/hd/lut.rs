//! The grading LUT as plain data: our own 32³ 3D table on display-encoded axes that holds the image settings' formation from the exposed HDR
//! colour to the display (the curve's coupling, then the grade, then an optional user `.cube`), and the `.cube` reader.
//!
//! - **The shaper** ([`shaper`], computed by the tonemap pass before the table): the part of a curve that couples the channels over
//!   an unbounded range, which no table of a bounded range can hold (a colour whose largest channel is past the table's top would
//!   lose its ratios): the neutral hue-keeping shoulder (the largest channel brought to at most 1, the others scaled with it), or the
//!   AgX-style pull toward the luminance. Before it, the grade's linear part (white balance, saturation).
//! - **The table's axes** ([`LutAxis`]): the curve's own per-channel encoding of the shaped value, so the nodes fall evenly on the
//!   display values the grade works on: the neutral curve the sRGB transfer (a power law over the shaped 0..1, the perceptual cousin of a log
//!   encoding); the AgX-style curve's log2 exposure about mid grey (-11 to +7 EV) through its sigmoid. A plain log2 axis
//!   was tried first: 32 nodes over 12 EV (neutral) or 18 EV (AgX-style) put the curve's bend between nodes, 0.5 to 0.6/255 off in the
//!   frame. The table holds the rest of the curve (neutral nothing more, AgX-style the saturation given back), then the grade's display
//!   part, then the user table, all open (no clamp inside: the tonemap pass clamps after the lookup, else a clamp would be
//!   interpolated across). With the identity grade the table is the analytic curves to the hardware's filtering (the
//!   `LUT identity`).
//! - **The grade** ([`Grade`]): a white balance (linear multipliers) and a saturation about the luminance (linear, before the
//!   shaper), then on the display values a contrast about mid grey, lift, gamma and gain per channel (ASC CDL's form), and a split
//!   tone (shadows toward one hue, highlights toward another). The default grade is the identity (a warm and cool split tone moved
//!   the scene's hues). The AgX-style shaper and axis stay as
//!   data for the [`LutAxis::AgxSigmoid`] axis; the default settings do not take it.
//! - **A user `.cube`** (Adobe's Cube LUT format 1.0, `LUT_3D_SIZE` 2 to [`MAX_CUBE_SIZE`]) is applied last, to the display values
//!   (a display-referred grade, as photo tools export them), trilinearly. [`parse_cube`] is a file reader: every size and count is
//!   capped and checked before anything is allocated, and any malformed line refuses the whole file.

use crate::hd::ImageSettings;
use crate::hd::grade::{AGX_INSET, Tonemap, agx_channel, agx_outset, agx_style, agx_style_open};
use crate::hd::linear_to_srgb;

/// The table's nodes a side.
pub const LUT_SIZE: usize = 32;

/// How a shaped linear value is placed on the table's axis (0..1 over the nodes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LutAxis {
    /// The sRGB transfer of the shaped value (0..1 after the neutral shoulder).
    Srgb,
    /// The AgX-style curve's log2 exposure about mid grey through its sigmoid ([`agx_channel`]).
    AgxSigmoid,
}

impl LutAxis {
    /// The shader's code of an axis (`tonemap.wgsl`'s `lut.y`).
    pub fn code(self) -> f32 {
        match self {
            LutAxis::Srgb => 0.0,
            LutAxis::AgxSigmoid => 1.0,
        }
    }
}

/// A grade (the identity is [`Grade::NEUTRAL`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grade {
    /// Linear multipliers before the curve.
    pub white_balance: [f32; 3],
    /// About the luminance, before the curve (1: none).
    pub saturation: f32,
    /// About the display's mid grey (1: none).
    pub contrast: f32,
    /// ASC CDL's offset-like lift, power and slope per channel, on display values.
    pub lift: [f32; 3],
    pub gamma: [f32; 3],
    pub gain: [f32; 3],
    /// Added to the display values, weighted toward the shadows and toward the highlights.
    pub shadow_tint: [f32; 3],
    pub highlight_tint: [f32; 3],
}

impl Grade {
    pub const NEUTRAL: Grade = Grade {
        white_balance: [1.0; 3],
        saturation: 1.0,
        contrast: 1.0,
        lift: [0.0; 3],
        gamma: [1.0; 3],
        gain: [1.0; 3],
        shadow_tint: [0.0; 3],
        highlight_tint: [0.0; 3],
    };
}

/// The neutral hue-keeping shoulder (the CPU form of `tonemap.wgsl`'s `tonemap`).
pub fn neutral_tonemap(c: [f32; 3]) -> [f32; 3] {
    const SHOULDER: f32 = 0.8;
    let m = c[0].max(c[1]).max(c[2]);
    if m <= SHOULDER {
        return c;
    }
    let span = 1.0 - SHOULDER;
    let mapped = SHOULDER + span * (1.0 - (-(m - SHOULDER) / span).exp());
    c.map(|v| v * (mapped / m))
}

fn lum(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn clean(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 })
}

/// The shaper the tonemap pass applies before the table: the neutral hue-keeping shoulder, or the AgX-style pull toward the
/// luminance.
pub fn shaper(settings: impl Into<ImageSettings>, c: [f32; 3]) -> [f32; 3] {
    let settings: ImageSettings = settings.into();
    let c = clean(c);
    match settings.tonemap() {
        Tonemap::Neutral => neutral_tonemap(c),
        Tonemap::AgxStyle => {
            let l = lum(c);
            c.map(|v| v + (l - v) * AGX_INSET)
        }
    }
}

/// The settings' analytic curve: an exposed linear colour to display values (what the tonemap pass computes without the table).
pub fn curve(settings: impl Into<ImageSettings>, c: [f32; 3]) -> [f32; 3] {
    let settings: ImageSettings = settings.into();
    let c = clean(c);
    match settings.tonemap() {
        Tonemap::Neutral => neutral_tonemap(c).map(|v| linear_to_srgb(v.clamp(0.0, 1.0))),
        Tonemap::AgxStyle => agx_style(c),
    }
}

/// The grade's linear part (the white balance and the saturation), which the tonemap pass applies before the shaper.
pub fn graded_linear(grade: &Grade, c: [f32; 3]) -> [f32; 3] {
    let c = clean(c);
    let mut x = [0, 1, 2].map(|k| c[k] * grade.white_balance[k]);
    if grade.saturation != 1.0 {
        let l = lum(x);
        x = x.map(|v| (l + (v - l) * grade.saturation).max(0.0));
    }
    x
}

/// The grade's display part and a user table on the curve's display values `d`, open (smooth past 0..1: the tonemap pass clamps
/// after the table, so no clamp is interpolated across; the user table is read clamped to its domain).
pub fn graded_display(grade: &Grade, user: Option<&CubeLut>, mut d: [f32; 3]) -> [f32; 3] {
    if grade.contrast != 1.0 {
        let pivot = 0.46;
        d = d.map(|v| pivot + (v - pivot) * grade.contrast);
    }
    d = [0, 1, 2].map(|k| {
        let v = d[k] * grade.gain[k] + grade.lift[k] * (1.0 - d[k]);
        let g = 1.0 / grade.gamma[k].max(1e-3);
        if g == 1.0 {
            v
        } else {
            v.abs().powf(g).copysign(v)
        }
    });
    let l = lum(d).clamp(0.0, 1.0);
    let shadows = (1.0 - l) * (1.0 - l);
    let highlights = l * l;
    d = [0, 1, 2]
        .map(|k| d[k] + grade.shadow_tint[k] * shadows + grade.highlight_tint[k] * highlights);
    match user {
        Some(u) => u.sample(d),
        None => d,
    }
}

/// The settings' formation with `grade` and an optional user table, exactly: an exposed linear colour to display values 0..1.
pub fn formed(
    settings: impl Into<ImageSettings>,
    grade: &Grade,
    user: Option<&CubeLut>,
    c: [f32; 3],
) -> [f32; 3] {
    let settings: ImageSettings = settings.into();
    let x = graded_linear(grade, c);
    let open = match settings.tonemap() {
        Tonemap::Neutral => curve(settings, x),
        Tonemap::AgxStyle => agx_style_open(shaper(settings, x)),
    };
    graded_display(grade, user, open).map(|v| v.clamp(0.0, 1.0))
}

/// The open curve (before the grade) at the node whose axis coordinates are `t` (0..1 each).
fn node_curve(settings: ImageSettings, t: [f32; 3]) -> [f32; 3] {
    match settings.lut_axis() {
        LutAxis::Srgb => t,
        // The axis is the curve's per-channel part itself.
        LutAxis::AgxSigmoid => agx_outset(t),
    }
}

/// The table of the settings (RGBA, red fastest, then green, then blue; alpha 1): the open curve after the shaper and the grade's
/// display part (and the user table) at every node.
pub fn bake(
    settings: impl Into<ImageSettings>,
    grade: &Grade,
    user: Option<&CubeLut>,
) -> Vec<[f32; 4]> {
    let settings: ImageSettings = settings.into();
    let n = (LUT_SIZE - 1) as f32;
    let mut out = Vec::with_capacity(LUT_SIZE.pow(3));
    for b in 0..LUT_SIZE {
        for g in 0..LUT_SIZE {
            for r in 0..LUT_SIZE {
                let t = [r as f32 / n, g as f32 / n, b as f32 / n];
                let d = graded_display(grade, user, node_curve(settings, t));
                out.push([d[0], d[1], d[2], 1.0]);
            }
        }
    }
    out
}

/// The table's coordinate (0..1 over the nodes, as the GPU's trilinear lookup takes it) of a shaped value.
pub fn encode(axis: LutAxis, v: f32) -> f32 {
    let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
    match axis {
        LutAxis::Srgb => linear_to_srgb(v.clamp(0.0, 1.0)),
        LutAxis::AgxSigmoid => agx_channel(v),
    }
}

/// A trilinear lookup of `table` (`n`³, red fastest) at node coordinates `p` (0..n-1 each).
fn trilinear(table: &[[f32; 4]], n: usize, p: [f32; 3]) -> [f32; 3] {
    let p = p.map(|v| v.clamp(0.0, (n - 1) as f32));
    let i = p.map(|v| (v.floor() as usize).min(n - 2));
    let f = [0, 1, 2].map(|k| p[k] - i[k] as f32);
    let at = |r: usize, g: usize, b: usize| table[(b * n + g) * n + r];
    let mut out = [0.0f32; 3];
    for corner in 0..8 {
        let d = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
        let w: f32 = (0..3)
            .map(|k| if d[k] == 1 { f[k] } else { 1.0 - f[k] })
            .product();
        let c = at(i[0] + d[0], i[1] + d[1], i[2] + d[2]);
        for k in 0..3 {
            out[k] += w * c[k];
        }
    }
    out
}

/// What the GPU computes from a baked table for an exposed linear colour (the CPU form of `tonemap.wgsl`'s `graded`): the grade's
/// linear part, the shaper, the table.
pub fn lookup(
    settings: impl Into<ImageSettings>,
    grade: &Grade,
    table: &[[f32; 4]],
    c: [f32; 3],
) -> [f32; 3] {
    let settings: ImageSettings = settings.into();
    let axis = settings.lut_axis();
    let s = shaper(settings, graded_linear(grade, c));
    let p = s.map(|v| encode(axis, v) * (LUT_SIZE - 1) as f32);
    trilinear(table, LUT_SIZE, p).map(|v| v.clamp(0.0, 1.0))
}

/// The largest `.cube` read (bytes) and its table's largest side.
pub const MAX_CUBE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_CUBE_SIZE: usize = 65;
/// The longest line read.
pub const MAX_CUBE_LINE: usize = 512;
/// The values a `.cube` may hold (a display table's outputs a little outside 0..1 are kept, nothing wild).
pub const CUBE_VALUE_LIMIT: f32 = 16.0;

/// A user `.cube` 3D table (display to display).
#[derive(Debug, Clone, PartialEq)]
pub struct CubeLut {
    pub title: String,
    pub size: usize,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    /// `size`³ entries, red fastest.
    pub table: Vec<[f32; 4]>,
}

impl CubeLut {
    /// A trilinear lookup of display values (clamped to the domain).
    pub fn sample(&self, c: [f32; 3]) -> [f32; 3] {
        let p = [0, 1, 2].map(|k| {
            let span = (self.domain_max[k] - self.domain_min[k]).max(1e-6);
            ((c[k] - self.domain_min[k]) / span).clamp(0.0, 1.0) * (self.size - 1) as f32
        });
        trilinear(&self.table, self.size, p)
    }

    /// The identity table of `size` (tests, and a reference for a user's file).
    pub fn identity(size: usize) -> CubeLut {
        let size = size.clamp(2, MAX_CUBE_SIZE);
        let mut table = Vec::with_capacity(size.pow(3));
        let s = (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    table.push([r as f32 / s, g as f32 / s, b as f32 / s, 1.0]);
                }
            }
        }
        CubeLut {
            title: "identity".to_string(),
            size,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            table,
        }
    }
}

/// Why a `.cube` was refused.
pub type CubeError = String;

fn three(words: &[&str], line: usize) -> Result<[f32; 3], CubeError> {
    if words.len() != 3 {
        return Err(format!("line {line}: three numbers expected"));
    }
    let mut out = [0.0f32; 3];
    for (o, w) in out.iter_mut().zip(words) {
        if w.len() > 32 {
            return Err(format!("line {line}: a number too long"));
        }
        let v: f32 = w
            .parse()
            .map_err(|_| format!("line {line}: not a number"))?;
        if !v.is_finite() || v.abs() > CUBE_VALUE_LIMIT {
            return Err(format!("line {line}: a value out of range"));
        }
        *o = v;
    }
    Ok(out)
}

/// Reads a `.cube` file's text (Adobe Cube LUT 1.0: `TITLE`, `LUT_3D_SIZE`, `DOMAIN_MIN`, `DOMAIN_MAX`, `#` comments, then the
/// size³ rows of three numbers, red fastest). A 1D table, an unknown keyword, a keyword after the rows, a malformed, overlong or
/// control-character line, a value not finite or beyond ±[`CUBE_VALUE_LIMIT`], an empty domain, or a row count other than the size³
/// refuses the file.
pub fn parse_cube(text: &str) -> Result<CubeLut, CubeError> {
    if text.len() as u64 > MAX_CUBE_BYTES {
        return Err("the file is too large".to_string());
    }
    let mut title = String::new();
    let mut size: Option<usize> = None;
    let mut domain_min = [0.0f32; 3];
    let mut domain_max = [1.0f32; 3];
    let mut table: Vec<[f32; 4]> = Vec::new();
    let mut expected = 0usize;
    for (n, raw) in text.lines().enumerate() {
        let line_no = n + 1;
        if raw.len() > MAX_CUBE_LINE {
            return Err(format!("line {line_no}: too long"));
        }
        if raw
            .chars()
            .any(|c| c.is_control() && c != '\t' && c != '\r')
        {
            return Err(format!("line {line_no}: a control character"));
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let head = words[0];
        let numeric = head
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+' || c == '.');
        if numeric {
            let Some(s) = size else {
                return Err(format!("line {line_no}: rows before LUT_3D_SIZE"));
            };
            if table.len() >= expected {
                return Err(format!("line {line_no}: more than {s}^3 rows"));
            }
            let v = three(&words, line_no)?;
            table.push([v[0], v[1], v[2], 1.0]);
            continue;
        }
        if !table.is_empty() {
            return Err(format!("line {line_no}: a keyword after the rows"));
        }
        match head {
            "TITLE" => {
                let rest = line["TITLE".len()..].trim().trim_matches('"');
                title = rest.chars().take(128).collect();
            }
            "LUT_3D_SIZE" => {
                if size.is_some() || words.len() != 2 {
                    return Err(format!("line {line_no}: LUT_3D_SIZE again or malformed"));
                }
                let s: usize = words[1]
                    .parse()
                    .map_err(|_| format!("line {line_no}: LUT_3D_SIZE is not a number"))?;
                if !(2..=MAX_CUBE_SIZE).contains(&s) {
                    return Err(format!(
                        "line {line_no}: LUT_3D_SIZE {s} (2 to {MAX_CUBE_SIZE})"
                    ));
                }
                size = Some(s);
                expected = s * s * s;
                table.reserve_exact(expected);
            }
            "DOMAIN_MIN" => domain_min = three(&words[1..], line_no)?,
            "DOMAIN_MAX" => domain_max = three(&words[1..], line_no)?,
            "LUT_1D_SIZE" => return Err("a 1D table (only 3D tables are read)".to_string()),
            _ => return Err(format!("line {line_no}: an unknown keyword")),
        }
    }
    let Some(size) = size else {
        return Err("no LUT_3D_SIZE".to_string());
    };
    if table.len() != expected {
        return Err(format!(
            "{} rows, {size}^3 = {expected} expected",
            table.len()
        ));
    }
    if (0..3).any(|k| domain_max[k] <= domain_min[k]) {
        return Err("an empty domain".to_string());
    }
    Ok(CubeLut {
        title,
        size,
        domain_min,
        domain_max,
        table,
    })
}

#[cfg(test)]
#[path = "lut_tests.rs"]
mod tests;
