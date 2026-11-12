//! Screen-space distortion: surfaces that bend the picture behind them (glass, water, heat haze, a force field's bubble, a
//! shockwave's growing sphere). A frame's [`HdDistorter`]s ([`super::HdScene::distorters`]) are triangles in the world with a
//! [`DistortionLook`]; the enhanced renderer draws them into a buffer of screen offsets and moves the anti-aliased picture's
//! pixels by them (`torque_render_hd::distortion`).
//!
//! **The bend.** At each pixel of a distorter the picture is read from a point moved by `strength` world units along the
//! surface's normal as the eye sees it (its screen part, pointing in: a convex surface magnifies what is behind; a negative
//! strength shrinks it) plus the normal map's `xy` ([`DistortionLook::map`], scrolled over time), projected by the surface's
//! distance (a far window bends fewer pixels than a near one) and held to [`MAX_SCREEN_OFFSET`] of the frame. `edge` above 0
//! moves the bend toward the silhouette (`(1 - n.v)^edge`: a shockwave's ring); every distorter fades out at its silhouette, so
//! no edge is sharp. The vertex color's alpha scales it (a shockwave fading as it grows), the tint multiplies what is seen
//! through it, and `chroma` reads red and blue that share of the bend farther and nearer (a prism's fringe).
//!
//! **What it bends.** Only what is behind it: what stands in front of a distorter hides it, and a moved read that lands on
//! something nearer than the distorter is pulled back (to half, then to none), so a pole in front of a heat haze is not smeared
//! into it. A draw with a back-face [`Cull`] bends with its front faces only (a closed shape: the sphere's far side would undo
//! its near side); [`Cull::None`] bends with both sides (a card).
//!
//! **Making them.** [`sphere`] is a shockwave's or a bubble's mesh, [`HdDistorter::new`] makes one of any draw, and
//! [`HdDistorter::from_object`] one per draw of a moving shape (its vertices placed in the world). Their bounding spheres are
//! what the renderer culls: a frame with none in sight costs nothing.

use glam::Vec3;

use crate::hd::origin::Shift;
use crate::hd::{DynamicObject, HdDraw, HdVertex, oct_encode};
use crate::{Blend, Cull, Sampler};

/// The most a distorter moves a read, as a share of the frame's height.
pub const MAX_SCREEN_OFFSET: f32 = 0.08;
/// The range of [`DistortionLook::strength`] (world units, either way).
pub const MAX_STRENGTH: f32 = 4.0;
/// The most [`DistortionLook::edge`].
pub const MAX_EDGE: f32 = 16.0;
/// The range of [`DistortionLook::map_scale`] (repeats a texture unit).
pub const MAP_SCALE_RANGE: (f32, f32) = (0.01, 100.0);
/// The most [`DistortionLook::scroll`] a side (texture units a second).
pub const MAX_SCROLL: f32 = 100.0;
/// The most distorters a frame draws (the nearest), and the most vertices and indices they hold together: a server's ghosts
/// decide how many shapes bend, so the frame's buffers stay far under any device's largest ([`DistortionBudget`]).
pub const MAX_DISTORTERS: usize = 256;
pub const MAX_VERTICES: usize = 1 << 20;
pub const MAX_INDICES: usize = 3 << 20;

/// What is left of a frame's distorter caps ([`MAX_DISTORTERS`], [`MAX_VERTICES`], [`MAX_INDICES`]). Callers offer the
/// candidates nearest first; one that does not fit is left out and the next is tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DistortionBudget {
    distorters: usize,
    vertices: usize,
    indices: usize,
}

impl Default for DistortionBudget {
    fn default() -> DistortionBudget {
        DistortionBudget {
            distorters: MAX_DISTORTERS,
            vertices: MAX_VERTICES,
            indices: MAX_INDICES,
        }
    }
}

impl DistortionBudget {
    /// Whether `count` distorters of `vertices` and `indices` fit, taking them from the budget when they do.
    pub fn take(&mut self, count: usize, vertices: usize, indices: usize) -> bool {
        let fits = count <= self.distorters && vertices <= self.vertices && indices <= self.indices;
        if fits {
            self.distorters -= count;
            self.vertices -= vertices;
            self.indices -= indices;
        }
        fits
    }

    /// Whether no distorter fits any more.
    pub fn is_spent(&self) -> bool {
        self.distorters == 0
    }
}

/// The order to offer distorters to a [`DistortionBudget`] in: by the distance from `eye` to the nearest point of each sphere
/// (`centre`, `radius`) of `spheres`, nearest first (ties in their order).
pub fn nearest_first(eye: [f32; 3], spheres: impl Iterator<Item = ([f32; 3], f32)>) -> Vec<usize> {
    let eye = Vec3::from(eye);
    let mut keyed: Vec<(f32, usize)> = spheres
        .enumerate()
        .map(|(i, (c, r))| ((Vec3::from(c).distance(eye) - r).max(0.0), i))
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    keyed.into_iter().map(|(_, i)| i).collect()
}

/// How a distorter bends what is behind it (the module's header has the rules).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistortionLook {
    /// World units a read moves along the surface's eye-space normal (negative: outward).
    pub strength: f32,
    /// 0: the whole surface bends alike; above 0 the bend gathers toward the silhouette as `(1 - n.v)^edge`.
    pub edge: f32,
    /// A normal map (the scene's texture table) whose `xy` (`rg * 2 - 1`) adds to the bend.
    pub map: Option<(usize, Sampler)>,
    /// The map's repeats a texture unit of the surface.
    pub map_scale: f32,
    /// The map's motion (texture units a second).
    pub scroll: [f32; 2],
    /// What is seen through it is multiplied by `rgb`, by `a` of the way (0: no tint).
    pub tint: [f32; 4],
    /// The share of the bend red is read farther and blue nearer (0..1).
    pub chroma: f32,
}

impl Default for DistortionLook {
    fn default() -> DistortionLook {
        DistortionLook {
            strength: 0.0,
            edge: 0.0,
            map: None,
            map_scale: 1.0,
            scroll: [0.0; 2],
            tint: [1.0, 1.0, 1.0, 0.0],
            chroma: 0.0,
        }
    }
}

/// `v` held to `lo..=hi`, `default` when it is not finite.
fn held(v: f32, default: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        default
    }
}

impl DistortionLook {
    /// Every value in range: a value that is not finite is its default, any other is held to its range.
    #[must_use]
    pub fn sanitize(mut self) -> DistortionLook {
        self.strength = held(self.strength, 0.0, -MAX_STRENGTH, MAX_STRENGTH);
        self.edge = held(self.edge, 0.0, 0.0, MAX_EDGE);
        self.map_scale = held(self.map_scale, 1.0, MAP_SCALE_RANGE.0, MAP_SCALE_RANGE.1);
        self.scroll = self.scroll.map(|s| held(s, 0.0, -MAX_SCROLL, MAX_SCROLL));
        self.tint = self.tint.map(|c| held(c, 1.0, 0.0, 1.0));
        self.chroma = held(self.chroma, 0.0, 0.0, 1.0);
        self
    }

    /// Whether it changes the picture at all (a bend or a tint).
    pub fn is_seen(&self) -> bool {
        self.strength != 0.0 || self.tint[3] > 0.0
    }
}

/// A surface that bends the picture behind it: its triangles in the world, its look and its bounding sphere.
#[derive(Debug, Clone, PartialEq)]
pub struct HdDistorter {
    /// The triangles (world space; their normals, texture coordinates and vertex color alpha are read, the rest of the draw's
    /// state but its cull is not).
    pub draw: HdDraw,
    pub look: DistortionLook,
    pub centre: [f32; 3],
    pub radius: f32,
}

impl HdDistorter {
    /// A distorter of `draw`'s triangles (their bounding sphere worked out), its look made in range.
    pub fn new(draw: HdDraw, look: DistortionLook) -> HdDistorter {
        let (centre, radius) = bounds(&draw.vertices);
        HdDistorter {
            draw,
            look: look.sanitize(),
            centre,
            radius,
        }
    }

    /// A distorter for each of `object`'s draws (their vertices placed in the world as the vertex shaders place them).
    pub fn from_object(object: &DynamicObject, look: DistortionLook) -> Vec<HdDistorter> {
        // The distorters own their triangles: an object placed afresh gives its copies up, one in the world is copied.
        let draws = if object.nodes.is_empty() {
            object.draws.clone()
        } else {
            object.placed().draws
        };
        draws
            .into_iter()
            .filter(|d| !d.indices.is_empty())
            .map(|d| HdDistorter::new(d, look))
            .collect()
    }

    /// Whether the renderer has anything to draw of it.
    pub fn is_drawn(&self) -> bool {
        self.look.is_seen() && !self.draw.indices.is_empty() && self.radius.is_finite()
    }
}

impl Shift for HdDistorter {
    fn shift(&mut self, by: [f32; 3]) {
        let HdDistorter {
            draw,
            look: _,
            centre,
            radius: _,
        } = self;
        draw.shift(by);
        centre.shift(by);
    }
}

/// The centre of the box of `vertices` and the radius that holds them (a point at the origin for none).
fn bounds(vertices: &[HdVertex]) -> ([f32; 3], f32) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for v in vertices {
        for k in 0..3 {
            lo[k] = lo[k].min(v.position[k]);
            hi[k] = hi[k].max(v.position[k]);
        }
    }
    if !(lo[0] <= hi[0] && lo[1] <= hi[1] && lo[2] <= hi[2]) {
        return ([0.0; 3], 0.0);
    }
    let c = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
    let r = vertices
        .iter()
        .map(|v| {
            let d = [0, 1, 2].map(|k| v.position[k] - c[k]);
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .fold(0.0f32, f32::max);
    (c, r)
}

/// A sphere's mesh at `centre` of `radius` in the world: `rings` bands from pole to pole of `segments` quads around, normals
/// out, texture coordinates around (u) and down (v), the vertex color white at `alpha` (the bend's share), its back faces
/// culled (it bends with its near side only).
pub fn sphere(centre: [f32; 3], radius: f32, rings: u32, segments: u32, alpha: u8) -> HdDraw {
    let (rings, segments) = (rings.max(2), segments.max(3));
    let mut d = HdDraw::empty();
    d.label = "distortion sphere".to_string();
    d.blend = Blend::Alpha;
    d.cull = Cull::BackCcw;
    d.depth_write = false;
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let theta = v * std::f32::consts::PI;
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let phi = u * std::f32::consts::TAU;
            let n = [
                theta.sin() * phi.cos(),
                theta.sin() * phi.sin(),
                theta.cos(),
            ];
            d.vertices.push(HdVertex {
                position: [0, 1, 2].map(|k| centre[k] + radius * n[k]),
                normal: oct_encode(n),
                uv: [u, v],
                color: [255, 255, 255, alpha],
                material: 0,
                extra: 0,
            });
        }
    }
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * row + s;
            let (b, c, e) = (a + 1, a + row, a + row + 1);
            d.indices.extend_from_slice(&[a, c, b, b, c, e]);
        }
    }
    d
}

#[cfg(test)]
#[path = "distortion_tests.rs"]
mod tests;
