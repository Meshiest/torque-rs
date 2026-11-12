//! The engine protocol's screen-space distortion block ([`DistortionExt`]): the fields that make a shape (`ShapeBaseData` and the
//! classes built on it) or a decal (`DecalData`) bend the picture behind it, sent only on a connection whose
//! [`NetWidths::extensions`](crate::codec::NetWidths::extensions) is set.
//!
//! ```text
//! flag(distortion != defaults): F32 distortion, distortionEdge; string distortionMap; F32 distortionMapScale;
//!                               F32 x2 distortionScroll; F32 x4 distortionTint; F32 distortionChroma; flag distortionOnly
//! ```
//!
//! `distortion` is the bend's strength (world units, either sign), `distortionEdge` gathers it toward the silhouette,
//! `distortionMap` names a normal map whose `xy` adds to it (repeated `distortionMapScale` times a texture unit, moving
//! `distortionScroll` units a second), `distortionTint` multiplies what is seen through it (`rgb`, by `a` of the way),
//! `distortionChroma` splits red from blue by that share of the bend, and `distortionOnly` draws the shape as the bend alone (a
//! heat haze, a shockwave: its own surfaces are not drawn). A reader sanitizes every value ([`DistortionExt::sanitize`]) and the
//! writer the same, so a record on the wire is one a reader keeps as it is.

use super::{float, present};
use crate::codec::{Codec, F32, Layout, R};
use crate::{ObjectView, boolean, raw, text};

/// The range of `distortion` (world units, either way).
pub const MAX_STRENGTH: f32 = 4.0;
/// The most `distortionEdge`.
pub const MAX_EDGE: f32 = 16.0;
/// The range of `distortionMapScale` (repeats a texture unit).
pub const MAP_SCALE_RANGE: (f32, f32) = (0.01, 100.0);
/// The most `distortionScroll` a side (texture units a second).
pub const MAX_SCROLL: f32 = 100.0;

/// The fields' defaults that are not 0 or false, as script defaults are written (`ClassSpec::defaults`).
pub const DISTORTION_DEFAULTS: &[(&str, &str)] =
    &[("distortionMapScale", "1"), ("distortionTint", "1 1 1 0")];

/// The block (its defaults change nothing: no bend, no tint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistortionExt {
    pub strength: F32,
    pub edge: F32,
    pub map: Vec<u8>,
    pub map_scale: F32,
    pub scroll: [F32; 2],
    pub tint: [F32; 4],
    pub chroma: F32,
    pub only: bool,
}

impl Default for DistortionExt {
    fn default() -> DistortionExt {
        DistortionExt {
            strength: F32::new(0.0),
            edge: F32::new(0.0),
            map: Vec::new(),
            map_scale: F32::new(1.0),
            scroll: [F32::new(0.0); 2],
            tint: [F32::new(1.0), F32::new(1.0), F32::new(1.0), F32::new(0.0)],
            chroma: F32::new(0.0),
            only: false,
        }
    }
}

/// `v` held to `lo..=hi`, `default` when it is not finite.
fn held(v: F32, default: f32, lo: f32, hi: f32) -> F32 {
    let x = v.get();
    F32::new(if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        default
    })
}

impl DistortionExt {
    /// Every value in range: a float that is not finite is its default, any other is held to its range.
    #[must_use]
    pub fn sanitize(mut self) -> DistortionExt {
        self.strength = held(self.strength, 0.0, -MAX_STRENGTH, MAX_STRENGTH);
        self.edge = held(self.edge, 0.0, 0.0, MAX_EDGE);
        self.map_scale = held(self.map_scale, 1.0, MAP_SCALE_RANGE.0, MAP_SCALE_RANGE.1);
        self.scroll = self.scroll.map(|s| held(s, 0.0, -MAX_SCROLL, MAX_SCROLL));
        self.tint = self.tint.map(|c| held(c, 1.0, 0.0, 1.0));
        self.chroma = held(self.chroma, 0.0, 0.0, 1.0);
        self
    }

    /// Whether the datablock bends: `distortion` is not 0. The other fields only shape a bend; a tint alone is none (a class
    /// whose script defaults do not reach the fields reads a `ColorF`'s zero, an opaque black tint).
    pub fn bends(&self) -> bool {
        self.strength.get() != 0.0
    }

    /// The block a datablock's fields give, sanitized; None when it does not bend ([`DistortionExt::bends`]). A
    /// `distortionMapScale` of 0 (a field never set) reads as 1.
    pub fn pack(v: &dyn ObjectView) -> Option<DistortionExt> {
        let d = DistortionExt::default();
        let or = |name: &str, i: usize, default: F32| {
            if present(v, name, i) {
                raw(v, name, i)
            } else {
                default
            }
        };
        let e = DistortionExt {
            strength: or("distortion", 0, d.strength),
            edge: or("distortionEdge", 0, d.edge),
            map: text(v, "distortionMap", 0),
            map_scale: Some(or("distortionMapScale", 0, d.map_scale))
                .filter(|s| s.get() != 0.0)
                .unwrap_or(d.map_scale),
            scroll: [0, 1].map(|i| or("distortionScroll", i, d.scroll[i])),
            tint: [0, 1, 2, 3].map(|i| or("distortionTint", i, d.tint[i])),
            chroma: or("distortionChroma", 0, d.chroma),
            only: boolean(v, "distortionOnly", 0),
        }
        .sanitize();
        e.bends().then_some(e)
    }
}

impl Layout for DistortionExt {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        // What is written is in range, and so is what is read.
        *self = std::mem::take(self).sanitize();
        float(c, "distortion", &mut self.strength)?;
        float(c, "distortionEdge", &mut self.edge)?;
        c.string("distortionMap", &mut self.map)?;
        float(c, "distortionMapScale", &mut self.map_scale)?;
        c.floats("distortionScroll", &mut self.scroll)?;
        c.floats("distortionTint", &mut self.tint)?;
        float(c, "distortionChroma", &mut self.chroma)?;
        c.flag("distortionOnly", &mut self.only)?;
        *self = std::mem::take(self).sanitize();
        Ok(())
    }
}
