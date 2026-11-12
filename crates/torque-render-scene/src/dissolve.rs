//! Shapes dissolving away or materializing, run by the client ([`Dissolves`]): each frame the share of a shape's surface
//! eaten by its clock, the noise threshold the renderer clips at ([`HdDissolve`]), and the particles its front emits from the
//! surface where it crossed since the last frame.
//!
//! - **Samples**: the first frame a dissolving object is drawn, points over its triangles (each triangle's share by its area,
//!   [`Look::density`] a square unit, [`MIN_SAMPLES`] to [`MAX_SAMPLES`]) in the object's own space, each with its normal, its
//!   color's place in the object's palette and its field ([`field`]); kept sorted by the field.
//! - **The threshold** is the share's quantile of the samples' fields: the surface drawn (the field at or over it) is `1 - eaten`
//!   of the samples, so of the surface's area, whatever the noise's spread; the edge glows up to the quantile of the share plus
//!   [`Look::edge_share`].
//! - **The front**: a frame emits the samples whose field lies between the last frame's threshold and this one's, so the count
//!   tracks the area the front swept; at most [`MAX_SPAWNS_PER_OBJECT`] an object and [`MAX_SPAWNS_PER_FRAME`] in all, thinned
//!   evenly over the crossed samples. A sample is placed by the object's transform now, ejected along its normal turned out
//!   from the object's upright axis ([`SPILL`]: sand spills off the body rather than through it); a surface
//!   whose density asks for more particles than [`MAX_SAMPLES`] makes [`Surface::grains`] of them a sample, spread about it
//!   (fine sand: many small grains).
//! - **Colors**: with [`Look::tint`] each palette color (a draw's vertex color times its texture's mean; at most
//!   [`MAX_PALETTE`]) has its own emitter, its particles' color keys times the color.
//! - **Caps**: [`MAX_OBJECTS`] dissolves; a dissolve whose object is not drawn keeps its clock and emits nothing.
//! - **Work a frame** (a server may start dissolves on every ghost every tick): at most [`MAX_SAMPLINGS_PER_FRAME`] surfaces
//!   are sampled a frame (an object waiting for its turn is drawn whole under half eaten and left out over half, as a renderer
//!   without the clip draws it); a restart keeps its samples while the object's shape and the look's cell, sweep and density
//!   are the same ([`surface_key`]); a ghost restarts at most once every [`MIN_RESTART_MS`] (a newer serial waits for the next
//!   sync); a surface of more than [`MAX_TRIANGLES`] triangles is sampled from an even stride of them; a texture's mean color
//!   is worked out once ([`TextureMeans`]) from at most [`MEAN_TEXELS`] of its texels.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

use ghost_world::tge::{TgeClientSet, shape_part};
use ghost_world::world::ClientWorld;
use ghosts::shapebase::DissolveField;
use ghosts::tge::TgeRecord;

use crate::Draw;
use crate::Texture;
use crate::hd::DynamicObject;
use crate::hd::dissolve::{GONE, HdDissolve, MIN_CELL, WHOLE, field};
use crate::particles::{Emitter, Rng, SpriteFrame};
use crate::textures::TextureSet;

/// The most dissolves the client runs.
pub const MAX_OBJECTS: usize = 64;
/// The fewest and most surface samples an object takes.
pub const MIN_SAMPLES: usize = 16;
pub const MAX_SAMPLES: usize = 16_384;
/// The most grains one sample stands for (a surface whose density asks for more particles than its samples).
pub const MAX_GRAINS_PER_SAMPLE: u32 = 16;
/// The most particles an object's front makes a frame, and all the fronts together.
pub const MAX_SPAWNS_PER_OBJECT: usize = 4096;
pub const MAX_SPAWNS_PER_FRAME: usize = 8192;
/// The most surfaces sampled a frame, the fewest milliseconds between two starts of one ghost's dissolve, the most triangles a
/// sampling walks, and the most texels a texture's mean reads.
pub const MAX_SAMPLINGS_PER_FRAME: usize = 2;
pub const MIN_RESTART_MS: f64 = 250.0;
pub const MAX_TRIANGLES: usize = 65_536;
pub const MEAN_TEXELS: usize = 4096;
/// The most colors an object's particles take ([`Look::tint`]).
pub const MAX_PALETTE: usize = 8;
/// The longest dissolve (ms): what the wire carries (`DissolveField`'s 20 bits), as the server holds it.
pub const MAX_DURATION_MS: u32 = DissolveField::max_of(ghosts::shapebase::dissolve_bits::MS);
/// The most particles a square unit's samples ask for.
pub const MAX_DENSITY: f32 =
    DissolveField::max_of(ghosts::shapebase::dissolve_bits::DENSITY) as f32;

/// Which way a dissolve runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// From whole to gone.
    #[default]
    Out,
    /// From gone to whole (materialize).
    In,
}

/// How a dissolve looks and runs (what the server sent, or a host's own).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub direction: Direction,
    pub duration_ms: u32,
    /// The edge's color (0..1, as a material's) and its glow (the color's multiple in light).
    pub edge_color: [f32; 3],
    pub glow: f32,
    /// The share of the surface the glowing edge covers (0..1).
    pub edge_share: f32,
    /// The `ParticleEmitterData` the front emits (None: no particles).
    pub emitter: Option<u32>,
    /// The particles take the surface's color.
    pub tint: bool,
    /// The noise's cell (object units).
    pub cell: f32,
    /// The height share's weight ([`HdDissolve::sweep`]).
    pub sweep: f32,
    /// The particles a square unit of surface makes as the front crosses it.
    pub density: f32,
}

impl Default for Look {
    fn default() -> Look {
        Look {
            direction: Direction::Out,
            duration_ms: 2000,
            edge_color: [1.0, 0.45, 0.1],
            glow: 6.0,
            edge_share: 0.04,
            emitter: None,
            tint: false,
            cell: 0.25,
            sweep: 0.0,
            density: 64.0,
        }
    }
}

impl Look {
    /// The look with every value in its range (a value that is not finite is the default's).
    #[must_use]
    pub fn held(self) -> Look {
        let d = Look::default();
        let finite = |v: f32, or: f32| if v.is_finite() { v } else { or };
        Look {
            duration_ms: self.duration_ms.min(MAX_DURATION_MS),
            edge_color: [0, 1, 2].map(|k| finite(self.edge_color[k], 0.0).clamp(0.0, 1.0)),
            glow: finite(self.glow, d.glow).clamp(0.0, 64.0),
            edge_share: finite(self.edge_share, d.edge_share).clamp(0.0, 1.0),
            cell: finite(self.cell, d.cell).clamp(MIN_CELL, 1000.0),
            sweep: finite(self.sweep, 0.0).clamp(-1.0, 1.0),
            density: finite(self.density, d.density).clamp(0.0, MAX_DENSITY),
            ..self
        }
    }

    /// The look a ghost's cloak group carries (`DissolveField`, its whole numbers back to their units), held to its ranges.
    #[must_use]
    pub fn of_field(d: &DissolveField) -> Look {
        Look {
            direction: if d.out { Direction::Out } else { Direction::In },
            duration_ms: d.duration_ms,
            edge_color: d.edge.map(|b| b as f32 / 255.0),
            glow: d.glow as f32 / 8.0,
            edge_share: d.edge_share as f32 / 255.0,
            emitter: d.emitter,
            tint: d.tint,
            cell: d.cell as f32 / 64.0,
            sweep: (d.sweep as f32 - 127.0) / 127.0,
            density: d.density as f32,
        }
        .held()
    }

    /// The share eaten `ms` into the dissolve.
    #[must_use]
    pub fn eaten_at(&self, ms: f64) -> f32 {
        let p = if self.duration_ms == 0 {
            1.0
        } else {
            (ms / f64::from(self.duration_ms)).clamp(0.0, 1.0) as f32
        };
        match self.direction {
            Direction::Out => p,
            Direction::In => 1.0 - p,
        }
    }
}

/// A point of an object's surface in its own space.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    p: [f32; 3],
    n: [f32; 3],
    field: f32,
    color: u8,
}

/// An object's surface as its dissolve samples it.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    /// By field, lowest first.
    samples: Vec<Sample>,
    /// The palette's colors (0..1 RGBA).
    pub palette: Vec<[f32; 4]>,
    pub z_range: [f32; 2],
    /// The surface's area (square units, in the world at the sampling's transform).
    pub area: f32,
    /// The particles each sample makes as the front crosses it ([`MAX_GRAINS_PER_SAMPLE`] at most), spread over a square of
    /// `spacing` (units) about it.
    pub grains: u32,
    pub spacing: f32,
}

impl Surface {
    /// The samples taken.
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The field under which `eaten` of the samples lie ([`WHOLE`] for none, [`GONE`] for all).
    #[must_use]
    pub fn quantile(&self, eaten: f32) -> f32 {
        let n = self.samples.len();
        if eaten <= 0.0 || (n == 0 && eaten < 0.5) {
            return WHOLE;
        }
        if eaten >= 1.0 || n == 0 {
            return GONE;
        }
        let at = ((eaten * n as f32) as usize).min(n - 1);
        self.samples[at].field
    }

    /// The samples' place where the share `eaten` is (the first not eaten).
    fn index(&self, eaten: f32) -> usize {
        let n = self.samples.len();
        ((eaten.clamp(0.0, 1.0) * n as f32) as usize).min(n)
    }
}

/// A texture's mean color (its smallest level's, from at most [`MEAN_TEXELS`] texels spread over it), 0..1 RGBA.
fn texture_mean(t: &Texture) -> [f32; 4] {
    let Some(level) = t.levels.last().filter(|l| l.len() >= 4) else {
        return [1.0; 4];
    };
    let texels = level.as_chunks::<4>().0;
    let step = texels.len().div_ceil(MEAN_TEXELS).max(1);
    let mut sum = [0.0f64; 4];
    let mut n = 0.0f64;
    for px in texels.iter().step_by(step) {
        for (s, &c) in sum.iter_mut().zip(px) {
            *s += f64::from(c);
        }
        n += 255.0;
    }
    sum.map(|s| (s / n) as f32)
}

/// The mean colors of the scene's textures, by their index in its table (worked out once: the table keeps an index's texture).
#[derive(Debug, Clone, Default)]
pub struct TextureMeans(HashMap<usize, [f32; 4]>);

impl TextureMeans {
    /// Texture `i`'s mean color (white for none).
    pub fn get(&mut self, textures: &[Texture], i: usize) -> [f32; 4] {
        *self
            .0
            .entry(i)
            .or_insert_with(|| textures.get(i).map_or([1.0; 4], texture_mean))
    }
}

/// What an object's samples for `look` are made of: its draws' vertex and index counts and textures, and the look's cell,
/// sweep and density. A restart with the same key keeps the samples.
#[must_use]
pub fn surface_key(object: &DynamicObject, look: &Look) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    for d in &object.draws {
        (d.vertices.len(), d.indices.len(), d.texture.map(|t| t.0)).hash(&mut h);
    }
    (
        look.cell.to_bits(),
        look.sweep.to_bits(),
        look.density.to_bits(),
    )
        .hash(&mut h);
    h.finish()
}

/// A small deterministic random stream (the samples' places).
struct Pcg(u64);

impl Pcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / 16_777_216.0
    }
}

/// A draw's color for its particles: its vertices' mean color times its texture's mean.
fn draw_color(d: &crate::hd::HdDraw, textures: &[Texture], means: &mut TextureMeans) -> [f32; 4] {
    let tex = d.texture.map_or([1.0; 4], |(i, _)| means.get(textures, i));
    let mut vc = [0.0f32; 4];
    for v in &d.vertices {
        for (s, &c) in vc.iter_mut().zip(&v.color) {
            *s += f32::from(c) / 255.0;
        }
    }
    let nv = d.vertices.len().max(1) as f32;
    std::array::from_fn(|c| vc[c] / nv * tex[c])
}

/// A triangle of an object in its own space, its area (in the world) and its draw.
#[derive(Debug, Clone, Copy)]
struct Tri {
    corners: [[f32; 3]; 3],
    area: f32,
    draw: usize,
}

/// `object`'s triangles in its own space (every `stride`th of each draw's), each draw's area of them, and their z range.
fn object_triangles(object: &DynamicObject, stride: usize) -> (Vec<Tri>, Vec<f32>, [f32; 2]) {
    let to_object = crate::hd::post::affine_inverse_scaled(&object.transform);
    let mut tris = Vec::new();
    let mut areas = Vec::with_capacity(object.draws.len());
    let mut z_range = [f32::INFINITY, f32::NEG_INFINITY];
    for (k, d) in object.draws.iter().enumerate() {
        let mut area = 0.0;
        for t in d.indices.as_chunks::<3>().0.iter().step_by(stride) {
            let corner = |i: u32| d.vertices.get(i as usize).map(|v| object.world_point(v));
            let (Some(w0), Some(w1), Some(w2)) = (corner(t[0]), corner(t[1]), corner(t[2])) else {
                continue;
            };
            let (a, b) = (
                glam::Vec3::from(w1) - glam::Vec3::from(w0),
                glam::Vec3::from(w2) - glam::Vec3::from(w0),
            );
            let ta = 0.5 * a.cross(b).length();
            if !(ta.is_finite() && ta > 0.0) {
                continue;
            }
            let corners = [w0, w1, w2].map(|w| to_object.mul_p(w));
            for p in &corners {
                z_range[0] = z_range[0].min(p[2]);
                z_range[1] = z_range[1].max(p[2]);
            }
            tris.push(Tri {
                corners,
                area: ta,
                draw: k,
            });
            area += ta;
        }
        areas.push(area);
    }
    (tris, areas, z_range)
}

/// The palette of draws' `colors` weighed by their `areas`: the largest ones' colors (at most [`MAX_PALETTE`], alike ones
/// merged), and each draw's place in it (a draw past the palette's room takes the nearest).
fn palette_of(colors: &[[f32; 4]], areas: &[f32]) -> (Vec<[f32; 4]>, Vec<u8>) {
    let mut order: Vec<usize> = (0..colors.len()).collect();
    order.sort_by(|&a, &b| areas[b].total_cmp(&areas[a]));
    let mut palette: Vec<[f32; 4]> = Vec::new();
    let mut slot = vec![0u8; colors.len()];
    for &k in &order {
        let c = colors[k];
        let near = |p: &[f32; 4]| (0..3).map(|i| (p[i] - c[i]).powi(2)).sum::<f32>();
        slot[k] = match palette.iter().position(|p| near(p) < 1e-4) {
            Some(i) => i as u8,
            None if palette.len() < MAX_PALETTE => {
                palette.push(c);
                (palette.len() - 1) as u8
            }
            None => (0..palette.len())
                .min_by(|&a, &b| near(&palette[a]).total_cmp(&near(&palette[b])))
                .unwrap_or(0) as u8,
        };
    }
    (palette, slot)
}

/// `count` points over `tris` (a triangle's share by its area), each with its normal, its draw's palette place (`slot`) and
/// its field for `look`, sorted by the field; seeded by `seed`.
fn scatter(
    tris: &[Tri],
    slot: &[u8],
    count: usize,
    seed: u64,
    look: &Look,
    z_range: [f32; 2],
) -> Vec<Sample> {
    let mut cumulative = Vec::with_capacity(tris.len());
    let mut acc = 0.0f64;
    for t in tris {
        acc += f64::from(t.area);
        cumulative.push(acc);
    }
    let mut rng = Pcg(0x9e37_79b9_7f4a_7c15 ^ seed);
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        let r = f64::from(rng.next()) * acc;
        let t = tris[cumulative.partition_point(|&c| c < r).min(tris.len() - 1)];
        let (mut u, mut v) = (rng.next(), rng.next());
        if u + v > 1.0 {
            (u, v) = (1.0 - u, 1.0 - v);
        }
        let o = t.corners;
        let p: [f32; 3] =
            std::array::from_fn(|c| o[0][c] + (o[1][c] - o[0][c]) * u + (o[2][c] - o[0][c]) * v);
        let e1 = glam::Vec3::from(o[1]) - glam::Vec3::from(o[0]);
        let e2 = glam::Vec3::from(o[2]) - glam::Vec3::from(o[0]);
        samples.push(Sample {
            p,
            n: e1.cross(e2).normalize_or_zero().to_array(),
            field: field(p, look.cell, look.sweep, z_range),
            color: slot[t.draw],
        });
    }
    samples.sort_by(|a, b| a.field.total_cmp(&b.field));
    samples
}

/// Samples `object`'s surface for `look` (its textures in `textures`, the scene's table, their means kept in `means`).
#[must_use]
pub fn sample_surface(
    object: &DynamicObject,
    textures: &[Texture],
    means: &mut TextureMeans,
    look: &Look,
) -> Surface {
    // Past the most triangles, an even stride of them stands for the rest (their area scaled up by it).
    let triangles: usize = object.draws.iter().map(|d| d.indices.len() / 3).sum();
    let stride = triangles.div_ceil(MAX_TRIANGLES).max(1);
    let (tris, areas, z_range) = object_triangles(object, stride);
    if tris.is_empty() {
        return Surface {
            samples: Vec::new(),
            palette: Vec::new(),
            z_range: [0.0, 0.0],
            area: 0.0,
            grains: 1,
            spacing: 0.0,
        };
    }
    let colors: Vec<[f32; 4]> = object
        .draws
        .iter()
        .map(|d| draw_color(d, textures, means))
        .collect();
    let (palette, slot) = palette_of(&colors, &areas);
    let area: f32 = areas.iter().sum::<f32>() * stride as f32;
    let wanted = (area * look.density).max(0.0);
    let count = (wanted as usize).clamp(MIN_SAMPLES, MAX_SAMPLES);
    let samples = scatter(&tris, &slot, count, u64::from(object.ghost), look, z_range);
    Surface {
        // Past the samples' cap each one stands for several grains, spread about it.
        grains: ((wanted / count as f32).ceil() as u32).clamp(1, MAX_GRAINS_PER_SAMPLE),
        spacing: (area / count as f32).sqrt(),
        samples,
        palette,
        z_range,
        area,
    }
}

/// One dissolve the client runs.
#[derive(Debug, Clone)]
struct Item {
    serial: u32,
    look: Look,
    /// The clock (ms) it started at, and at which the client last started it (the restarts' limit).
    started: f64,
    restarted: f64,
    /// Its samples and what they were made of ([`surface_key`]).
    surface: Option<Surface>,
    surface_key: Option<u64>,
    /// The share eaten when the front last emitted (None: not yet drawn).
    eaten_last: Option<f32>,
    /// The emitter the dissolve was started with (taken when its surface is sampled), then the front's: it alone, or one a
    /// palette color ([`Look::tint`]).
    base: Option<Emitter>,
    emitters: Vec<Emitter>,
    /// Drawn this frame.
    drawn: bool,
}

/// The client's dissolves (module docs).
#[derive(Debug, Clone)]
pub struct Dissolves {
    items: BTreeMap<u32, Item>,
    /// The dissolves' clock (ms).
    clock: f64,
    rng: Rng,
    live: usize,
    wind: [f32; 3],
    /// The textures' mean colors.
    means: TextureMeans,
    /// The particles the fronts made in the last frame, and the surfaces it sampled.
    pub spawned: usize,
    pub sampled: usize,
}

impl Default for Dissolves {
    fn default() -> Dissolves {
        Dissolves {
            items: BTreeMap::new(),
            clock: 0.0,
            rng: Rng::new(0xd155_01e5),
            live: 0,
            wind: [0.0; 3],
            means: TextureMeans::default(),
            spawned: 0,
            sampled: 0,
        }
    }
}

impl Dissolves {
    /// Ghost `ghost` dissolves as `look` says, `elapsed_ms` into it, under `serial`: a new serial (or a new ghost) starts its
    /// clock again, its front's particles an emitter of [`Look::emitter`] made by `make` (None: no such datablock yet: tried
    /// again at each sync); the same one keeps it (the server sends the dissolve again with every update of its group). A
    /// ghost restarts at most once every [`MIN_RESTART_MS`]: a newer serial before then waits for a later sync. None: whole
    /// again.
    pub fn sync(
        &mut self,
        ghost: u32,
        state: Option<(u32, Look, u32)>,
        make: impl FnOnce(u32) -> Option<Emitter>,
    ) {
        let Some((serial, look, elapsed_ms)) = state else {
            self.forget(ghost);
            return;
        };
        if let Some(item) = self.items.get_mut(&ghost) {
            if item.serial == serial {
                if item.base.is_none()
                    && item.emitters.is_empty()
                    && let Some(db) = item.look.emitter
                {
                    item.base = make(db);
                }
                return;
            }
            if self.clock - item.restarted < MIN_RESTART_MS {
                return;
            }
        } else if self.items.len() >= MAX_OBJECTS {
            return;
        }
        let look = look.held();
        // The samples stay for the same look's noise (their shape is checked as the object is drawn).
        let kept = self.take(ghost).filter(|old| {
            (old.look.cell, old.look.sweep, old.look.density)
                == (look.cell, look.sweep, look.density)
        });
        let (surface, surface_key) =
            kept.map_or((None, None), |old| (old.surface, old.surface_key));
        self.items.insert(
            ghost,
            Item {
                serial,
                look,
                started: self.clock - f64::from(elapsed_ms.min(look.duration_ms)),
                restarted: self.clock,
                surface,
                surface_key,
                eaten_last: None,
                base: look.emitter.and_then(make),
                emitters: Vec::new(),
                drawn: false,
            },
        );
    }

    /// Every ghost of `world` whose record carries a dissolve ([`DissolveField`], the engine protocol's cloak group) synced
    /// ([`Dissolves::sync`]; its particles an emitter of the world's datablock), and the dissolves of the ghosts that no
    /// longer carry one forgotten.
    pub fn sync_world(&mut self, world: &ClientWorld<TgeClientSet>) {
        let mut carried = BTreeSet::new();
        for (&index, ghost) in &world.ghosts {
            let Some(d) = ghost.record.as_ref().and_then(record_dissolve) else {
                continue;
            };
            carried.insert(index);
            self.sync(
                index,
                Some((d.serial, Look::of_field(&d), d.elapsed_ms)),
                |db| Emitter::new(world, db),
            );
        }
        let gone: Vec<u32> = self
            .items
            .keys()
            .copied()
            .filter(|g| !carried.contains(g))
            .collect();
        for g in gone {
            self.forget(g);
        }
    }

    /// Forgets ghost `ghost`'s dissolve and its particles.
    pub fn forget(&mut self, ghost: u32) {
        let _ = self.take(ghost);
    }

    /// Ghost `ghost`'s dissolve out of the set, its particles off the live count.
    fn take(&mut self, ghost: u32) -> Option<Item> {
        let item = self.items.remove(&ghost)?;
        let n: usize = item.emitters.iter().map(|e| e.particles.len()).sum();
        self.live = self.live.saturating_sub(n);
        Some(item)
    }

    /// Forgets every dissolve (a new connection's ghosts are new).
    pub fn clear(&mut self) {
        self.items.clear();
        self.live = 0;
    }

    /// The ghosts dissolving.
    pub fn ghosts(&self) -> impl Iterator<Item = u32> + '_ {
        self.items.keys().copied()
    }

    /// The share of ghost `ghost`'s surface eaten now (None: not dissolving).
    #[must_use]
    pub fn eaten(&self, ghost: u32) -> Option<f32> {
        let item = self.items.get(&ghost)?;
        Some(item.look.eaten_at(self.clock - item.started))
    }

    /// The surface sampled for ghost `ghost`'s dissolve, once drawn.
    #[must_use]
    pub fn surface(&self, ghost: u32) -> Option<&Surface> {
        self.items.get(&ghost)?.surface.as_ref()
    }

    /// The air's velocity the fronts' particles are carried by.
    pub fn set_wind(&mut self, wind: [f32; 3]) {
        self.wind = wind;
        for e in self.items.values_mut().flat_map(|i| i.emitters.iter_mut()) {
            e.set_wind(wind);
        }
    }

    /// The particles alive (on the CPU path; the pool holds the handed-over ones).
    #[must_use]
    pub fn particles(&self) -> usize {
        self.items
            .values()
            .flat_map(|i| &i.emitters)
            .map(|e| e.particles.len())
            .sum()
    }

    /// A frame `dt` seconds after the last: every dissolve's clock moved on, its particles stepped, and for each one whose
    /// object is among `objects` (its textures in `textures`) its surface sampled the first time, its front's particles made,
    /// and its [`HdDissolve`] returned.
    pub fn frame(
        &mut self,
        objects: &[DynamicObject],
        textures: &[Texture],
        dt: f32,
    ) -> Vec<HdDissolve> {
        let ms = if dt.is_finite() {
            (dt.max(0.0) * 1000.0).min(1000.0)
        } else {
            0.0
        };
        self.clock += f64::from(ms);
        for e in self.items.values_mut().flat_map(|i| i.emitters.iter_mut()) {
            e.advance(ms as u32, &mut self.live);
        }
        let mut out = Vec::new();
        let mut left = MAX_SPAWNS_PER_FRAME;
        let mut samplings = MAX_SAMPLINGS_PER_FRAME;
        self.spawned = 0;
        self.sampled = 0;
        for item in self.items.values_mut() {
            item.drawn = false;
        }
        for o in objects {
            let Some(item) = self.items.get_mut(&o.ghost) else {
                continue;
            };
            if item.drawn {
                continue;
            }
            item.drawn = true;
            let look = item.look;
            let eaten = look.eaten_at(self.clock - item.started);
            let key = surface_key(o, &look);
            if item.surface_key != Some(key) && samplings > 0 {
                samplings -= 1;
                self.sampled += 1;
                item.surface = Some(sample_surface(o, textures, &mut self.means, &look));
                item.surface_key = Some(key);
            }
            let Some(surface) = item.surface.as_ref() else {
                // Not sampled yet: whole under half eaten, gone over half, no front.
                let t = if eaten < 0.5 { WHOLE } else { GONE };
                out.push(HdDissolve {
                    ghost: o.ghost,
                    threshold: t,
                    edge_to: t,
                    edge_color: [0.0; 3],
                    cell: look.cell,
                    sweep: look.sweep,
                    z_range: [0.0; 2],
                    eaten,
                });
                continue;
            };
            let threshold = surface.quantile(eaten);
            let edge_to = if eaten <= 0.0 || eaten >= 1.0 {
                threshold
            } else {
                surface.quantile(eaten + look.edge_share).max(threshold)
            };
            let glow = look.glow;
            out.push(HdDissolve {
                ghost: o.ghost,
                threshold,
                edge_to: if edge_to >= GONE && eaten < 1.0 {
                    // The band reaches past the last sample: all that is left glows.
                    f32::MAX
                } else {
                    edge_to
                },
                edge_color: look.edge_color.map(|c| crate::hd::srgb_to_linear(c) * glow),
                cell: look.cell,
                sweep: look.sweep,
                z_range: surface.z_range,
                eaten,
            });
            if let Some(base) = item.base.take() {
                item.emitters = tinted(base, &look, &surface.palette, self.wind);
            }
            let last = item.eaten_last.replace(eaten);
            let Some(last) = last else {
                continue;
            };
            if item.emitters.is_empty() || surface.is_empty() || last == eaten || left == 0 {
                continue;
            }
            let (a, b) = {
                let (i, j) = (surface.index(last), surface.index(eaten));
                (i.min(j), i.max(j))
            };
            let crossed = b - a;
            let grains = surface.grains.max(1) as usize;
            let allowed = (crossed * grains).min(MAX_SPAWNS_PER_OBJECT.min(left));
            let take = allowed.div_ceil(grains).min(crossed);
            let mut made = 0;
            for k in 0..take {
                let s = surface.samples[a + k * crossed / take];
                let n = glam::Vec3::from(s.n);
                let t1 = n.any_orthonormal_vector();
                let t2 = n.cross(t1);
                let e = usize::from(s.color).min(item.emitters.len() - 1);
                for _ in 0..grains.min(allowed - made) {
                    let (x, y) = (self.rng.f() - 0.5, self.rng.f() - 0.5);
                    let p = glam::Vec3::from(s.p) + (t1 * x + t2 * y) * surface.spacing;
                    let pos = o.transform.mul_p(p.to_array());
                    let axis = o.transform.mul_v(spill_axis(n, p).to_array());
                    if item.emitters[e].emit_at(pos, axis, &mut self.rng, &mut self.live) {
                        self.spawned += 1;
                    }
                    made += 1;
                }
            }
            left -= made;
        }
        out
    }

    /// The particles made since the last hand-over into `out`, for the GPU particle pool ([`Emitter::hand_over`]) instead of
    /// [`Dissolves::draws`].
    pub fn hand_over(&mut self, out: &mut SpriteFrame, textures: &mut TextureSet<'_>) {
        for e in self.items.values_mut().flat_map(|i| i.emitters.iter_mut()) {
            e.hand_over(out, textures, "dissolve", &mut self.live);
        }
    }

    /// The particles as draws, each emitter at its center's distance from `eye`, billboards along the camera's `right` and `up`.
    pub fn draws(
        &self,
        eye: [f32; 3],
        right: [f32; 3],
        up: [f32; 3],
        textures: &mut TextureSet<'_>,
    ) -> Vec<(f32, Draw)> {
        self.items
            .values()
            .flat_map(|i| &i.emitters)
            .filter_map(|e| {
                let c = e.centre()?;
                let d = e.draw(right, up, textures, "dissolve")?;
                let dist = (0..3).map(|i| (c[i] - eye[i]).powi(2)).sum::<f32>().sqrt();
                Some((dist, d))
            })
            .collect()
    }
}

/// The dissolve a ShapeBase record's cloak group carries.
fn record_dissolve(record: &TgeRecord) -> Option<DissolveField> {
    shape_part(record)?
        .body
        .as_ref()?
        .rare
        .as_ref()?
        .cloak?
        .dissolve
}

/// How much a grain is thrown away from the object's upright axis beside its surface's normal (`spill_axis`).
pub const SPILL: f32 = 0.8;

/// The direction a grain at `p` (object space) on a surface of normal `n` is ejected along: the normal turned away from the
/// object's upright axis by [`SPILL`], so a top face's grains spill out over the sides instead of falling back through the
/// body (the emitter's cone and speed spread them about it).
fn spill_axis(n: glam::Vec3, p: glam::Vec3) -> glam::Vec3 {
    let out = glam::Vec3::new(p.x, p.y, 0.0).normalize_or_zero();
    (n + out * SPILL).normalize_or(n)
}

/// A dissolve's emitters: `base` alone, or with [`Look::tint`] one a palette color, its color keys times the color.
fn tinted(mut base: Emitter, look: &Look, palette: &[[f32; 4]], wind: [f32; 3]) -> Vec<Emitter> {
    base.set_wind(wind);
    if !look.tint || palette.is_empty() {
        return vec![base];
    }
    let keys = base.particle_data[0].colors;
    let inv_alpha = base.particle_data[0].inv_alpha;
    base.data.use_emitter_colors = true;
    palette
        .iter()
        .map(|c| {
            let mut e = base.clone();
            e.set_colors(
                keys.map(|k| [k[0] * c[0], k[1] * c[1], k[2] * c[2], k[3]]),
                inv_alpha,
            );
            e
        })
        .collect()
}

#[cfg(test)]
#[path = "dissolve_tests.rs"]
mod tests;
