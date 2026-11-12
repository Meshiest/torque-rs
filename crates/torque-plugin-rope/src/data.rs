//! The `RopeData` datablock: how a kind of rope is built and how it behaves.

use torque_engine::{EngineClass, Filename};

/// The most pieces a rope is cut into.
pub const MAX_SEGMENTS_PER_ROPE: usize = 64;
/// The longest a rope may be (world units).
pub const MAX_LENGTH: f32 = 10_000.0;

/// How a rope is drawn: set by the datablock's `style`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RopeStyle {
    /// A lit tube that casts a shadow, simulated: the default.
    #[default]
    Tube,
    /// A flat strip that always faces the camera, following a smooth curve through the simulated rope's points. Cheap cables,
    /// vines, beams and tethers. It casts no shadow.
    Ribbon,
    /// A straight strip that always faces the camera between the rope's two ends: no sag, no simulation (a laser, a tripwire, a
    /// power line seen from far away). It casts no shadow.
    Line,
}

impl RopeStyle {
    /// The style a script names (`"tube"`, `"ribbon"`, `"line"`, in any case); None for anything else.
    pub fn parse(text: &str) -> Option<RopeStyle> {
        match text.trim().to_ascii_lowercase().as_str() {
            "tube" => Some(RopeStyle::Tube),
            "ribbon" => Some(RopeStyle::Ribbon),
            "line" => Some(RopeStyle::Line),
            _ => None,
        }
    }

    /// The name a script writes.
    pub fn name(self) -> &'static str {
        match self {
            RopeStyle::Tube => "tube",
            RopeStyle::Ribbon => "ribbon",
            RopeStyle::Line => "line",
        }
    }
}

/// A kind of rope: a script writes one as `datablock RopeData(Cable) { segments = 16; thickness = 0.05; };` and names it when it
/// makes a rope (`ropeCreate(start, end, Cable)`). Every field is clamped to its range when it is written, and again when a rope
/// reads it, so a hostile value never reaches the simulation ([`RopeParams`]).
#[derive(EngineClass, Clone, Debug, PartialEq)]
#[class(name = "RopeData", parent = "SimDataBlock")]
pub struct RopeData {
    /// The pieces the rope is cut into. More is smoother and dearer to simulate and draw.
    #[field(range = 2..=64, default = 12)]
    pub segments: i32,
    /// The rope's resting length in world units; 0 is 1.1 times the distance between its ends when it is made.
    #[field(range = 0.0..=10000.0, default = 0.0)]
    pub length: f32,
    /// How the rope is drawn: `"tube"` (lit, casts a shadow), `"ribbon"` (a flat strip that faces the camera along a smooth curve)
    /// or `"line"` (a straight strip between the ends, not simulated). Anything else is a warning and a tube.
    #[field(default = "tube")]
    pub style: String,
    /// The tube's or strip's width in world units.
    #[field(range = 0.005..=2.0, default = 0.06)]
    pub thickness: f32,
    /// How much the rope resists bending, 0 (a cord) to 1 (a cable).
    #[field(range = 0.0..=1.0, default = 0.0)]
    pub stiffness: f32,
    /// How fast it loses speed, per second.
    #[field(range = 0.0..=20.0, default = 0.5)]
    pub damping: f32,
    /// The pull toward the ground, in units a second squared.
    #[field(range = 0.0..=100.0, default = 9.81)]
    pub gravity: f32,
    /// The rope's paint, as `"r g b a"` from 0 to 255. A ribbon or a line is see-through by the alpha; a tube is not.
    #[field(default = "150 110 70 255")]
    pub color: [u8; 4],
    /// Whether it casts a shadow from the sun: 1 or 0, or -1 (the default) for the style's own: a tube does, a ribbon and a line do not.
    /// A ribbon's or a line's shadow is a band of its thickness that faces the sun, so it does not change as the camera moves.
    #[field(name = "castShadow", range = -1..=1, default = -1)]
    pub cast_shadow: i32,
    /// Whether the sun's shadows fall on it: 1 or 0, or -1 (the default) for the style's own: a tube and a ribbon do, a line does
    /// unless it is emissive (a laser does not darken).
    #[field(name = "receiveShadow", range = -1..=1, default = -1)]
    pub receive_shadow: i32,
    /// Whether a ribbon or a line glows: drawn in its color with no light or shade (a laser). Fog still covers it.
    #[field(default = false)]
    pub emissive: bool,
    /// How the rope's shape comes about: `"verlet"` (the default: simulated, sagging and swinging) or `"none"`: not simulated, its
    /// shape is driven: straight between its ends, or the curve `ropeSetCurve` sets (a beam from a muzzle), both ends and the curve's
    /// tangents following what is drawn each frame.
    #[field(default = "verlet")]
    pub sim: String,
    /// How bright a glowing ribbon or line is: its color is multiplied by this in the HDR frame, so more than 1 is brighter than
    /// white and the bloom (where the renderer has it) catches it.
    #[field(name = "emissiveIntensity", range = 0.0..=64.0, default = 1.0)]
    pub emissive_intensity: f32,
    /// Whether a ribbon or a line adds its light to what is behind it (an energy beam) instead of covering it by its alpha.
    #[field(default = false)]
    pub additive: bool,
    /// The width at the far end, in world units: `thickness` tapers to it along the rope. -1 (the default) is no taper.
    #[field(name = "endThickness", range = -1.0..=2.0, default = -1.0)]
    pub end_thickness: f32,
    /// The color at the far end, `"r g b a"`: the paint blends from `color` to it along the rope. Empty (the default) is no gradient.
    #[field(name = "colorEnd", default = "")]
    pub color_end: String,
    /// A picture to draw the rope with (a `.png` or `.jpg`; empty for none): it runs along the rope (u) and around a tube or across
    /// a strip (v). The `color` multiplies it (white keeps its own colors). A ribbon or a line blends by its alpha; a tube cuts out
    /// what is under half alpha (and casts no shadow there). A file that is missing or does not load warns once and the rope is
    /// drawn plain.
    #[field(default = "")]
    pub texture: Filename,
    /// How far along the rope one repeat of the texture is, in world units; 0 (the default) is the rope's width, a square tile.
    #[field(name = "textureLength", range = 0.0..=1000.0, default = 0.0)]
    pub texture_length: f32,
    /// How fast the texture runs along the rope, in repeats a second (negative: toward the start): a laser, a conveyor, an energy
    /// beam.
    #[field(name = "textureScroll", range = -1000.0..=1000.0, default = 0.0)]
    pub texture_scroll: f32,
    /// How the texture is filtered: `"linear"` (the default, with mip levels) or `"nearest"` (hard texels, for pixel art).
    #[field(name = "textureFilter", default = "linear")]
    pub texture_filter: String,
}

/// What a script changed on one rope with `ropeSetColor` and kin: each value that is set takes the place of the datablock's for that
/// rope alone, and `None` is the datablock's own. Plain data (a rope's ghost can carry it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RopeLook {
    /// The paint.
    pub color: Option<[u8; 4]>,
    /// The texture's file (empty text is not stored: `None` is the datablock's).
    pub texture: Option<String>,
    /// The texture's speed, repeats a second.
    pub scroll: Option<f32>,
    /// Whether a strip glows.
    pub emissive: Option<bool>,
}

/// What a rope is made from: a [`RopeData`] with every number made safe (a NaN or an infinity becomes the field's default, the rest
/// is kept inside its range).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RopeParams {
    /// How it is drawn.
    pub style: RopeStyle,
    /// The pieces the rope is cut into (2 to [`MAX_SEGMENTS_PER_ROPE`]; a line is one piece whatever this is).
    pub segments: usize,
    /// The resting length; 0 for 1.1 times the distance between the ends when the rope is made.
    pub length: f32,
    /// The tube's or strip's width.
    pub thickness: f32,
    /// How much it resists bending, 0 to 1.
    pub stiffness: f32,
    /// The speed it loses a second.
    pub damping: f32,
    /// The pull down, units a second squared.
    pub gravity: f32,
    /// The paint.
    pub color: [u8; 4],
    /// Whether it casts a shadow.
    pub cast_shadow: bool,
    /// Whether the sun's shadows fall on it.
    pub receive_shadow: bool,
    /// Whether a ribbon or a line is drawn unlit.
    pub emissive: bool,
    /// Whether the shape is driven, not simulated.
    pub driven: bool,
    /// The glow's brightness.
    pub emissive_intensity: f32,
    /// Whether a strip adds its light.
    pub additive: bool,
    /// The width at the far end; 0 (or less) for no taper ([`RopeParams::end_thickness`]).
    pub thickness_end: f32,
    /// The color at the far end, if the paint blends to one.
    pub color_end: Option<[u8; 4]>,
    /// The datablock's texture, as the world numbers it (0: none, or one that did not load): set when the rope is made.
    pub texture: u32,
    /// How far along the rope one repeat is (0: the width).
    pub texture_length: f32,
    /// The texture's speed, repeats a second.
    pub texture_scroll: f32,
    /// Whether the texture is filtered `"nearest"`.
    pub texture_nearest: bool,
}

impl Default for RopeParams {
    fn default() -> RopeParams {
        RopeParams {
            style: RopeStyle::Tube,
            segments: 12,
            length: 0.0,
            thickness: 0.06,
            stiffness: 0.0,
            damping: 0.5,
            gravity: 9.81,
            color: [150, 110, 70, 255],
            cast_shadow: true,
            receive_shadow: true,
            emissive: false,
            driven: false,
            emissive_intensity: 1.0,
            additive: false,
            thickness_end: 0.0,
            color_end: None,
            texture: 0,
            texture_length: 0.0,
            texture_scroll: 0.0,
            texture_nearest: false,
        }
    }
}

/// `"r g b"` or `"r g b a"` as a color, each number from 0 to 255 (a number past that is held to it; alpha is 255 when left out).
/// None for anything else.
pub fn parse_color(text: &str) -> Option<[u8; 4]> {
    let words: Vec<&str> = text.split_ascii_whitespace().collect();
    if !(3..=4).contains(&words.len()) {
        return None;
    }
    let mut out = [255u8; 4];
    for (slot, word) in out.iter_mut().zip(&words) {
        let value = word.parse::<f64>().ok().filter(|v| v.is_finite())?;
        *slot = value.round().clamp(0.0, 255.0) as u8;
    }
    Some(out)
}

/// A switch a script writes as 1 or 0, or -1 (anything below 0) for `by_default`.
fn choice(value: i32, by_default: bool) -> bool {
    match value {
        0 => false,
        v if v > 0 => true,
        _ => by_default,
    }
}

/// `value` inside `low..=high`; `fallback` if it is not a finite number.
fn clean(value: f32, low: f32, high: f32, fallback: f32) -> f32 {
    torque_collide::math::finite_clamp(value, low, high).unwrap_or(fallback)
}

impl RopeParams {
    /// The parameters of the datablock `data`, every field inside its range.
    pub fn new(data: &RopeData) -> RopeParams {
        let d = RopeParams::default();
        let style = RopeStyle::parse(&data.style).unwrap_or_default();
        RopeParams {
            style,
            segments: usize::try_from(data.segments)
                .unwrap_or(d.segments)
                .clamp(2, MAX_SEGMENTS_PER_ROPE),
            length: clean(data.length, 0.0, MAX_LENGTH, d.length),
            thickness: clean(data.thickness, 0.005, 2.0, d.thickness),
            stiffness: clean(data.stiffness, 0.0, 1.0, d.stiffness),
            damping: clean(data.damping, 0.0, 20.0, d.damping),
            gravity: clean(data.gravity, 0.0, 100.0, d.gravity),
            color: data.color,
            cast_shadow: choice(data.cast_shadow, style == RopeStyle::Tube),
            receive_shadow: choice(
                data.receive_shadow,
                !(style == RopeStyle::Line && data.emissive),
            ),
            emissive: data.emissive,
            driven: data.sim.trim().eq_ignore_ascii_case("none"),
            emissive_intensity: clean(data.emissive_intensity, 0.0, 64.0, 1.0),
            additive: data.additive,
            thickness_end: if data.end_thickness >= 0.005 {
                clean(data.end_thickness, 0.005, 2.0, 0.0)
            } else {
                0.0
            },
            color_end: parse_color(&data.color_end),
            texture: 0,
            texture_length: clean(data.texture_length, 0.0, 1000.0, d.texture_length),
            texture_scroll: clean(data.texture_scroll, -1000.0, 1000.0, d.texture_scroll),
            texture_nearest: data.texture_filter.trim().eq_ignore_ascii_case("nearest"),
        }
    }

    /// The width at the far end: [`RopeParams::thickness_end`], or the thickness for a rope that does not taper.
    pub fn end_thickness(&self) -> f32 {
        if self.thickness_end > 0.0 {
            self.thickness_end
        } else {
            self.thickness
        }
    }

    /// The pieces the rope is really made of: [`RopeParams::segments`], or one for a line.
    pub fn pieces(&self) -> usize {
        match self.style {
            RopeStyle::Line => 1,
            _ => self.segments.clamp(2, MAX_SEGMENTS_PER_ROPE),
        }
    }
}

impl From<&RopeData> for RopeParams {
    fn from(data: &RopeData) -> RopeParams {
        RopeParams::new(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(style: &str) -> RopeData {
        RopeData {
            style: style.to_string(),
            segments: 12,
            length: 0.0,
            thickness: 0.06,
            stiffness: 0.0,
            damping: 0.5,
            gravity: 9.81,
            color: [1, 2, 3, 4],
            cast_shadow: -1,
            receive_shadow: -1,
            emissive: false,
            sim: "verlet".to_string(),
            emissive_intensity: 1.0,
            additive: false,
            end_thickness: -1.0,
            color_end: String::new(),
            texture: Filename(String::new()),
            texture_length: 0.0,
            texture_scroll: 0.0,
            texture_filter: "linear".to_string(),
        }
    }

    #[test]
    fn the_texture_fields_are_read_and_made_safe() {
        let mut d = data("ribbon");
        d.texture = Filename("art/braid.png".to_string());
        d.texture_length = 0.5;
        d.texture_scroll = -2.0;
        d.texture_filter = " Nearest ".to_string();
        let p = RopeParams::new(&d);
        assert_eq!(
            (p.texture_length, p.texture_scroll, p.texture_nearest),
            (0.5, -2.0, true)
        );
        d.texture_length = f32::NAN;
        d.texture_scroll = f32::INFINITY;
        d.texture_filter = "trilinear".to_string();
        let p = RopeParams::new(&d);
        assert_eq!(
            (p.texture_length, p.texture_scroll, p.texture_nearest),
            (0.0, 0.0, false)
        );
        d.texture_length = 1.0e9;
        d.texture_scroll = -1.0e9;
        let p = RopeParams::new(&d);
        assert_eq!((p.texture_length, p.texture_scroll), (1000.0, -1000.0));
    }

    #[test]
    fn the_style_names_parse_in_any_case() {
        for (text, style) in [
            ("tube", RopeStyle::Tube),
            ("Ribbon", RopeStyle::Ribbon),
            (" LINE ", RopeStyle::Line),
        ] {
            assert_eq!(RopeStyle::parse(text), Some(style), "{text:?}");
            assert_eq!(RopeStyle::parse(style.name()), Some(style));
        }
        for text in ["", "rope", "ribbons", "tube ribbon"] {
            assert_eq!(RopeStyle::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn an_unknown_style_is_a_tube() {
        assert_eq!(RopeParams::new(&data("laser")).style, RopeStyle::Tube);
        assert_eq!(RopeParams::new(&data("")).style, RopeStyle::Tube);
        assert_eq!(RopeParams::new(&data("ribbon")).style, RopeStyle::Ribbon);
        assert_eq!(RopeParams::default().style, RopeStyle::Tube);
    }

    #[test]
    fn a_tube_casts_a_shadow_by_default_and_a_ribbon_and_a_line_do_not() {
        assert!(RopeParams::new(&data("tube")).cast_shadow);
        assert!(!RopeParams::new(&data("ribbon")).cast_shadow);
        assert!(!RopeParams::new(&data("line")).cast_shadow);
        for (style, cast) in [("tube", false), ("ribbon", true), ("line", true)] {
            let mut d = data(style);
            d.cast_shadow = i32::from(cast);
            assert_eq!(RopeParams::new(&d).cast_shadow, cast, "{style}");
        }
    }

    #[test]
    fn a_laser_does_not_take_a_shadow_by_default_and_the_rest_do() {
        let mut d = data("line");
        assert!(RopeParams::new(&d).receive_shadow);
        d.emissive = true;
        assert!(!RopeParams::new(&d).receive_shadow);
        d.receive_shadow = 1;
        assert!(RopeParams::new(&d).receive_shadow);
        for style in ["tube", "ribbon"] {
            let mut d = data(style);
            d.emissive = true;
            assert!(RopeParams::new(&d).receive_shadow, "{style}");
            d.receive_shadow = 0;
            assert!(!RopeParams::new(&d).receive_shadow, "{style}");
        }
    }

    #[test]
    fn a_line_is_one_piece() {
        assert_eq!(RopeParams::new(&data("line")).pieces(), 1);
        assert_eq!(RopeParams::new(&data("ribbon")).pieces(), 12);
        assert_eq!(RopeParams::new(&data("tube")).pieces(), 12);
    }
}
