//! The enhanced renderer's image formation as plain data (exposure,
//! tonemap, bloom, emissive): what the settings do from the HDR frame to the display, and the CPU forms the checks and
//! tests check the GPU's against.
//!
//! - **Exposure**: fixed (1: the lit colour of the fixed-function frame).
//!   The automatic exposure (kept for the renderer's `auto_exposure` switch): a 256-bin histogram of log2
//!   luminance ([`HISTOGRAM_MIN_EV`] to [`HISTOGRAM_MAX_EV`]), the mean of the bins between the [`LOW_PERCENTILE`] and the
//!   [`HIGH_PERCENTILE`] of the pixels set to [`DEFAULT_KEY`], clamped to ±[`EV_CLAMP`] about the fixed exposure, approached
//!   exponentially with the time constant [`ADAPT_SECONDS`] (a history's first frame takes it at once).
//! - **Tonemap**: our neutral hue-keeping shoulder. The AgX-style formation (kept as data: [`agx_style`]) pulled saturated
//!   colours off their hue: the colour pulled
//!   a little toward its luminance (so a saturated colour can reach white), a log2 encoding around mid grey, a logistic sigmoid per
//!   channel (each bright channel rolls off on its own: the "path to white"), the saturation given back, the result display-encoded.
//! - **Bloom**: a dual-filter mip chain (5 to 8 levels by quality), each level a blur of the one above with weights summing to 1, and
//!   the way up a convex mix of each level and the next one's blur: the chain conserves the light it is given (within 1 %).
//!   With a threshold it is fed only what is above a soft threshold (glow, lights, the sun, flares) and added; the default settings feed
//!   it the whole frame and mixes [`DEFAULT_BLOOM_MIX`] of it in (energy-conserving end to end).
//! - **Emissive**: emissive surfaces are their colour times [`Emissive::glow`]; self-illuminating shape parts, coronas and flares are scaled
//!   into the HDR range the bloom picks up.

use crate::hd::Quality;

/// The histogram's bins and range (log2 luminance).
pub const HISTOGRAM_BINS: usize = 256;
pub const HISTOGRAM_MIN_EV: f32 = -14.0;
pub const HISTOGRAM_MAX_EV: f32 = 10.0;
/// The share of the pixels below the averaged range and the share at its top (the darkest 40 % and the brightest 2 % are left out).
pub const LOW_PERCENTILE: f32 = 0.4;
pub const HIGH_PERCENTILE: f32 = 0.98;
/// The luminance the averaged range is exposed to (the default settings'), the clamp about the fixed exposure (EV) and the adaptation's time
/// constant (seconds: a 6 EV step is within 0.04 EV after 2 s).
pub const DEFAULT_KEY: f32 = 0.8;
pub const EV_CLAMP: f32 = 1.0;
pub const ADAPT_SECONDS: f32 = 0.35;
/// The default settings' share of the bloom mixed into the frame.
pub const DEFAULT_BLOOM_MIX: f32 = 0.04;

/// How the frame's exposure is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExposureMode {
    Fixed(f32),
    Auto,
}

/// The curve from the exposed HDR colour to the display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tonemap {
    /// The neutral hue-keeping shoulder (then sRGB encoded).
    Neutral,
    /// Our AgX-style log + sigmoid (display-encoded by the curve itself).
    AgxStyle,
}

/// What the bloom is fed and how it is put back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BloomSettings {
    /// The soft threshold (exposed luminance) and its knee (0: none, all light enters).
    pub threshold: f32,
    pub knee: f32,
    /// Added times this, or mixed in at this share (`mix`).
    pub strength: f32,
    pub mix: bool,
}

/// The emitters' gains (linear HDR multipliers).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emissive {
    /// Emissive surfaces ([`crate::hd::material::GLOW`]): their colour times this, unlit.
    pub glow: f32,
    /// Self-illuminating (unlit) shape parts.
    pub shape: f32,
    /// Coronas and the sun's flare sprites.
    pub emitter: f32,
}

impl Quality {
    /// The bloom chain's levels.
    pub fn bloom_levels(self) -> u32 {
        match self {
            Quality::Low => 5,
            Quality::Medium => 6,
            Quality::High => 7,
            Quality::Ultra => 8,
        }
    }
}

/// The soft threshold's weight of a luminance: 0 below `threshold - knee`, a quadratic knee, then `(l - threshold) / l` (the share of
/// the colour above the threshold), as the prefilter applies it.
pub fn bloom_weight(l: f32, b: &BloomSettings) -> f32 {
    if b.threshold <= 0.0 {
        return 1.0;
    }
    let l = l.max(1e-6);
    let soft = if b.knee > 0.0 {
        let x = (l - b.threshold + b.knee).clamp(0.0, 2.0 * b.knee);
        x * x / (4.0 * b.knee)
    } else {
        0.0
    };
    soft.max(l - b.threshold).max(0.0) / l
}

/// A histogram bin's centre (log2 luminance).
pub fn bin_ev(bin: usize) -> f32 {
    HISTOGRAM_MIN_EV
        + (bin as f32 + 0.5) / HISTOGRAM_BINS as f32 * (HISTOGRAM_MAX_EV - HISTOGRAM_MIN_EV)
}

/// The bin of a luminance (0 for black: the darkest bin, which the percentile range leaves out).
pub fn bin_of(l: f32) -> usize {
    if l.is_nan() || l <= 0.0 || !l.is_finite() {
        return 0;
    }
    let t = (l.log2() - HISTOGRAM_MIN_EV) / (HISTOGRAM_MAX_EV - HISTOGRAM_MIN_EV);
    ((t * HISTOGRAM_BINS as f32) as i64).clamp(0, HISTOGRAM_BINS as i64 - 1) as usize
}

/// The mean log2 luminance of the bins between the percentiles (each bin weighted by its pixels inside the range).
pub fn histogram_mean_ev(hist: &[u32]) -> f32 {
    let total: f64 = hist.iter().map(|&c| f64::from(c)).sum();
    if total <= 0.0 {
        return 0.0;
    }
    let lo = total * f64::from(LOW_PERCENTILE);
    let hi = total * f64::from(HIGH_PERCENTILE);
    let (mut below, mut sum, mut weight) = (0.0f64, 0.0f64, 0.0f64);
    for (i, &c) in hist.iter().enumerate() {
        let c = f64::from(c);
        let start = below;
        let end = below + c;
        below = end;
        let w = (end.min(hi) - start.max(lo)).max(0.0);
        sum += w * f64::from(bin_ev(i));
        weight += w;
    }
    if weight <= 0.0 {
        return 0.0;
    }
    (sum / weight) as f32
}

/// The exposure (EV: log2 of the multiplier) a histogram asks for: the averaged range to [`DEFAULT_KEY`], clamped.
pub fn target_ev(hist: &[u32]) -> f32 {
    (DEFAULT_KEY.log2() - histogram_mean_ev(hist)).clamp(-EV_CLAMP, EV_CLAMP)
}

/// One frame's adaptation from `ev` toward `target` over `dt` seconds.
pub fn adapt(ev: f32, target: f32, dt: f32) -> f32 {
    let k = 1.0 - (-dt.max(0.0) / ADAPT_SECONDS).exp();
    ev + (target - ev) * k
}

/// The histogram of linear RGB pixels (CPU; the luminance is Rec. 709's).
pub fn histogram(px: impl Iterator<Item = [f32; 3]>) -> Vec<u32> {
    let mut h = vec![0u32; HISTOGRAM_BINS];
    for c in px {
        let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        h[bin_of(l)] += 1;
    }
    h
}

/// The AgX-style formation's constants: the pull toward luminance, the log range about mid grey (EV), the sigmoid's slope and
/// pivot, and the saturation given back.
pub const AGX_INSET: f32 = 0.14;
pub const AGX_MIN_EV: f32 = -11.0;
pub const AGX_MAX_EV: f32 = 7.0;
pub const AGX_SLOPE: f32 = 0.7;
/// Mid grey (0.18) lands on the display's 0.46 (sRGB's encoding of it).
pub const AGX_MID_GREY_DISPLAY: f32 = 0.461;
pub const AGX_OUTSET: f32 = 1.12;

/// Our AgX-style tonemap (CPU form of `tonemap.wgsl`'s): an exposed linear colour to display values 0..1.
pub fn agx_style(c: [f32; 3]) -> [f32; 3] {
    let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let c = c.map(|v| v.max(0.0));
    let l = lum(c);
    agx_style_shaped(c.map(|v| v + (l - v) * AGX_INSET))
}

/// [`agx_style`] after its pull toward the luminance (the grading table holds this part; the tonemap pass's shaper does the
/// pull).
pub fn agx_style_shaped(c: [f32; 3]) -> [f32; 3] {
    agx_style_open(c).map(|v| v.clamp(0.0, 1.0))
}

/// [`agx_style_shaped`] before its last clamp to 0..1 (the grading table stores this, the tonemap pass clamps after the lookup: a
/// clamp inside the table would be interpolated across).
pub fn agx_style_open(c: [f32; 3]) -> [f32; 3] {
    agx_outset(c.map(agx_channel))
}

/// The AgX-style curve's per-channel part: a value's log2 exposure about mid grey (clamped to the range) through the logistic
/// sigmoid, its ends pinned to 0 and 1.
pub fn agx_channel(v: f32) -> f32 {
    let pivot = (1.0 / AGX_MID_GREY_DISPLAY - 1.0).ln() / AGX_SLOPE;
    let ev = (v.max(1e-10) / 0.18).log2().clamp(AGX_MIN_EV, AGX_MAX_EV);
    let s = 1.0 / (1.0 + (-AGX_SLOPE * (ev - pivot)).exp());
    // The ends pinned: the curve's value at the range's bottom is black, at its top white.
    let lo = 1.0 / (1.0 + (-AGX_SLOPE * (AGX_MIN_EV - pivot)).exp());
    let hi = 1.0 / (1.0 + (-AGX_SLOPE * (AGX_MAX_EV - pivot)).exp());
    ((s - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// The AgX-style curve's saturation given back (open: not clamped).
pub fn agx_outset(y: [f32; 3]) -> [f32; 3] {
    let ly = 0.2126 * y[0] + 0.7152 * y[1] + 0.0722 * y[2];
    y.map(|v| ly + (v - ly) * AGX_OUTSET)
}

#[cfg(test)]
#[path = "grade_tests.rs"]
mod tests;
