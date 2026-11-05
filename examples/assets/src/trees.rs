//! The hills' trees: low-poly stylized trees built from brushes into interiors (`.dif`), in forests round the race track.
//!
//! **The kinds** ([`Kind`], [`Kind::height`]): a redwood (about 40 m: a thick, straight red-brown trunk, broad at its foot
//! and tapering, bare to about half its height, under a narrow spire of five-sided tiers stacked one on another, each turned
//! its own way and overhanging the next one's foot), a fir (about 30 m, slenderer, its five-sided tiers from a third of the
//! way up, in a darker green), a pine (a tall bare trunk under a flat crown of three tiers), and under them a young spruce
//! (three tiers from just over a buggy's roof), an oak (a trunk into two faceted blobs, a broad one and a smaller one off its
//! side, higher) and a birch (a slim pale trunk under one tall narrow blob). Each tree has its own size and one of a few
//! greens of its kind; a leafy one leans a little (a conifer stands straight, [`tree`]). A conifer's parts stand [`GAP`]
//! apart rather than cut through each other, so the builder keeps each face whole (it cuts each face by every brush whose
//! box it meets); a blob is the solid behind twelve planes facing the corners of an icosahedron, set out to an ellipsoid
//! and jittered (a lumpy dodecahedron), its faces flat so the sun lights the facets one by one. The trunks reach [`SINK`] m
//! into the ground, so a tree on a slope stands on it all round, and every canopy starts at least [`CLEARANCE`] m over the
//! tree's foot. Every brush is solid (the builder makes one hull a brush): the trunk stops a buggy, the canopy is above it.
//!
//! **The forests** ([`forest`]): patches where a smooth noise over the map ([`PATCH_SCALE`] m across) runs high, thick in
//! their cores and thinning to single young trees at their edges, open meadows between them. The ground pulls them: slopes
//! (the valley's walls, the ridge's flanks, the hillsides) carry more forest, hollows less. They keep [`TRACK_CLEAR`] m off
//! the track's edge and thin out over [`EDGE`] m beyond that; they stay off the start field and its way north over the round
//! hill, and fade out [`REACH_OUT`] m from the track. A second noise sorts conifer stands from leafy ones. Trees stand
//! between [`SPACING`] m apart (in a core) and its upper end (at an edge), and grown crowns at one height no nearer than
//! [`CROWN_OVERLAP`] of their radii (a young tree takes the place under them instead). The forest is cut into squares of
//! [`CHUNK`] m, one interior each ([`chunks`]), built in the world's coordinates (its `InteriorInstance` at the origin).
//! [`lineup`] is each kind in a row (the review pictures' interior, not in the map).
//!
//! The materials ([`BARK`], [`BARK_RED`], [`BARK_BIRCH`], [`PINE`], [`FIR`], [`LEAVES`], [`LEAVES_DARK`], [`LEAVES_LIGHT`])
//! are the kit's textures of the same names, one repeat every [`TEXTURE_SIZE`] m along the world axes.

use std::collections::{BTreeMap, HashMap};
use std::f64::consts::TAU;
use std::io;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use torque_interior_builder::{Brush, Options, Plane, TexGen, V3, build};

use crate::terrain::smoothstep;
use crate::track;

/// The materials: the trunks (brown, and the birches' pale bark), the conifers' needles (a blue-green, a darker one) and the
/// leafy trees' leaves (a leaf green, a deep green and a yellow-green).
pub const BARK: &str = "tree_bark";
pub const BARK_RED: &str = "tree_bark_red";
pub const BARK_BIRCH: &str = "tree_bark_birch";
pub const PINE: &str = "tree_pine";
pub const FIR: &str = "tree_fir";
pub const LEAVES: &str = "tree_leaves";
pub const LEAVES_DARK: &str = "tree_leaves_dark";
pub const LEAVES_LIGHT: &str = "tree_leaves_light";
/// One texture repeat every this many meters.
pub const TEXTURE_SIZE: f64 = 4.0;
/// The space left between a conifer's trunk and tiers: parts that touch or cross would have the builder cut their faces up
/// (it cuts each face by every brush whose box it meets); a few centimeters apart, each face stays whole.
pub const GAP: f64 = 0.03;
/// How far a trunk reaches below the ground at its foot.
pub const SINK: f64 = 1.5;
/// The least height of a canopy over its tree's foot (upright): a buggy with its roll cage passes under.
pub const CLEARANCE: f64 = 2.9;
/// The least distance from a tree's foot to the track's edge, and over how many meters beyond that a forest thins out.
pub const TRACK_CLEAR: f32 = 9.0;
pub const EDGE: f32 = 18.0;
/// The least distance from a tree's foot to the start field's edge, and over how many meters beyond that it thins out.
pub const FIELD_CLEAR: f32 = 14.0;
pub const FIELD_EDGE: f32 = 30.0;
/// The way north over the round hill kept clear: within this of x = 0, from the field to this far north.
pub const NORTH_LANE: (f32, f32) = (16.0, 230.0);
/// The forests fade out between these distances from the track's centerline.
pub const REACH_OUT: (f32, f32) = (90.0, 140.0);
/// The size of the forest patches' noise (m to a lattice cell).
pub const PATCH_SCALE: f32 = 140.0;
/// The least distance between two trees' feet: in a forest's core, and at its edge.
pub const SPACING: (f32, f32) = (5.6, 9.5);
/// The side of the forest's squares, one interior each.
pub const CHUNK: f32 = 128.0;
/// About the most surfaces one interior holds ([`Kind::surfaces`]; a crowded square is cut into quarters, [`chunks`]): its
/// solid BSP takes about a node a surface, and a version-0 interior numbers at most 16,383.
pub const CHUNK_SURFACES: usize = 8_000;
/// The box the forests are placed in (x and y, m).
pub const AREA: ([f32; 2], [f32; 2]) = ([-560.0, -540.0], [540.0, 420.0]);
/// The forests' lightmap texel (m): the world view lights the facets itself, so the lightmaps stay coarse.
pub const LIGHTMAP_TEXEL: f64 = 16.0;

/// A kind of tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Redwood,
    Fir,
    Pine,
    Spruce,
    Oak,
    Birch,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Redwood,
        Kind::Fir,
        Kind::Pine,
        Kind::Spruce,
        Kind::Oak,
        Kind::Birch,
    ];

    /// Whether it is a conifer (tiers of needles) rather than a leafy tree (blobs).
    pub fn conifer(self) -> bool {
        matches!(self, Kind::Redwood | Kind::Fir | Kind::Pine | Kind::Spruce)
    }

    /// Its usual height grown (m): the redwoods and firs tower, the pines stand half as tall, the spruces, oaks and birches
    /// grow under them.
    pub fn height(self) -> f64 {
        match self {
            Kind::Redwood => 40.0,
            Kind::Fir => 30.0,
            Kind::Pine => 16.0,
            Kind::Spruce => 8.5,
            Kind::Oak => 9.0,
            Kind::Birch => 10.0,
        }
    }

    /// About where a tree of `size` has its crown: its radius, and the lowest and highest it reaches over the foot (m). Two
    /// crowns at one height stand at least [`CROWN_OVERLAP`] of their radii together apart: crowns cutting deep into each
    /// other would have the builder cut their faces into many small parts.
    pub fn crown(self, size: f32) -> (f32, f32, f32) {
        let s = size;
        let h = self.height() as f32 * s;
        let clear = CLEARANCE as f32;
        match self {
            Kind::Redwood => (0.105 * h, 0.42 * 0.8 * h, 1.2 * h),
            Kind::Fir => (0.085 * h, 0.33 * 0.85 * h, 1.15 * h),
            Kind::Pine => (0.22 * h, (0.6 * 0.9 * h).max(clear), 1.15 * h),
            Kind::Spruce => (2.4 * s, clear, 1.1 * h),
            Kind::Oak => (4.3 * s, clear, 1.1 * h),
            Kind::Birch => (1.7 * s, clear, 1.1 * h),
        }
    }
}

impl Kind {
    /// About how many surfaces a tree of this kind makes (the leafy ones' blobs cut each other's faces up).
    pub fn surfaces(self) -> usize {
        match self {
            Kind::Redwood | Kind::Fir => 50,
            Kind::Pine | Kind::Spruce => 40,
            Kind::Oak => 60,
            Kind::Birch => 25,
        }
    }
}

/// The share of two crowns' radii together that they stand apart at least, where they reach the same heights.
pub const CROWN_OVERLAP: f32 = 0.75;

/// One tree of the forest: its kind, its foot (on the ground), its size (1 a grown tree) and the seed of its look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tree {
    pub kind: Kind,
    pub foot: [f32; 3],
    pub size: f32,
    pub seed: u64,
}

/// A small fixed-seed random stream (SplitMix64).
#[derive(Debug, Clone)]
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in [0, 1).
    pub(crate) fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub(crate) fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// One of `items`, each as likely as its weight.
    pub(crate) fn pick<T: Copy>(&mut self, items: &[(T, f64)]) -> T {
        let total: f64 = items.iter().map(|i| i.1).sum();
        let mut at = self.unit() * total;
        for &(item, weight) in items {
            if at < weight {
                return item;
            }
            at -= weight;
        }
        items[items.len() - 1].0
    }
}

/// A hash of lattice point (x, y) and `seed` in [0, 1).
fn lattice(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xff_ffff) as f32 / 0x100_0000 as f32
}

/// Smooth value noise at (x, y) (in lattice cells), in [0, 1).
fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (sx, sy) = (smoothstep(0.0, 1.0, x - fx), smoothstep(0.0, 1.0, y - fy));
    let (ix, iy) = (fx as i32, fy as i32);
    let top = lattice(ix, iy, seed) + (lattice(ix + 1, iy, seed) - lattice(ix, iy, seed)) * sx;
    let bottom = lattice(ix, iy + 1, seed)
        + (lattice(ix + 1, iy + 1, seed) - lattice(ix, iy + 1, seed)) * sx;
    top + (bottom - top) * sy
}

/// Three octaves of [`noise`] (each twice as fine and half as strong), in [0, 1).
pub fn fbm(x: f32, y: f32, seed: u32) -> f32 {
    let (mut sum, mut weight, mut amp, mut f) = (0.0, 0.0, 1.0, 1.0);
    for o in 0..3 {
        sum += noise(x * f, y * f, seed.wrapping_add(o)) * amp;
        weight += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / weight
}

fn texgen(n: V3) -> TexGen {
    TexGen::world(n, TEXTURE_SIZE)
}

/// A convex solid in a tree's own space (its foot at the origin, z up): each plane's outward normal and a point on it.
type Solid = Vec<(V3, V3)>;

/// Where a tree stands: its foot and its lean (a rotation about the foot).
struct Frame {
    foot: V3,
    rot: [[f64; 3]; 3],
}

impl Frame {
    /// Standing at `foot`, leaning `lean` radians toward the heading `toward` (radians from +x).
    fn new(foot: V3, lean: f64, toward: f64) -> Frame {
        // About the level axis across the heading (Rodrigues' rotation).
        let k = [-toward.sin(), toward.cos(), 0.0];
        let (s, c) = lean.sin_cos();
        let mut rot = [[0.0; 3]; 3];
        for (i, row) in rot.iter_mut().enumerate() {
            for (j, r) in row.iter_mut().enumerate() {
                let cross = match (i, j) {
                    (0, 1) => -k[2],
                    (0, 2) => k[1],
                    (1, 0) => k[2],
                    (1, 2) => -k[0],
                    (2, 0) => -k[1],
                    (2, 1) => k[0],
                    _ => 0.0,
                };
                *r = if i == j { c } else { 0.0 } + s * cross + (1.0 - c) * k[i] * k[j];
            }
        }
        Frame { foot, rot }
    }

    fn turn(&self, v: V3) -> V3 {
        [0, 1, 2].map(|i| self.rot[i][0] * v[0] + self.rot[i][1] * v[1] + self.rot[i][2] * v[2])
    }

    fn brush(&self, solid: &Solid, material: &str) -> Brush {
        let planes: Vec<Plane> = solid
            .iter()
            .filter_map(|&(n, p)| {
                let q = self.turn(p);
                Plane::new(
                    self.turn(n),
                    [
                        self.foot[0] + q[0],
                        self.foot[1] + q[1],
                        self.foot[2] + q[2],
                    ],
                )
            })
            .collect();
        Brush::from_planes(&planes, material, texgen)
    }
}

/// A frustum round the z axis: `sides` faces from radius (apothem) `r0` at `z0` to `r1` at `z1`, the first face at `turn`.
fn frustum(z0: f64, z1: f64, r0: f64, r1: f64, sides: usize, turn: f64) -> Solid {
    let mut out = Vec::with_capacity(sides + 2);
    for k in 0..sides {
        let a = turn + TAU * k as f64 / sides as f64;
        let (c, s) = (a.cos(), a.sin());
        out.push((
            [c * (z1 - z0), s * (z1 - z0), r0 - r1],
            [c * r0, s * r0, z0],
        ));
    }
    out.push(([0.0, 0.0, -1.0], [0.0, 0.0, z0]));
    out.push(([0.0, 0.0, 1.0], [0.0, 0.0, z1]));
    out
}

/// The corners of an icosahedron (unit).
fn icosahedron() -> [V3; 12] {
    let p = (1.0 + 5f64.sqrt()) / 2.0;
    let l = (1.0 + p * p).sqrt();
    let raw = [
        [-1.0, p, 0.0],
        [1.0, p, 0.0],
        [-1.0, -p, 0.0],
        [1.0, -p, 0.0],
        [0.0, -1.0, p],
        [0.0, 1.0, p],
        [0.0, -1.0, -p],
        [0.0, 1.0, -p],
        [p, 0.0, -1.0],
        [p, 0.0, 1.0],
        [-p, 0.0, -1.0],
        [-p, 0.0, 1.0],
    ];
    raw.map(|v| v.map(|c| c / l))
}

/// The reach of a [`blob`] below its middle at most (its planes jitter out to 1.04 of the ellipsoid).
const BLOB_REACH: f64 = 1.04;

/// A faceted blob round `at`: twelve planes facing the icosahedron's corners (tipped a little, turned by a random angle about
/// z), each at the ellipsoid of `radii`'s reach along it, jittered between 0.92 and [`BLOB_REACH`] of it.
fn blob(at: V3, radii: V3, rng: &mut Rng) -> Solid {
    let turn = rng.range(0.0, TAU);
    let (c, s) = (turn.cos(), turn.sin());
    let tip: f64 = 0.35;
    let (ct, st) = (tip.cos(), tip.sin());
    icosahedron()
        .iter()
        .map(|v| {
            let t = [v[0], v[1] * ct - v[2] * st, v[1] * st + v[2] * ct];
            let n = [t[0] * c - t[1] * s, t[0] * s + t[1] * c, t[2]];
            let reach =
                ((radii[0] * n[0]).powi(2) + (radii[1] * n[1]).powi(2) + (radii[2] * n[2]).powi(2))
                    .sqrt()
                    * rng.range(0.92, BLOB_REACH);
            (n, [0, 1, 2].map(|k| at[k] + n[k] * reach))
        })
        .collect()
}

/// How a stack of conifer tiers is shaped ([`tiers`]).
struct Tiers {
    /// How many tiers.
    count: usize,
    /// The lowest tier's radius, and the share of it the radius loses from the bottom of the stack to its top.
    radius: f64,
    taper: f64,
    /// How far each tier narrows under the next (a share of its radius).
    waist: f64,
    sides: usize,
}

/// A stack of conifer tiers from `bottom` to `top`, each about as tall, the last to a point; each tier overhangs the
/// next one's foot.
fn tiers(bottom: f64, top: f64, shape: &Tiers, rng: &mut Rng) -> Vec<Solid> {
    let n = shape.count;
    let step = (top - bottom) / n as f64;
    // Where each tier starts, jittered by a tenth of a tier.
    let starts: Vec<f64> = (0..n)
        .map(|k| {
            let jitter = if k == 0 { 0.0 } else { rng.range(-0.1, 0.1) };
            bottom + step * (k as f64 + jitter)
        })
        .collect();
    (0..n)
        .map(|k| {
            let r =
                shape.radius * (1.0 - shape.taper * k as f64 / n as f64) * rng.range(0.92, 1.06);
            let (z1, r1) = match starts.get(k + 1) {
                Some(&next) => (next - GAP, r * shape.waist),
                None => (top, 0.07),
            };
            frustum(starts[k], z1, r, r1, shape.sides, rng.range(0.0, TAU))
        })
        .collect()
}

/// A leafy canopy of `blobs` (their middles and radii, round the canopy's middle), raised so its lowest reach is at least
/// [`CLEARANCE`] over the foot and its middle at least `middle`: the solids and the canopy's lowest height.
fn canopy(blobs: &[(V3, V3)], middle: f64, rng: &mut Rng) -> (Vec<Solid>, f64) {
    let lowest = blobs
        .iter()
        .map(|(c, r)| c[2] - r[2] * BLOB_REACH)
        .fold(f64::INFINITY, f64::min);
    let z = middle.max(CLEARANCE - lowest);
    let solids = blobs
        .iter()
        .map(|&(c, r)| blob([c[0], c[1], c[2] + z], r, rng))
        .collect();
    (solids, z + lowest)
}

/// A tree of `kind` standing at `foot` (on the ground), `size` times a grown one, its look (lean, shape, greens) from `seed`.
pub fn tree(kind: Kind, foot: V3, size: f64, seed: u64) -> Vec<Brush> {
    let mut rng = Rng(seed);
    let s = size;
    // The conifers stand straight: a leaning tier's box would reach down into the one below's, and the builder cuts every
    // face by each brush whose box it meets.
    let max_lean = match kind {
        Kind::Redwood | Kind::Fir | Kind::Spruce | Kind::Pine => 0.0,
        Kind::Oak => 4.0,
        Kind::Birch => 5.0,
    };
    let frame = Frame::new(
        foot,
        rng.range(0.0, max_lean).to_radians(),
        rng.range(0.0, TAU),
    );
    let mut out = Vec::new();
    let mut add = |solids: Vec<Solid>, material: &str| {
        out.extend(solids.iter().map(|solid| frame.brush(solid, material)));
    };
    match kind {
        Kind::Redwood => {
            // A tall straight trunk, flared at its foot, bare to about half its height; a narrow spire of tiers above.
            let h = s * rng.range(0.8, 1.2) * kind.height();
            let base = (h * rng.range(0.42, 0.55)).max(CLEARANCE);
            let r0 = 0.028 * h * rng.range(0.9, 1.1);
            add(
                vec![frustum(
                    -SINK,
                    base - GAP,
                    1.3 * r0,
                    0.5 * r0,
                    6,
                    rng.range(0.0, TAU),
                )],
                BARK_RED,
            );
            let shape = Tiers {
                count: (((h - base) / 5.5).round() as usize).clamp(3, 5),
                radius: 0.105 * h * rng.range(0.9, 1.1),
                taper: 0.8,
                waist: 0.55,
                sides: 5,
            };
            let needles = rng.pick(&[(FIR, 0.6), (PINE, 0.4)]);
            add(tiers(base, h, &shape, &mut rng), needles);
        }
        Kind::Fir => {
            // Slenderer than a redwood, its tiers starting lower, in a darker green.
            let h = s * rng.range(0.85, 1.15) * kind.height();
            let base = (h * rng.range(0.33, 0.42)).max(CLEARANCE);
            let r0 = 0.022 * h * rng.range(0.9, 1.1);
            add(
                vec![frustum(
                    -SINK,
                    base - GAP,
                    r0,
                    0.5 * r0,
                    5,
                    rng.range(0.0, TAU),
                )],
                BARK_RED,
            );
            let shape = Tiers {
                count: (((h - base) / 4.6).round() as usize).clamp(3, 5),
                radius: 0.085 * h * rng.range(0.9, 1.1),
                taper: 0.85,
                waist: 0.45,
                sides: 5,
            };
            add(tiers(base, h, &shape, &mut rng), FIR);
        }
        Kind::Spruce => {
            // The young conifer under the tall ones: three tiers from just over a buggy's roof.
            let h = s * rng.range(0.9, 1.1) * kind.height();
            let needles = rng.pick(&[(PINE, 0.65), (FIR, 0.35)]);
            let shape = Tiers {
                count: 3,
                radius: 2.4 * s,
                taper: 0.6,
                waist: 0.36,
                sides: 5,
            };
            let trunk = frustum(
                -SINK,
                CLEARANCE - GAP,
                0.3 * s,
                0.2 * s,
                5,
                rng.range(0.0, TAU),
            );
            add(vec![trunk], BARK);
            add(
                tiers(CLEARANCE, h.max(CLEARANCE + 3.0), &shape, &mut rng),
                needles,
            );
        }
        Kind::Pine => {
            // A tall bare trunk under a flat crown of three tiers.
            let h = s * rng.range(0.9, 1.15) * kind.height();
            let w = h / 10.5;
            let crown = (0.6 * h).max(CLEARANCE);
            let trunk = frustum(
                -SINK,
                crown - GAP,
                0.27 * w,
                0.16 * w,
                5,
                rng.range(0.0, TAU),
            );
            add(vec![trunk], BARK);
            let (a, b) = (crown + 1.2 * w, crown + 2.3 * w);
            let crown = vec![
                frustum(crown, a, 2.3 * w, 1.05 * w, 6, rng.range(0.0, TAU)),
                frustum(a + GAP, b, 1.85 * w, 0.85 * w, 6, rng.range(0.0, TAU)),
                frustum(
                    b + GAP,
                    h.max(b + 1.0),
                    1.25 * w,
                    0.07,
                    6,
                    rng.range(0.0, TAU),
                ),
            ];
            add(crown, PINE);
        }
        Kind::Oak => {
            let j = rng.range(0.9, 1.1) * s;
            // A broad blob and a smaller one off its side and higher.
            let a = rng.range(0.0, TAU);
            let (reach, r) = (rng.range(1.7, 2.2) * j, rng.range(1.5, 1.9) * j);
            let blobs = [
                ([0.0, 0.0, 0.0], [2.7 * j, 2.4 * j, 2.0 * j]),
                (
                    [a.cos() * reach, a.sin() * reach, rng.range(0.6, 1.3) * j],
                    [r, r * 0.95, r * 0.85],
                ),
            ];
            let (solids, low) = canopy(&blobs, 5.4 * s, &mut rng);
            let leaves = rng.pick(&[(LEAVES, 0.45), (LEAVES_DARK, 0.4), (LEAVES_LIGHT, 0.15)]);
            // Up into the main blob's underside.
            let trunk = frustum(
                -SINK,
                low + 0.25 * j,
                0.38 * s,
                0.22 * s,
                6,
                rng.range(0.0, TAU),
            );
            add(vec![trunk], BARK);
            add(solids, leaves);
        }
        Kind::Birch => {
            let h = s * rng.range(0.9, 1.1) * kind.height();
            let blobs = [([0.0, 0.0, 0.0], [1.5 * s, 1.35 * s, 3.0 * s])];
            let (solids, low) = canopy(&blobs, 0.52 * h, &mut rng);
            let leaves = rng.pick(&[(LEAVES_LIGHT, 0.65), (LEAVES, 0.35)]);
            let trunk = frustum(
                -SINK,
                low + 0.3 * s,
                0.18 * s,
                0.1 * s,
                5,
                rng.range(0.0, TAU),
            );
            add(vec![trunk], BARK_BIRCH);
            add(solids, leaves);
        }
    }
    out
}

/// The distance from (x, y) to the track's centerline: exact within [`track::REACH`], else from a grid of 16 m squares.
fn track_distance(x: f32, y: f32) -> f32 {
    const CELL: f32 = 16.0;
    struct Far {
        min: [f32; 2],
        side: [usize; 2],
        d: Vec<f32>,
    }
    static FAR: OnceLock<Far> = OnceLock::new();
    if let Some(near) = track::track().nearest(x, y) {
        return near.distance;
    }
    let far = FAR.get_or_init(|| {
        let (lo, hi) = (AREA.0, AREA.1);
        let side = [0, 1].map(|k| ((hi[k] - lo[k]) / CELL).ceil() as usize + 1);
        let points = track::track().points();
        let d = (0..side[0] * side[1])
            .map(|i| {
                let c = [
                    lo[0] + ((i % side[0]) as f32 + 0.5) * CELL,
                    lo[1] + ((i / side[0]) as f32 + 0.5) * CELL,
                ];
                points
                    .iter()
                    .step_by(2)
                    .map(|p| (p[0] - c[0]).hypot(p[1] - c[1]))
                    .fold(f32::INFINITY, f32::min)
            })
            .collect();
        Far { min: lo, side, d }
    });
    let i = (((x - far.min[0]) / CELL).floor().max(0.0) as usize).min(far.side[0] - 1);
    let j = (((y - far.min[1]) / CELL).floor().max(0.0) as usize).min(far.side[1] - 1);
    far.d[i + j * far.side[0]].max(track::REACH)
}

/// Whether a tree's foot may stand at (x, y): off the track, the start field and the way north over the hill.
pub fn allowed(x: f32, y: f32) -> bool {
    let field = x.hypot(y) >= crate::terrain::FIELD_RADIUS + FIELD_CLEAR;
    let lane = x.abs() < NORTH_LANE.0 && y > 0.0 && y < NORTH_LANE.1;
    let off_track = track::track()
        .nearest(x, y)
        .is_none_or(|n| n.distance >= track::HALF_WIDTH + TRACK_CLEAR);
    field && !lane && off_track
}

/// How much forest there is at (x, y), 0 (a meadow, the track, the field) to 1 (a forest's core), and the ground's relief
/// there (its height over the ring 36 m round it: above zero on a rise, below in a hollow).
pub fn density(x: f32, y: f32) -> (f32, f32) {
    if !allowed(x, y) {
        return (0.0, 0.0);
    }
    let d = track_distance(x, y);
    let r = x.hypot(y);
    let field_edge = crate::terrain::FIELD_RADIUS + FIELD_CLEAR;
    let gate = smoothstep(
        track::HALF_WIDTH + TRACK_CLEAR,
        track::HALF_WIDTH + TRACK_CLEAR + EDGE,
        d,
    ) * (1.0 - smoothstep(REACH_OUT.0, REACH_OUT.1, d))
        * smoothstep(field_edge, field_edge + FIELD_EDGE, r);
    let patch = fbm(x / PATCH_SCALE, y / PATCH_SCALE, 5);
    // The ground's pull adds at most 0.17 below.
    if gate <= 0.0 || patch < 0.33 {
        return (0.0, 0.0);
    }
    let h = |x: f32, y: f32| crate::terrain::height(x, y);
    let here = h(x, y);
    let ring = 0.25 * (h(x + 36.0, y) + h(x - 36.0, y) + h(x, y + 36.0) + h(x, y - 36.0));
    let relief = here - ring;
    let slope = (h(x + 4.0, y) - h(x - 4.0, y)).hypot(h(x, y + 4.0) - h(x, y - 4.0)) / 8.0;
    let value =
        patch + 0.07 * (relief / 2.0).clamp(-1.0, 1.0) + 0.10 * smoothstep(0.05, 0.18, slope);
    (smoothstep(0.53, 0.66, value) * gate, relief)
}

/// The forest, placed once: candidates on a jittered grid over [`AREA`], each kept as likely as the [`density`] there and
/// only clear of the trees before it by [`SPACING`] (closer in a core).
pub fn forest() -> &'static [Tree] {
    static FOREST: OnceLock<Vec<Tree>> = OnceLock::new();
    FOREST.get_or_init(|| {
        const STEP: f32 = 2.6;
        const CELL: f32 = 10.0;
        let mut rng = Rng(0x0f0_7e57);
        let mut out: Vec<Tree> = Vec::new();
        let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        // Each tree's crown: its radius and the heights it spans over the foot ([`Kind::crown`]).
        let mut crowns: Vec<(f32, f32, f32)> = Vec::new();
        let cell = |x: f32, y: f32| ((x / CELL).floor() as i32, (y / CELL).floor() as i32);
        let (lo, hi) = AREA;
        let (nx, ny) = (
            ((hi[0] - lo[0]) / STEP) as usize,
            ((hi[1] - lo[1]) / STEP) as usize,
        );
        for j in 0..ny {
            for i in 0..nx {
                let x = lo[0] + (i as f32 + rng.unit() as f32) * STEP;
                let y = lo[1] + (j as f32 + rng.unit() as f32) * STEP;
                let keep = rng.unit() as f32;
                let (dens, relief) = density(x, y);
                if dens <= 0.0 || keep > dens.powf(0.8) {
                    continue;
                }
                let spacing = SPACING.1 + (SPACING.0 - SPACING.1) * dens;
                // Conifer stands and leafy ones, conifers liking the rises.
                let stand = fbm(x / 230.0 + 17.3, y / 230.0 - 9.1, 41)
                    + 0.08 * (relief / 2.0).clamp(-1.0, 1.0);
                let conifers = f64::from(smoothstep(0.3, 0.55, stand)) > rng.unit();
                let kind = if conifers {
                    // The tall ones in the cores; young spruces under them and at the edges.
                    let young = 0.15 + 0.45 * f64::from(1.0 - dens);
                    rng.pick(&[
                        (Kind::Redwood, 0.55 * (1.0 - young)),
                        (Kind::Fir, 0.3 * (1.0 - young)),
                        (Kind::Pine, 0.15 * (1.0 - young)),
                        (Kind::Spruce, young),
                    ])
                } else if rng.unit() < 0.2 + 0.35 * f64::from(1.0 - dens) {
                    Kind::Birch
                } else {
                    Kind::Oak
                };
                // Grown in the cores, younger toward the edges (the tall ones' skyline steps down to the edge).
                let size = (0.6 + 0.45 * dens) * rng.range(0.85, 1.15) as f32;
                // Where a grown tree's crown would crowd another's, a young one under the crowns instead.
                let young = if conifers { Kind::Spruce } else { Kind::Birch };
                let (cx, cy) = cell(x, y);
                let fits = |kind: Kind, size: f32| {
                    let c = kind.crown(size);
                    (cx - 2..=cx + 2).all(|a| {
                        (cy - 2..=cy + 2).all(|b| {
                            cells.get(&(a, b)).is_none_or(|v| {
                                v.iter().all(|&k| {
                                    let d = (out[k].foot[0] - x).hypot(out[k].foot[1] - y);
                                    let o = crowns[k];
                                    let apart = o.1 > c.2 || c.1 > o.2;
                                    d >= spacing && (apart || d >= CROWN_OVERLAP * (o.0 + c.0))
                                })
                            })
                        })
                    })
                };
                // (Half the time: the understory stays open enough to see into.)
                let understory = rng.unit() < 0.5;
                let Some((kind, size)) = [(kind, size), (young, size.min(0.7))]
                    .into_iter()
                    .take(1 + usize::from(understory))
                    .find(|&(k, s)| fits(k, s))
                else {
                    continue;
                };
                cells.entry((cx, cy)).or_default().push(out.len());
                crowns.push(kind.crown(size));
                out.push(Tree {
                    kind,
                    foot: [x, y, crate::terrain::height(x, y)],
                    size,
                    seed: rng.next(),
                });
            }
        }
        out
    })
}

/// One square of the forest: its interior's file name and its trees (indices into [`forest`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub file: String,
    pub trees: Vec<usize>,
}

/// The forest's squares that have trees: [`CHUNK`] m a side, a square of more than [`CHUNK_SURFACES`] cut into quarters
/// (and those again) so no interior passes a version-0 interior's counts; in order of their rows (south to north) and
/// columns, their files `forest00.dif` on (the map's `InteriorInstance`s name them under `kit/`).
pub fn chunks() -> &'static [Chunk] {
    static CHUNKS: OnceLock<Vec<Chunk>> = OnceLock::new();
    CHUNKS.get_or_init(|| chunks_of(forest(), "forest"))
}

/// The squares of the trees `all` as [`chunks`] cuts the forest, their files `<prefix>00.dif` on.
pub fn chunks_of(all: &[Tree], prefix: &str) -> Vec<Chunk> {
    let mut squares: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
    for (k, t) in all.iter().enumerate() {
        let key = (
            (t.foot[1] / CHUNK).floor() as i32,
            (t.foot[0] / CHUNK).floor() as i32,
        );
        squares.entry(key).or_default().push(k);
    }
    // Quarters of a crowded square, about its middle, until each holds few enough.
    fn split(
        trees: Vec<usize>,
        middle: [f32; 2],
        half: f32,
        all: &[Tree],
        out: &mut Vec<Vec<usize>>,
    ) {
        if trees.iter().map(|&k| all[k].kind.surfaces()).sum::<usize>() <= CHUNK_SURFACES {
            out.push(trees);
            return;
        }
        let mut quarters: [Vec<usize>; 4] = Default::default();
        for k in trees {
            let f = all[k].foot;
            quarters[usize::from(f[0] >= middle[0]) + 2 * usize::from(f[1] >= middle[1])].push(k);
        }
        for (q, trees) in quarters.into_iter().enumerate() {
            let h = half / 2.0;
            let m = [
                middle[0] + if q % 2 == 1 { h } else { -h },
                middle[1] + if q / 2 == 1 { h } else { -h },
            ];
            if !trees.is_empty() {
                split(trees, m, h, all, out);
            }
        }
    }
    let mut pieces = Vec::new();
    for ((row, column), trees) in squares {
        let middle = [(column as f32 + 0.5) * CHUNK, (row as f32 + 0.5) * CHUNK];
        split(trees, middle, CHUNK / 2.0, all, &mut pieces);
    }
    pieces
        .into_iter()
        .enumerate()
        .map(|(k, trees)| Chunk {
            file: format!("{prefix}{k:02}.dif"),
            trees,
        })
        .collect()
}

/// How the trees' interiors are built: the coarse lightmaps ([`LIGHTMAP_TEXEL`]).
pub fn options() -> Options {
    Options {
        lightmap_texel: LIGHTMAP_TEXEL,
        ..Options::default()
    }
}

fn interior(brushes: &[Brush]) -> io::Result<Vec<u8>> {
    build(brushes, &options())
        .and_then(|b| b.bytes())
        .map_err(io::Error::other)
}

/// Square `index`'s brushes (in the world's coordinates).
pub fn chunk_brushes(index: usize) -> Vec<Brush> {
    brushes_of(forest(), &chunks()[index])
}

/// The brushes of `chunk`'s trees of `all` (in the world's coordinates).
pub fn brushes_of(all: &[Tree], chunk: &Chunk) -> Vec<Brush> {
    chunk
        .trees
        .iter()
        .flat_map(|&k| {
            let t = all[k];
            tree(t.kind, t.foot.map(f64::from), f64::from(t.size), t.seed)
        })
        .collect()
}

/// Every square as a `.dif` with its file name, built once (the kit is made many times in a run of the tests).
pub fn chunk_difs() -> io::Result<Vec<(&'static str, Vec<u8>)>> {
    static DIFS: OnceLock<Result<Vec<Vec<u8>>, String>> = OnceLock::new();
    let difs = DIFS.get_or_init(|| build_chunks(forest(), chunks()));
    let difs = difs.as_ref().map_err(|e| io::Error::other(e.clone()))?;
    Ok(chunks()
        .iter()
        .zip(difs)
        .map(|(c, d)| (c.file.as_str(), d.clone()))
        .collect())
}

/// The `.dif` of each of `chunks` (of the trees `all`), built on a few threads, in order.
pub fn build_chunks(all: &[Tree], chunks: &[Chunk]) -> Result<Vec<Vec<u8>>, String> {
    let n = chunks.len();
    let workers = std::thread::available_parallelism().map_or(1, |w| w.get().min(4));
    let next = AtomicUsize::new(0);
    let mut built: Vec<(usize, io::Result<Vec<u8>>)> = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        if k >= n {
                            return done;
                        }
                        done.push((k, interior(&brushes_of(all, &chunks[k]))));
                    }
                })
            })
            .collect();
        jobs.into_iter()
            .flat_map(|j| j.join().unwrap_or_default())
            .collect()
    });
    if built.len() != n {
        return Err("a forest square's build panicked".to_string());
    }
    built.sort_by_key(|b| b.0);
    built
        .into_iter()
        .map(|(_, b)| b.map_err(|e| e.to_string()))
        .collect()
}

/// The lineup's file.
pub const LINEUP_FILE: &str = "tree_lineup.dif";

/// The lineup's brushes: each kind grown ([`Kind::ALL`]'s order) 12 m apart along x, and a younger one of each 12 m behind,
/// their feet at height 0.
pub fn lineup() -> Vec<Brush> {
    let mut out = Vec::new();
    for (k, &kind) in Kind::ALL.iter().enumerate() {
        let x = -30.0 + 12.0 * k as f64;
        out.extend(tree(kind, [x, 0.0, 0.0], 1.0, 0x11e_0000 + k as u64));
        out.extend(tree(kind, [x + 4.0, 12.0, 0.0], 0.7, 0x11e_1000 + k as u64));
    }
    out
}

/// The lineup as a `.dif`.
pub fn lineup_dif() -> io::Result<Vec<u8>> {
    interior(&lineup())
}
