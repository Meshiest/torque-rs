//! Physically based materials for the enhanced renderer: a scene's [`MaterialTable`] beside its texture table, what a draw that
//! has one carries ([`PbrDraw`]: the material and its tangents), the maps' mip chains (color filtered in linear light, normals
//! renormalized with their shortening kept as roughness, Toksvig; a cutout's coverage kept level by level), the loader of a
//! [`torque_modelcore::pbr::PbrMaterial`]'s maps (PNG, JPEG, OpenEXR and KTX 2.0), and the bake of its maps into
//! block-compressed KTX 2.0 files ([`bake`]) that the loader takes instead of the images.
//!
//! A draw without a material is drawn exactly as before: its own texture, the class roughness. A draw with one keeps its own
//! texture too (the base color, so every pass that does not read the maps, the shadows, the secondary views and the prepass,
//! draws it as any other); the forward pass reads the material's maps.

use std::collections::HashMap;
use std::sync::Arc;

pub use images::ktx2::Ktx2Format;
pub use torque_modelcore::pbr::{AlphaMode, NormalConvention, PbrMaterial};

use images::ktx2;

use super::{HdVertex, TextureTable, linear_to_srgb, oct_encode, srgb_to_linear};
use crate::{TexelFormat, Texture};

/// A material's place in its scene's [`MaterialTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MaterialId(pub u32);

/// A material on the GPU's terms: its maps as indices into the scene's [`TextureTable`] (every map's levels already made by
/// this module's chains) and its factors.
#[derive(Debug, Clone, PartialEq)]
pub struct HdMaterial {
    /// Base color (sRGB), alpha in A.
    pub base: Option<usize>,
    /// Tangent-space normal, OpenGL's convention (a DirectX map is flipped as it is loaded), X and Y in R and G.
    pub normal: Option<usize>,
    /// Occlusion, roughness and metalness (linear).
    pub orm: Option<usize>,
    /// Height in R (linear, 1 the top).
    pub height: Option<usize>,
    /// Emissive color (sRGB).
    pub emissive: Option<usize>,
    pub base_factor: [f32; 4],
    pub roughness: f32,
    pub metal: f32,
    pub ao_strength: f32,
    pub normal_scale: f32,
    pub height_scale: f32,
    pub emissive_factor: [f32; 3],
    pub alpha: AlphaMode,
    pub alpha_cutoff: f32,
    pub two_sided: bool,
    pub uv_scale: [f32; 2],
}

impl HdMaterial {
    /// The material of `m`'s factors with no maps (the loader sets them).
    pub fn of_factors(m: &PbrMaterial) -> HdMaterial {
        HdMaterial {
            base: None,
            normal: None,
            orm: None,
            height: None,
            emissive: None,
            base_factor: m.base_factor,
            roughness: m.roughness,
            metal: m.metal,
            ao_strength: m.ao_strength,
            normal_scale: m.normal_scale,
            height_scale: m.height_scale,
            emissive_factor: m.emissive_factor,
            alpha: m.alpha,
            alpha_cutoff: m.alpha_cutoff,
            two_sided: m.two_sided,
            uv_scale: m.uv_scale,
        }
    }

    /// The material's uniform ([`MATERIAL_UNIFORM_BYTES`], `pbr.wgsl`'s `PbrMaterial`): the base factor; the emissive factor;
    /// roughness, metalness, AO strength and normal scale; height scale, alpha cutoff and UV scale; the maps present
    /// ([`map_bits`]), the alpha mode (0 opaque, 1 cutout, 2 blend) and whether it is two-sided (1).
    pub fn uniform_bytes(&self) -> [u8; MATERIAL_UNIFORM_BYTES] {
        let mut out = [0u8; MATERIAL_UNIFORM_BYTES];
        let floats = [
            self.base_factor[0],
            self.base_factor[1],
            self.base_factor[2],
            self.base_factor[3],
            self.emissive_factor[0],
            self.emissive_factor[1],
            self.emissive_factor[2],
            0.0,
            self.roughness,
            self.metal,
            self.ao_strength,
            self.normal_scale,
            self.height_scale,
            self.alpha_cutoff,
            self.uv_scale[0],
            self.uv_scale[1],
        ];
        for (c, v) in out.as_chunks_mut::<4>().0.iter_mut().zip(floats) {
            *c = v.to_le_bytes();
        }
        let maps = [self.normal, self.orm, self.height, self.emissive];
        let bits = maps
            .iter()
            .zip(map_bits::ALL)
            .filter(|(m, _)| m.is_some())
            .fold(0u32, |b, (_, bit)| b | bit);
        let alpha = match self.alpha {
            AlphaMode::Opaque => 0u32,
            AlphaMode::Cutout => 1,
            AlphaMode::Blend => 2,
        };
        out[64..68].copy_from_slice(&bits.to_le_bytes());
        out[68..72].copy_from_slice(&alpha.to_le_bytes());
        out[72..76].copy_from_slice(&u32::from(self.two_sided).to_le_bytes());
        out
    }

    /// Whether its draws go in the translucent pass.
    pub fn blended(&self) -> bool {
        self.alpha == AlphaMode::Blend
    }

    /// The bits of every float and index, for [`MaterialTable::intern`].
    fn key(&self) -> Vec<u32> {
        let index = |i: Option<usize>| i.map_or(u32::MAX, |i| i as u32);
        let mut k = vec![
            index(self.base),
            index(self.normal),
            index(self.orm),
            index(self.height),
            index(self.emissive),
            self.alpha as u32,
            u32::from(self.two_sided),
        ];
        k.extend(
            self.uniform_bytes()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c)),
        );
        k
    }
}

/// The uniform's size (`pbr.wgsl`'s `PbrMaterial`).
pub const MATERIAL_UNIFORM_BYTES: usize = 80;

/// The maps a material's uniform says it has (`pbr.wgsl`'s `PBR_*`): a map left out reads as its neutral value (a flat
/// normal, an occlusion, roughness and metalness of 1 times the factors, no height, an emission of the factor alone).
pub mod map_bits {
    pub const NORMAL: u32 = 1;
    pub const ORM: u32 = 2;
    pub const HEIGHT: u32 = 4;
    pub const EMISSIVE: u32 = 8;
    pub const ALL: [u32; 4] = [NORMAL, ORM, HEIGHT, EMISSIVE];
}

/// A scene's materials, each distinct one once, kept across syncs (the GPU makes only the new ones' groups).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaterialTable {
    pub materials: Vec<HdMaterial>,
    keys: HashMap<Vec<u32>, u32>,
}

impl MaterialTable {
    /// The table's id of `m` (added when new).
    pub fn intern(&mut self, m: HdMaterial) -> MaterialId {
        let key = m.key();
        if let Some(&i) = self.keys.get(&key) {
            return MaterialId(i);
        }
        let i = self.materials.len() as u32;
        self.materials.push(m);
        self.keys.insert(key, i);
        MaterialId(i)
    }

    pub fn get(&self, id: MaterialId) -> Option<&HdMaterial> {
        self.materials.get(id.0 as usize)
    }
}

/// A draw's physically based material and its vertices' tangents: one a vertex, the direction octahedral (as the normal is) and
/// the bitangent's handedness in the third (32,767 or -32,767), the fourth 0: the vertex stream the PBR path reads beside the
/// [`HdVertex`]es.
#[derive(Debug, Clone, PartialEq)]
pub struct PbrDraw {
    pub material: MaterialId,
    pub tangents: Vec<[i16; 4]>,
}

/// The bytes of a tangent a vertex.
pub const TANGENT_BYTES: usize = 8;

/// A tangent `[x, y, z, w]` in the stream's form.
pub fn encode_tangent(t: [f32; 4]) -> [i16; 4] {
    let [x, y] = oct_encode([t[0], t[1], t[2]]);
    [x, y, if t[3] < 0.0 { -32767 } else { 32767 }, 0]
}

/// The MikkTSpace tangents of indexed triangles of [`HdVertex`]es, in the stream's form.
pub fn tangents_of(vertices: &[HdVertex], indices: &[u32]) -> Vec<[i16; 4]> {
    let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
    let normals: Vec<[f32; 3]> = vertices
        .iter()
        .map(|v| super::oct_decode(v.normal))
        .collect();
    let uvs: Vec<[f32; 2]> = vertices.iter().map(|v| v.uv).collect();
    torque_modelcore::tangents::generate(&positions, &normals, &uvs, indices)
        .into_iter()
        .map(encode_tangent)
        .collect()
}

/// A level's size below one of `w` x `h`.
fn below(w: u32, h: u32) -> (u32, u32) {
    ((w >> 1).max(1), (h >> 1).max(1))
}

/// The 2 x 2 (or fewer, at an odd edge) texels of level `src` (`w` x `h`) under texel `(x, y)` of the level below.
fn footprint(w: u32, h: u32, x: u32, y: u32) -> impl Iterator<Item = usize> {
    let xs = [(2 * x).min(w - 1), (2 * x + 1).min(w - 1)];
    let ys = [(2 * y).min(h - 1), (2 * y + 1).min(h - 1)];
    ys.into_iter()
        .flat_map(move |yy| xs.into_iter().map(move |xx| (yy * w + xx) as usize))
}

/// The chain of levels from level 0 down to 1 x 1, each texel `f` of its footprint's texels in the level above, each level
/// handed to `emit` (its index, size and texels) as soon as it is made: only two levels of floats are alive at a time.
fn chain(
    level0: Vec<[f32; 4]>,
    w: u32,
    h: u32,
    mut f: impl FnMut(&[[f32; 4]], [usize; 4]) -> [f32; 4],
    mut emit: impl FnMut(usize, u32, u32, &[[f32; 4]]),
) {
    let (mut lw, mut lh, mut level) = (w, h, level0);
    for k in 0.. {
        emit(k, lw, lh, &level);
        if lw <= 1 && lh <= 1 {
            break;
        }
        let (nw, nh) = below(lw, lh);
        let mut next = Vec::with_capacity((nw * nh) as usize);
        for y in 0..nh {
            for x in 0..nw {
                let mut ids = [0usize; 4];
                for (slot, i) in ids.iter_mut().zip(footprint(lw, lh, x, y)) {
                    *slot = i;
                }
                next.push(f(&level, ids));
            }
        }
        (lw, lh, level) = (nw, nh, next);
    }
}

fn mean(level: &[[f32; 4]], ids: [usize; 4]) -> [f32; 4] {
    let mut s = [0.0f32; 4];
    for i in ids {
        for (a, b) in s.iter_mut().zip(level[i]) {
            *a += b;
        }
    }
    s.map(|v| v / 4.0)
}

fn to_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A level's RGBA8 bytes of its texels.
fn encoded(texels: &[[f32; 4]], encode: impl Fn([f32; 4]) -> [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 * texels.len());
    for &t in texels {
        out.extend_from_slice(&encode(t));
    }
    out
}

/// The levels of `level0`'s chain of means, each encoded by `encode`.
fn mean_levels(
    level0: Vec<[f32; 4]>,
    w: u32,
    h: u32,
    encode: impl Fn([f32; 4]) -> [u8; 4],
) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    chain(level0, w, h, mean, |_, _, _, l| {
        out.push(encoded(l, &encode))
    });
    out
}

/// The levels of an sRGB color map (RGBA8, `w` x `h`): averaged in linear light, alpha straight. With a `cutoff` (a cutout
/// material's) each level's alphas are those of [`coverage_alphas`]: as many of its texels pass the cutoff as level 0's do,
/// where a straight average would let the cutout thin out or fill in level by level.
pub(crate) fn color_levels(rgba: &[u8], w: u32, h: u32, cutoff: Option<f32>) -> Vec<Vec<u8>> {
    let level0: Vec<[f32; 4]> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let c = |v: u8| srgb_to_linear(f32::from(v) / 255.0);
            [c(p[0]), c(p[1]), c(p[2]), f32::from(p[3]) / 255.0]
        })
        .collect();
    let target = cutoff.map(|c| (c, coverage(&level0, c)));
    let mut out = Vec::new();
    chain(level0, w, h, mean, |k, _, _, texels| {
        let alphas = match target {
            Some((c, share)) if k > 0 => coverage_alphas(texels, c, share),
            _ => None,
        };
        let mut bytes = Vec::with_capacity(4 * texels.len());
        for (i, c) in texels.iter().enumerate() {
            let a = alphas
                .as_ref()
                .and_then(|a| a.get(i))
                .copied()
                .unwrap_or(c[3]);
            bytes.extend_from_slice(&[
                to_byte(linear_to_srgb(c[0])),
                to_byte(linear_to_srgb(c[1])),
                to_byte(linear_to_srgb(c[2])),
                to_byte(a),
            ]);
        }
        out.push(bytes);
    });
    out
}

/// The share of `texels` whose alpha passes `cutoff`.
pub fn coverage(texels: &[[f32; 4]], cutoff: f32) -> f32 {
    let passing = texels.iter().filter(|t| t[3] >= cutoff).count();
    passing as f32 / texels.len().max(1) as f32
}

/// `texels`' alphas scaled so that `share` of them (rounded to whole texels) pass `cutoff` (Castaño's coverage-preserving
/// mips): the cutoff falls halfway between the alpha of the texel ranked at that share and the next lower alpha, so neither
/// sits on it, and texels of the very alpha of the ranked one (an edge averaged the same along its length) pass in the
/// count wanted, spread evenly in their order, the others taking the next lower alpha. No alpha is left within
/// [`COVERAGE_MARGIN`] of the cutoff (a cutout's alpha is only compared with it, so moving it off the cutoff changes no
/// texel's side, and a block encoder's small errors then flip none). `None` when no scale reaches the share (texels of alpha
/// 0 only).
fn coverage_alphas(texels: &[[f32; 4]], cutoff: f32, share: f32) -> Option<Vec<f32>> {
    let alphas: Vec<f32> = texels.iter().map(|t| t[3]).collect();
    let n = alphas.len();
    let want = ((share * n as f32).round() as usize).min(n);
    let top = alphas.iter().copied().fold(0.0f32, f32::max);
    if top <= 0.0 {
        return None;
    }
    if want == 0 {
        // None pass: the largest goes to half the cutoff.
        let s = 0.5 * cutoff / top;
        return Some(alphas.iter().map(|a| a * s).collect());
    }
    let mut ranked = alphas.clone();
    let (_, &mut at, _) = ranked.select_nth_unstable_by(want - 1, |a, b| b.total_cmp(a));
    drop(ranked);
    if at <= 0.0 {
        return None;
    }
    // Alphas this close are one value (a level's means differ in float rounding only).
    const TIE: f32 = 1e-5;
    let tied = |a: f32| (a - at).abs() <= TIE;
    let below = alphas
        .iter()
        .copied()
        .filter(|&a| a < at - TIE)
        .fold(0.0f32, f32::max);
    let s = cutoff / (0.5 * (at + below));
    let above = alphas.iter().filter(|&&a| a > at + TIE).count();
    let ties = alphas.iter().filter(|&&a| tied(a)).count();
    let keep = want - above;
    let mut tie = 0usize;
    let off = |a: f32| {
        if a >= cutoff {
            a.max((cutoff + COVERAGE_MARGIN).min(1.0))
        } else {
            a.min((cutoff - COVERAGE_MARGIN).max(0.0))
        }
    };
    Some(
        alphas
            .iter()
            .map(|&a| {
                if !tied(a) {
                    return off(a * s);
                }
                // Bresenham: `keep` of the `ties` pass, spread evenly.
                let passes = (tie + 1) * keep / ties > tie * keep / ties;
                tie += 1;
                off(if passes { a * s } else { below * s })
            })
            .collect(),
    )
}

/// How far from the cutoff [`coverage_alphas`] keeps every alpha below level 0: 8 steps of 255.
const COVERAGE_MARGIN: f32 = 8.0 / 255.0;

/// The levels of a linear map (RGBA8): a straight average.
pub(crate) fn linear_levels(rgba: &[u8], w: u32, h: u32) -> Vec<Vec<u8>> {
    let level0 = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p.map(|v| f32::from(v) / 255.0))
        .collect();
    mean_levels(level0, w, h, |c| c.map(to_byte))
}

/// A normal map's levels (RGBA8, X and Y in R and G; Z rebuilt, B ignored) and each level's texels' mean length before it was
/// made unit again: the averaged normals renormalized, the green channel flipped first for a DirectX map, and the lengths
/// ([`orm_levels`] turns them into roughness).
pub(crate) fn normal_levels(
    rgba: &[u8],
    w: u32,
    h: u32,
    convention: NormalConvention,
) -> (Vec<Vec<u8>>, Vec<Vec<f32>>) {
    let flip = convention == NormalConvention::DirectX;
    let level0: Vec<[f32; 4]> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let x = f32::from(p[0]) / 127.5 - 1.0;
            let mut y = f32::from(p[1]) / 127.5 - 1.0;
            if flip {
                y = -y;
            }
            let z = (1.0 - x * x - y * y).max(0.0).sqrt();
            let l = (x * x + y * y + z * z).sqrt().max(1e-6);
            [x / l, y / l, z / l, 1.0]
        })
        .collect();
    let (mut bytes, mut lengths) = (Vec::new(), Vec::new());
    // Each texel keeps its unit normal in xyz and its footprint's mean length in w (multiplied down the chain).
    chain(
        level0,
        w,
        h,
        |l, ids| {
            let m = mean(l, ids);
            let len = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt();
            let lw = ids.iter().map(|&i| l[i][3]).sum::<f32>() / 4.0;
            if len < 1e-6 {
                [0.0, 0.0, 1.0, 0.0]
            } else {
                [m[0] / len, m[1] / len, m[2] / len, lw * len]
            }
        },
        |_, _, _, l| {
            bytes.push(encoded(l, |n| {
                [
                    to_byte(n[0] * 0.5 + 0.5),
                    to_byte(n[1] * 0.5 + 0.5),
                    to_byte(n[2] * 0.5 + 0.5),
                    255,
                ]
            }));
            lengths.push(l.iter().map(|t| t[3]).collect());
        },
    );
    (bytes, lengths)
}

/// The roughness a texel of mean normal length `len` adds to GGX's alpha squared (Toksvig's: the lost length is the spread
/// of the normals under it).
pub fn toksvig_alpha2(alpha2: f32, len: f32) -> f32 {
    let len = len.clamp(1e-3, 1.0);
    (alpha2 + (1.0 - len) / len).clamp(0.0, 1.0)
}

/// An ORM map's levels (RGBA8, `w` x `h`; occlusion, roughness, metalness): averaged, each level's roughness widened as it
/// is encoded by the normal map's lost length at the same place ([`toksvig_alpha2`]; `normal` is the normal map's size and
/// lengths a level; the averages are of the map's own roughness).
pub(crate) fn orm_levels(
    rgba: &[u8],
    w: u32,
    h: u32,
    normal: Option<(u32, u32, &[Vec<f32>])>,
) -> Vec<Vec<u8>> {
    let level0 = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p.map(|v| f32::from(v) / 255.0))
        .collect();
    let mut out = Vec::new();
    chain(level0, w, h, mean, |k, lw, lh, texels| {
        let mut bytes = encoded(texels, |c| c.map(to_byte));
        if let Some((nw, nh, lengths)) = normal
            && let Some(len) = lengths.get(k)
        {
            let (sw, sh) = ((nw >> k).max(1), (nh >> k).max(1));
            for y in 0..lh {
                for x in 0..lw {
                    let nx = (u64::from(x) * u64::from(sw) / u64::from(lw)) as u32;
                    let ny = (u64::from(y) * u64::from(sh) / u64::from(lh)) as u32;
                    let Some(&l) = len.get((ny * sw + nx) as usize) else {
                        continue;
                    };
                    let i = (y * lw + x) as usize;
                    let a2 = texels[i][1].powi(4);
                    bytes[4 * i + 1] = to_byte(toksvig_alpha2(a2, l).sqrt().sqrt());
                }
            }
        }
        out.push(bytes);
    });
    out
}

/// Separate occlusion, roughness and metalness maps (R of each, RGBA8, all `w` x `h`; a missing one 1) packed into one ORM
/// image.
pub(crate) fn pack_orm(
    occlusion: Option<&[u8]>,
    rough: Option<&[u8]>,
    metal: Option<&[u8]>,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let n = (w * h) as usize;
    let r = |m: Option<&[u8]>, i: usize| m.and_then(|m| m.get(4 * i)).copied().unwrap_or(255);
    (0..n)
        .flat_map(|i| [r(occlusion, i), r(rough, i), r(metal, i), 255])
        .collect()
}

/// The largest side of a map the loader takes.
pub const MAX_MAP_SIDE: u32 = 4096;
/// The most bytes a scene's texture table may hold (every texture's levels, the maps' and the others') for a material to be
/// added: past it [`load`] and [`build`] refuse new materials (a scene of many large maps would otherwise hold gigabytes).
pub const MAX_SCENE_TEXTURE_BYTES: u64 = 2 << 30;
/// The most working memory one material may take: [`WORKING_BYTES_PER_TEXEL`] a texel of every map's level 0 together.
pub const MAX_MATERIAL_BYTES: u64 = 1 << 30;
/// What a map costs to load and build, a texel of its level 0, at the most: its decoded RGBA8 (4, kept until the material
/// is built) and the decoder's own copy (4, let go); the chain's float levels (21.3 over a whole chain, two alive at a time);
/// the encoded levels (5.3); a normal map's lengths (5.3); separate maps' packing (4). Every byte [`build`] allocates is under
/// it (`a_materials_build_allocates_no_more_than_its_working_bytes`).
pub const WORKING_BYTES_PER_TEXEL: u64 = 48;

/// The bytes a map of `w` x `h` keeps in the texture table: RGBA8, its whole chain of levels (at most 4/3 of level 0).
pub fn retained_bytes(sizes: impl IntoIterator<Item = (u32, u32)>) -> u64 {
    sizes
        .into_iter()
        .map(|(w, h)| (u64::from(w) * u64::from(h)).saturating_mul(16).div_ceil(3))
        .fold(0, u64::saturating_add)
}

/// The working memory of maps of these sizes ([`WORKING_BYTES_PER_TEXEL`] a texel).
pub fn working_bytes(sizes: impl IntoIterator<Item = (u32, u32)>) -> u64 {
    sizes
        .into_iter()
        .map(|(w, h)| (u64::from(w) * u64::from(h)).saturating_mul(WORKING_BYTES_PER_TEXEL))
        .fold(0, u64::saturating_add)
}

/// Why a material's maps did not load.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PbrLoadError {
    #[error("map `{0}` could not be read")]
    Missing(String),
    #[error("map `{0}` is not a PNG, JPEG, KTX 2.0 or OpenEXR file")]
    Kind(String),
    #[error("map `{path}` did not decode: {error}")]
    Decode { path: String, error: String },
    #[error(
        "map `{path}` is {w} x {h}: empty, over the {MAX_MAP_SIDE} a side taken, or not its bytes"
    )]
    TooBig { path: String, w: u32, h: u32 },
    #[error(
        "the material's maps take {0} bytes of working memory, over the {MAX_MATERIAL_BYTES} taken"
    )]
    TooMuch(u64),
    #[error("maps `{0}` and `{1}` are packed together and must be the same size")]
    Sizes(String, String),
    #[error(
        "the scene's textures hold {held} bytes; {adding} more would pass the {MAX_SCENE_TEXTURE_BYTES} taken"
    )]
    OverBudget { held: u64, adding: u64 },
    #[error("the material is out of range: {0}")]
    Material(String),
    #[error("map `{path}` holds {format:?}, which a {role} map cannot")]
    Format {
        path: String,
        role: &'static str,
        format: Ktx2Format,
    },
}

/// Refuses `m` when its factors or paths are out of range, or when `adding` bytes of maps would take the scene's texture
/// table past [`MAX_SCENE_TEXTURE_BYTES`].
fn admit(m: &PbrMaterial, textures: &TextureTable, adding: u64) -> Result<(), PbrLoadError> {
    m.check("")
        .map_err(|e| PbrLoadError::Material(e.to_string()))?;
    over_budget(textures.bytes(), adding).map_or(Ok(()), Err)
}

/// The refusal of `adding` bytes of maps added to a table holding `held` bytes, when they pass [`MAX_SCENE_TEXTURE_BYTES`].
fn over_budget(held: u64, adding: u64) -> Option<PbrLoadError> {
    (held.saturating_add(adding) > MAX_SCENE_TEXTURE_BYTES)
        .then_some(PbrLoadError::OverBudget { held, adding })
}

/// A map's level 0: RGBA8 rows top to bottom, `width` x `height`.
#[derive(Debug, Clone, PartialEq)]
pub struct MapImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A material's maps as images (what [`load`] decodes, or what a program makes in code), each optional.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapImages {
    pub base: Option<MapImage>,
    pub normal: Option<MapImage>,
    pub orm: Option<MapImage>,
    pub occlusion: Option<MapImage>,
    pub roughness: Option<MapImage>,
    pub metal: Option<MapImage>,
    pub height: Option<MapImage>,
    pub emissive: Option<MapImage>,
}

impl MapImages {
    /// The maps in [`SLOT_ROLES`]' order.
    fn slots(&self) -> [(&'static str, &Option<MapImage>); 8] {
        [
            ("base", &self.base),
            ("normal", &self.normal),
            ("orm", &self.orm),
            ("occlusion", &self.occlusion),
            ("roughness", &self.roughness),
            ("metal", &self.metal),
            ("height", &self.height),
            ("emissive", &self.emissive),
        ]
    }

    fn slot_mut(&mut self, k: usize) -> Option<&mut Option<MapImage>> {
        Some(match k {
            0 => &mut self.base,
            1 => &mut self.normal,
            2 => &mut self.orm,
            3 => &mut self.occlusion,
            4 => &mut self.roughness,
            5 => &mut self.metal,
            6 => &mut self.height,
            7 => &mut self.emissive,
            _ => return None,
        })
    }
}

/// A map's place in a material: the texture table holds one texture a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapRole {
    Base,
    Normal,
    /// Occlusion, roughness and metalness: one packed map, or the three separate ones packed.
    Orm,
    Height,
    Emissive,
}

impl MapRole {
    pub const ALL: [MapRole; 5] = [
        MapRole::Base,
        MapRole::Normal,
        MapRole::Orm,
        MapRole::Height,
        MapRole::Emissive,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            MapRole::Base => "base",
            MapRole::Normal => "normal",
            MapRole::Orm => "orm",
            MapRole::Height => "height",
            MapRole::Emissive => "emissive",
        }
    }

    /// The format [`bake`] stores the role in: BC7 for color (sRGB) and ORM, BC5 for the normal's X and Y, BC4 for the height.
    pub const fn baked(self) -> Ktx2Format {
        match self {
            MapRole::Base | MapRole::Emissive => Ktx2Format::Bc7Srgb,
            MapRole::Orm => Ktx2Format::Bc7,
            MapRole::Normal => Ktx2Format::Bc5,
            MapRole::Height => Ktx2Format::Bc4,
        }
    }

    /// Whether a KTX 2.0 file of `f` may hold the role: color any four-channel format (it is sampled as sRGB), the others a
    /// linear format with the channels they read (the normal two, the ORM three, the height one).
    pub fn takes(self, f: Ktx2Format) -> bool {
        use Ktx2Format as F;
        match self {
            MapRole::Base | MapRole::Emissive => {
                matches!(f, F::Rgba8 | F::Rgba8Srgb | F::Bc7 | F::Bc7Srgb)
            }
            MapRole::Normal => matches!(f, F::Rgba8 | F::Bc5 | F::Bc7),
            MapRole::Orm => matches!(f, F::Rgba8 | F::Bc7),
            MapRole::Height => !f.srgb(),
        }
    }
}

/// The role of each of a material's map paths, in [`MapImages::slots`]' order.
const SLOT_ROLES: [MapRole; 8] = [
    MapRole::Base,
    MapRole::Normal,
    MapRole::Orm,
    MapRole::Orm,
    MapRole::Orm,
    MapRole::Orm,
    MapRole::Height,
    MapRole::Emissive,
];

/// `m`'s map paths in [`SLOT_ROLES`]' order (separate occlusion, roughness and metalness only without a packed map).
fn slot_paths(m: &PbrMaterial) -> [Option<&str>; 8] {
    let separate = m.orm.is_none();
    [
        m.base.as_deref(),
        m.normal.as_deref(),
        m.orm.as_deref(),
        m.occlusion.as_deref().filter(|_| separate),
        m.roughness_map.as_deref().filter(|_| separate),
        m.metal_map.as_deref().filter(|_| separate),
        m.height.as_deref(),
        m.emissive.as_deref(),
    ]
}

fn is_ktx2(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".ktx2")
}

fn is_exr(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".exr")
}

/// Where [`bake`]'s file of `role` goes beside `m`'s maps: the role's first map path with its extension replaced by
/// `.<role>.ktx2` (`wall_diff.png` gives `wall_diff.base.ktx2`; separate occlusion, roughness and metalness the first of
/// them's). `None` when the material has no map of the role, or names a `.ktx2` for it (that file is the map).
pub fn baked_path(m: &PbrMaterial, role: MapRole) -> Option<String> {
    let path = slot_paths(m)
        .into_iter()
        .zip(SLOT_ROLES)
        .find_map(|(p, r)| p.filter(|_| r == role))?;
    if is_ktx2(path) {
        return None;
    }
    let name_at = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let stem = match path[name_at..].rfind('.') {
        Some(dot) => &path[..name_at + dot],
        None => path,
    };
    Some(format!("{stem}.{}.ktx2", role.name()))
}

/// The key/value entry of a baked file holding [`source_stamp`].
const STAMP_KEY: &str = "torque.source";

/// The version of the chains and encoders a bake stamps: raised whenever they make other levels of the same files, so
/// that older baked files are passed over.
const BAKE_VERSION: u64 = 1;

/// What a baked material's levels were made of: every source file's bytes, the settings its chains read (the normal's
/// convention, the alpha mode and cutoff) and [`BAKE_VERSION`]. A baked file whose stamp differs is out of date.
fn source_stamp(m: &PbrMaterial, sources: &[Option<Vec<u8>>]) -> String {
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let settings = [
        BAKE_VERSION,
        u64::from(m.normal_convention == NormalConvention::DirectX),
        m.alpha as u64,
        u64::from(m.alpha_cutoff.to_bits()),
    ];
    let parts = sources
        .iter()
        .map(|s| s.as_deref().map_or(0, super::content_hash))
        .chain(settings);
    let h = parts.fold(0x243f_6a88_85a3_08d3u64, |h, p| {
        (h ^ p).wrapping_mul(K).rotate_left(27)
    });
    format!("{h:016x}")
}

/// The limits a map's KTX 2.0 file is read under: [`MAX_MAP_SIDE`], and at most a whole RGBA8 chain of that side.
fn ktx2_limits() -> ktx2::Limits {
    ktx2::Limits {
        max_side: MAX_MAP_SIDE,
        max_bytes: retained_bytes([(MAX_MAP_SIDE, MAX_MAP_SIDE)]),
        ..ktx2::Limits::default()
    }
}

/// The bytes a KTX 2.0 map of `h` keeps in the texture table (an R16 one is widened to RGBA8).
fn ktx2_retained(h: &ktx2::Header) -> u64 {
    if h.format == Ktx2Format::R16 {
        2 * h.bytes
    } else {
        h.bytes
    }
}

/// A KTX 2.0 map as a table texture: block formats as they are, RGBA8 as it is, R16 as gray RGBA8.
fn ktx2_texture(path: String, t: ktx2::Ktx2) -> Texture {
    let format = t
        .format
        .block()
        .map_or(TexelFormat::Rgba8, TexelFormat::Block);
    let levels = if t.format == Ktx2Format::R16 {
        t.levels
            .iter()
            .map(|l| {
                l.as_chunks::<2>()
                    .0
                    .iter()
                    .flat_map(|c| {
                        let v = images::exr::gray8(u16::from_le_bytes(*c));
                        [v, v, v, 255]
                    })
                    .collect()
            })
            .collect()
    } else {
        t.levels
    };
    Texture {
        path,
        width: t.width,
        height: t.height,
        levels: Arc::new(levels),
        format,
    }
}

/// A map file checked from its header, not yet decoded.
enum Source<'a> {
    Image {
        path: &'a str,
        kind: images::Kind,
        bytes: Vec<u8>,
    },
    Exr {
        path: &'a str,
        bytes: Vec<u8>,
    },
}

/// Loads `m`'s maps (paths relative to the sidecar, read by `read`) and builds the material ([`build`]). A map is a PNG,
/// JPEG or OpenEXR file (its chain made here) or a KTX 2.0 file (its levels taken as they are); a role whose maps are images
/// is taken from [`bake`]'s file beside them instead ([`baked_path`]) when that file is readable, holds a format the role
/// takes and was baked from these very files and settings (its source stamp; a baked file is used unchecked when a source
/// is not readable). A map that is missing, of another kind, does not decode or is over the limits fails the whole material
/// (the draw keeps its plain texture).
///
/// # Errors
/// A material out of range ([`PbrMaterial::check`]), the first map that failed, the maps' working memory ([`working_bytes`]
/// for images, a KTX 2.0 file's bytes and its levels'; counted from their headers before any is decoded) over
/// [`MAX_MATERIAL_BYTES`], or the scene's textures over [`MAX_SCENE_TEXTURE_BYTES`] with them.
pub fn load(
    m: &PbrMaterial,
    prefix: &str,
    read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    textures: &mut TextureTable,
) -> Result<HdMaterial, PbrLoadError> {
    admit(m, textures, 0)?;
    let paths = slot_paths(m);
    for path in paths.into_iter().flatten() {
        if !is_ktx2(path) && !is_exr(path) && images::Kind::of_name(path).is_none() {
            return Err(PbrLoadError::Kind(path.to_string()));
        }
    }
    let limits = ktx2_limits();
    let mut total = 0u64;
    let mut retained = 0u64;
    let mut charge = |working: u64, keeps: u64| {
        total = total.saturating_add(working);
        retained = retained.saturating_add(keeps);
        if total > MAX_MATERIAL_BYTES {
            return Err(PbrLoadError::TooMuch(total));
        }
        Ok(())
    };
    let mut sources = bake_sources(m, read);
    let (ktx2_files, from_ktx2) = ktx2_maps(m, &sources, read, &limits)?;
    // Images made here for the ORM slots widen their roughness by a KTX 2.0 normal's lengths.
    let orm_made_here = (2..6).any(|k| paths[k].is_some_and(|p| !from_ktx2[k] && !is_ktx2(p)));
    for f in &ktx2_files {
        let h = &f.header;
        // The levels, and an R16 map's widened RGBA8 beside them.
        let widened = if h.format == Ktx2Format::R16 {
            ktx2_retained(h)
        } else {
            0
        };
        charge(
            (f.bytes.len() as u64)
                .saturating_add(h.bytes)
                .saturating_add(widened),
            ktx2_retained(h),
        )?;
        // The normal's level 0 decoded to RGBA8 and its float lengths ([`ktx2_normal_lengths`]).
        if f.role == MapRole::Normal && orm_made_here {
            charge(working_bytes([(h.width, h.height)]), 0)?;
        }
    }
    // The images' headers: their working memory is checked before any is decoded.
    let images = image_sources(&paths, &mut sources, &from_ktx2, &mut charge)?;
    admit(m, textures, retained)?;
    let maps = decode_sources(images)?;
    let mut ready = Vec::with_capacity(ktx2_files.len());
    let mut normal_lengths = None;
    for Ktx2Map {
        role, path, bytes, ..
    } in ktx2_files
    {
        let t = ktx2::read(&bytes, &limits).map_err(|e| PbrLoadError::Decode {
            path: path.clone(),
            error: e.to_string(),
        })?;
        // An ORM map made here widens its roughness by the normal map's lengths: those of a KTX 2.0 normal's level 0.
        if role == MapRole::Normal && orm_made_here {
            normal_lengths =
                Some(
                    ktx2_normal_lengths(&t).map_err(|error| PbrLoadError::Decode {
                        path: path.clone(),
                        error,
                    })?,
                );
        }
        ready.push((role, ktx2_texture(format!("{prefix}#{}", role.name()), t)));
    }
    let levels = chains(m, maps, normal_lengths)?;
    Ok(put(m, prefix, levels, ready, textures))
}

/// A role's map held in a KTX 2.0 file, its header checked.
struct Ktx2Map {
    role: MapRole,
    path: String,
    bytes: Vec<u8>,
    header: ktx2::Header,
}

impl Ktx2Map {
    /// `bytes` as `role`'s map: its header within `limits` and its format one the role takes.
    fn new(
        role: MapRole,
        path: String,
        bytes: Vec<u8>,
        limits: &ktx2::Limits,
    ) -> Result<Ktx2Map, PbrLoadError> {
        let header = ktx2::header(&bytes, limits).map_err(|e| match e {
            ktx2::Ktx2Error::Size { w, h, .. } => PbrLoadError::TooBig {
                path: path.clone(),
                w,
                h,
            },
            e => PbrLoadError::Decode {
                path: path.clone(),
                error: e.to_string(),
            },
        })?;
        if !role.takes(header.format) {
            return Err(PbrLoadError::Format {
                path,
                role: role.name(),
                format: header.format,
            });
        }
        Ok(Ktx2Map {
            role,
            path,
            bytes,
            header,
        })
    }
}

/// The roles of `m` held in KTX 2.0 files, and the slots they cover: those the material names as `.ktx2`, and those whose
/// baked file beside its images is readable, of a format the role takes and up to date with `sources` (the images' bytes;
/// a baked file is taken unchecked when one of its role's images is not readable).
fn ktx2_maps(
    m: &PbrMaterial,
    sources: &[Option<Vec<u8>>],
    read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    limits: &ktx2::Limits,
) -> Result<(Vec<Ktx2Map>, [bool; 8]), PbrLoadError> {
    let paths = slot_paths(m);
    let stamp = source_stamp(m, sources);
    let mut out = Vec::new();
    let mut covered = [false; 8];
    for role in MapRole::ALL {
        let slots: Vec<usize> = (0..8)
            .filter(|&k| SLOT_ROLES[k] == role && paths[k].is_some())
            .collect();
        let Some(path) = slots.first().and_then(|&k| paths[k]) else {
            continue;
        };
        let file = if is_ktx2(path) {
            let bytes = read(path).ok_or_else(|| PbrLoadError::Missing(path.to_string()))?;
            Some(Ktx2Map::new(role, path.to_string(), bytes, limits)?)
        } else {
            baked_path(m, role).and_then(|p| {
                let bytes = read(&p)?;
                let file = Ktx2Map::new(role, p, bytes, limits).ok()?;
                let all_sources = slots.iter().all(|&k| sources[k].is_some());
                let fresh = || {
                    ktx2::value_of(&file.bytes, STAMP_KEY, limits)
                        .is_ok_and(|v| v.as_deref() == Some(stamp.as_bytes()))
                };
                (!all_sources || fresh()).then_some(file)
            })
        };
        if let Some(file) = file {
            out.push(file);
            for k in slots {
                covered[k] = true;
            }
        }
    }
    Ok((out, covered))
}

/// The image files of `paths` (those not `skip`ped), their bytes taken from `sources` and checked from their headers: the
/// kind, a readable size within [`MAX_MAP_SIDE`], and each one's working and retained bytes handed to `charge` (which refuses
/// past the limit) before any is decoded.
fn image_sources<'a>(
    paths: &[Option<&'a str>; 8],
    sources: &mut [Option<Vec<u8>>],
    skip: &[bool; 8],
    charge: &mut dyn FnMut(u64, u64) -> Result<(), PbrLoadError>,
) -> Result<Vec<Option<Source<'a>>>, PbrLoadError> {
    let mut images = Vec::with_capacity(8);
    for (k, path) in paths.iter().enumerate() {
        let Some(path) = path.filter(|p| !skip[k] && !is_ktx2(p)) else {
            images.push(None);
            continue;
        };
        let bytes = sources
            .get_mut(k)
            .and_then(Option::take)
            .ok_or_else(|| PbrLoadError::Missing(path.to_string()))?;
        let (source, size) = if is_exr(path) {
            // The side check below refuses a large one as too big.
            let size = images::exr::size_of(&bytes, u32::MAX);
            (Source::Exr { path, bytes }, size.map_err(|e| e.to_string()))
        } else {
            let kind =
                images::Kind::of_name(path).ok_or_else(|| PbrLoadError::Kind(path.to_string()))?;
            let size =
                images::size_of(kind, &bytes).ok_or_else(|| "no readable header".to_string());
            (Source::Image { path, kind, bytes }, size)
        };
        let (w, h) = size.map_err(|error| PbrLoadError::Decode {
            path: path.to_string(),
            error,
        })?;
        if w > MAX_MAP_SIDE || h > MAX_MAP_SIDE {
            return Err(PbrLoadError::TooBig {
                path: path.to_string(),
                w,
                h,
            });
        }
        charge(working_bytes([(w, h)]), retained_bytes([(w, h)]))?;
        images.push(Some(source));
    }
    Ok(images)
}

/// The checked image files decoded into their slots of a [`MapImages`].
fn decode_sources(images: Vec<Option<Source<'_>>>) -> Result<MapImages, PbrLoadError> {
    let mut maps = MapImages::default();
    for (k, source) in images.into_iter().enumerate() {
        let image = match source {
            None => continue,
            Some(Source::Image { path, kind, bytes }) => {
                let b = images::decode(kind, &bytes).map_err(|e| PbrLoadError::Decode {
                    path: path.to_string(),
                    error: e.to_string(),
                })?;
                // A gray file (height, roughness, metalness) is its gray in every channel, not TGE's alpha texel.
                let rgba = match b.format {
                    images::Format::Alpha => b.bits.iter().flat_map(|&v| [v, v, v, 255]).collect(),
                    _ => b.to_rgba8(),
                };
                MapImage {
                    rgba,
                    width: b.width,
                    height: b.height,
                }
            }
            Some(Source::Exr { path, bytes }) => {
                let r = images::exr::read_r16(&bytes, MAX_MAP_SIDE).map_err(|e| {
                    PbrLoadError::Decode {
                        path: path.to_string(),
                        error: e.to_string(),
                    }
                })?;
                MapImage {
                    rgba: r.to_rgba8(),
                    width: r.width,
                    height: r.height,
                }
            }
        };
        if let Some(slot) = maps.slot_mut(k) {
            *slot = Some(image);
        }
    }
    Ok(maps)
}

/// A normal map's lengths ([`normal_levels`]) of a KTX 2.0 file's level 0 (already in OpenGL's convention).
fn ktx2_normal_lengths(t: &ktx2::Ktx2) -> Result<(u32, u32, Vec<Vec<f32>>), String> {
    let level0 = t.levels.first().ok_or("no level")?;
    let rgba = match t.format.block() {
        Some(b) => images::bc::decode(b, level0, t.width, t.height).map_err(|e| e.to_string())?,
        None => level0.clone(),
    };
    let (_, lengths) = normal_levels(&rgba, t.width, t.height, NormalConvention::OpenGl);
    Ok((t.width, t.height, lengths))
}

/// The sizes of `maps`, each checked (not empty, within [`MAX_MAP_SIDE`], its bytes its size), and their working memory
/// within [`MAX_MATERIAL_BYTES`].
fn checked_sizes(prefix: &str, maps: &MapImages) -> Result<Vec<(u32, u32)>, PbrLoadError> {
    let slots = maps.slots();
    // Each map's size first: the memory sums below take sides within the limit.
    for (role, map) in slots {
        if let Some(i) = map
            && (i.width == 0
                || i.height == 0
                || i.width > MAX_MAP_SIDE
                || i.height > MAX_MAP_SIDE
                || i.rgba.len() as u64 != 4 * u64::from(i.width) * u64::from(i.height))
        {
            return Err(PbrLoadError::TooBig {
                path: format!("{prefix}#{role}"),
                w: i.width,
                h: i.height,
            });
        }
    }
    let sizes: Vec<(u32, u32)> = slots
        .iter()
        .filter_map(|(_, m)| m.as_ref().map(|i| (i.width, i.height)))
        .collect();
    let total = working_bytes(sizes.iter().copied());
    if total > MAX_MATERIAL_BYTES {
        return Err(PbrLoadError::TooMuch(total));
    }
    Ok(sizes)
}

/// A map's levels as this module's chains make them: RGBA8, level 0 first.
#[derive(Debug, Clone, PartialEq)]
pub struct MapLevels {
    pub role: MapRole,
    pub width: u32,
    pub height: u32,
    pub levels: Vec<Vec<u8>>,
}

/// The chains of `maps` (checked by [`checked_sizes`]): separate occlusion, roughness and metalness packed into one ORM map,
/// whose roughness is widened by the normal map's lengths (`maps`' own normal's, else `normal_lengths`).
fn chains(
    m: &PbrMaterial,
    maps: MapImages,
    normal_lengths: Option<(u32, u32, Vec<Vec<f32>>)>,
) -> Result<Vec<MapLevels>, PbrLoadError> {
    let orm = match maps.orm {
        Some(o) => Some(o),
        None => {
            let separate = [
                ("occlusion", &maps.occlusion),
                ("roughness", &maps.roughness),
                ("metal", &maps.metal),
            ];
            let sized: Vec<(&str, u32, u32)> = separate
                .iter()
                .filter_map(|(role, i)| i.as_ref().map(|i| (*role, i.width, i.height)))
                .collect();
            if let Some(first) = sized.first()
                && let Some(other) = sized.iter().find(|s| (s.1, s.2) != (first.1, first.2))
            {
                return Err(PbrLoadError::Sizes(
                    first.0.to_string(),
                    other.0.to_string(),
                ));
            }
            fn rgba(i: &Option<MapImage>) -> Option<&[u8]> {
                i.as_ref().map(|i| i.rgba.as_slice())
            }
            sized.first().map(|&(_, w, h)| MapImage {
                rgba: pack_orm(
                    rgba(&maps.occlusion),
                    rgba(&maps.roughness),
                    rgba(&maps.metal),
                    w,
                    h,
                ),
                width: w,
                height: h,
            })
        }
    };
    let mut out = Vec::with_capacity(5);
    let mut push = |role: MapRole, d: &MapImage, levels: Vec<Vec<u8>>| {
        out.push(MapLevels {
            role,
            width: d.width,
            height: d.height,
            levels,
        });
    };
    let cutoff = (m.alpha == AlphaMode::Cutout).then_some(m.alpha_cutoff);
    if let Some(d) = &maps.base {
        push(
            MapRole::Base,
            d,
            color_levels(&d.rgba, d.width, d.height, cutoff),
        );
    }
    if let Some(d) = &maps.emissive {
        push(
            MapRole::Emissive,
            d,
            color_levels(&d.rgba, d.width, d.height, None),
        );
    }
    if let Some(d) = &maps.height {
        push(
            MapRole::Height,
            d,
            linear_levels(&d.rgba, d.width, d.height),
        );
    }
    let normal_chain = maps.normal.as_ref().map(|d| {
        (
            d,
            normal_levels(&d.rgba, d.width, d.height, m.normal_convention),
        )
    });
    if let Some(d) = &orm {
        let lengths = match &normal_chain {
            Some((n, (_, lengths))) => Some((n.width, n.height, lengths.as_slice())),
            None => normal_lengths
                .as_ref()
                .map(|(w, h, l)| (*w, *h, l.as_slice())),
        };
        push(
            MapRole::Orm,
            d,
            orm_levels(&d.rgba, d.width, d.height, lengths),
        );
    }
    if let Some((d, (levels, _))) = normal_chain {
        push(MapRole::Normal, d, levels);
    }
    Ok(out)
}

/// `m`'s material of its maps' textures (`levels` made here, `ready` taken as they are), each interned into `textures`
/// under `prefix`.
fn put(
    m: &PbrMaterial,
    prefix: &str,
    levels: Vec<MapLevels>,
    ready: Vec<(MapRole, Texture)>,
    textures: &mut TextureTable,
) -> HdMaterial {
    let mut out = HdMaterial::of_factors(m);
    let made = levels.into_iter().map(|l| {
        (
            l.role,
            Texture {
                path: format!("{prefix}#{}", l.role.name()),
                width: l.width,
                height: l.height,
                levels: Arc::new(l.levels),
                format: TexelFormat::Rgba8,
            },
        )
    });
    for (role, t) in made.chain(ready) {
        let id = Some(textures.intern(t));
        match role {
            MapRole::Base => out.base = id,
            MapRole::Normal => out.normal = id,
            MapRole::Orm => out.orm = id,
            MapRole::Height => out.height = id,
            MapRole::Emissive => out.emissive = id,
        }
    }
    out
}

/// Builds `m`'s material of `maps`: each map's levels made by this module's chains into `textures` (keyed under `prefix`, the
/// sidecar's path or a program's name: two materials' maps of one role stay apart), separate occlusion, roughness and
/// metalness packed into one ORM map.
///
/// # Errors
/// A material out of range, separate maps of different sizes, a map whose bytes are not its size, maps whose working
/// memory is over [`MAX_MATERIAL_BYTES`], or the scene's textures over [`MAX_SCENE_TEXTURE_BYTES`] with them.
pub fn build(
    m: &PbrMaterial,
    prefix: &str,
    maps: MapImages,
    textures: &mut TextureTable,
) -> Result<HdMaterial, PbrLoadError> {
    let sizes = checked_sizes(prefix, &maps)?;
    admit(m, textures, retained_bytes(sizes))?;
    let levels = chains(m, maps, None)?;
    Ok(put(m, prefix, levels, Vec::new(), textures))
}

/// A role's baked KTX 2.0 file ([`bake`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BakedMap {
    pub role: MapRole,
    pub file: Vec<u8>,
}

/// `m`'s maps baked for loading: each role's levels made by this module's chains, encoded in its [`MapRole::baked`] block
/// format (a map whose sides are not multiples of 4 stays RGBA8: a GPU takes block textures of whole blocks only), and
/// written as a KTX 2.0 file with Zstandard levels. `sources` are the material's map files' bytes in their order (base,
/// normal, ORM, occlusion, roughness, metalness, height, emissive; the ones it names, see [`bake_sources`]), stamped into
/// each file so that [`load`] can tell a baked file made of other files; without them the files carry no stamp.
/// [`bake_files`] reads and decodes a sidecar's files and names what it bakes.
///
/// # Errors
/// As [`build`]'s, but for the texture budget (nothing is added to a table).
pub fn bake(
    m: &PbrMaterial,
    maps: MapImages,
    sources: Option<&[Option<Vec<u8>>]>,
) -> Result<Vec<BakedMap>, PbrLoadError> {
    bake_stamped(m, maps, sources.map(|s| source_stamp(m, s)))
}

fn bake_stamped(
    m: &PbrMaterial,
    maps: MapImages,
    stamp: Option<String>,
) -> Result<Vec<BakedMap>, PbrLoadError> {
    m.check("")
        .map_err(|e| PbrLoadError::Material(e.to_string()))?;
    checked_sizes("bake", &maps)?;
    chains(m, maps, None)?
        .into_iter()
        .map(|l| {
            let block = l.width.is_multiple_of(4) && l.height.is_multiple_of(4);
            let format = match l.role {
                _ if block => l.role.baked(),
                MapRole::Base | MapRole::Emissive => Ktx2Format::Rgba8Srgb,
                _ => Ktx2Format::Rgba8,
            };
            let levels = match format.block() {
                Some(b) => l
                    .levels
                    .iter()
                    .enumerate()
                    .map(|(k, level)| {
                        let (w, h) = ((l.width >> k).max(1), (l.height >> k).max(1));
                        images::bc::encode(b, level, w, h)
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| PbrLoadError::Material(e.to_string()))?,
                None => l.levels,
            };
            let t = ktx2::Ktx2 {
                format,
                width: l.width,
                height: l.height,
                levels,
                key_values: stamp
                    .iter()
                    .map(|s| (STAMP_KEY.to_string(), s.as_bytes().to_vec()))
                    .collect(),
            };
            let file = ktx2::write(&t, ktx2::Supercompression::Zstandard)
                .map_err(|e| PbrLoadError::Material(e.to_string()))?;
            Ok(BakedMap { role: l.role, file })
        })
        .collect()
}

/// The source files [`bake`] stamps, read by `read` as [`load`] reads them (`None` for a map the material does not name or
/// names as a `.ktx2`).
pub fn bake_sources(
    m: &PbrMaterial,
    read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Vec<Option<Vec<u8>>> {
    slot_paths(m)
        .into_iter()
        .map(|p| p.filter(|p| !is_ktx2(p)).and_then(&mut *read))
        .collect()
}

/// Bakes `m`'s image maps (read by `read` as [`load`] reads them, within its limits) into the files [`load`] takes beside
/// them: each [`BakedMap`] with its [`baked_path`]. A role the material names as a `.ktx2` is not baked.
///
/// # Errors
/// A material out of range, a map that is missing, of another kind, does not decode or is over the limits, or [`bake`]'s.
pub fn bake_files(
    m: &PbrMaterial,
    read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
) -> Result<Vec<(String, BakedMap)>, PbrLoadError> {
    m.check("")
        .map_err(|e| PbrLoadError::Material(e.to_string()))?;
    let paths = slot_paths(m);
    for path in paths.into_iter().flatten() {
        if !is_ktx2(path) && !is_exr(path) && images::Kind::of_name(path).is_none() {
            return Err(PbrLoadError::Kind(path.to_string()));
        }
    }
    let mut sources = bake_sources(m, read);
    let stamp = source_stamp(m, &sources);
    let mut total = 0u64;
    let mut charge = |working: u64, _: u64| {
        total = total.saturating_add(working);
        if total > MAX_MATERIAL_BYTES {
            return Err(PbrLoadError::TooMuch(total));
        }
        Ok(())
    };
    let images = image_sources(&paths, &mut sources, &[false; 8], &mut charge)?;
    let maps = decode_sources(images)?;
    Ok(bake_stamped(m, maps, Some(stamp))?
        .into_iter()
        .filter_map(|b| baked_path(m, b.role).map(|path| (path, b)))
        .collect())
}

#[cfg(test)]
#[path = "pbr_tests.rs"]
mod tests;
