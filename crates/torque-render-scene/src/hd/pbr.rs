//! Physically based materials for the enhanced renderer: a scene's [`MaterialTable`] beside its texture table, what a draw that
//! has one carries ([`PbrDraw`]: the material and its tangents), the maps' mip chains (color filtered in linear light, normals
//! renormalized with their shortening kept as roughness, Toksvig), and the loader of a [`torque_modelcore::pbr::PbrMaterial`]'s
//! maps.
//!
//! A draw without a material is drawn exactly as before: its own texture, the class roughness. A draw with one keeps its own
//! texture too (the base color, so every pass that does not read the maps, the shadows, the secondary views and the prepass,
//! draws it as any other); the forward pass reads the material's maps.

use std::collections::HashMap;
use std::sync::Arc;

pub use torque_modelcore::pbr::{AlphaMode, NormalConvention, PbrMaterial};

use super::{HdVertex, TextureTable, linear_to_srgb, oct_encode, srgb_to_linear};
use crate::Texture;

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

/// The levels of an sRGB color map (RGBA8, `w` x `h`): averaged in linear light, alpha straight.
pub(crate) fn color_levels(rgba: &[u8], w: u32, h: u32) -> Vec<Vec<u8>> {
    let level0 = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let c = |v: u8| srgb_to_linear(f32::from(v) / 255.0);
            [c(p[0]), c(p[1]), c(p[2]), f32::from(p[3]) / 255.0]
        })
        .collect();
    mean_levels(level0, w, h, |c| {
        [
            to_byte(linear_to_srgb(c[0])),
            to_byte(linear_to_srgb(c[1])),
            to_byte(linear_to_srgb(c[2])),
            to_byte(c[3]),
        ]
    })
}

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
    #[error("map `{0}` is not a PNG or JPEG file")]
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
}

/// Refuses `m` when its factors or paths are out of range, or when maps of `sizes` would take the scene's texture table past
/// [`MAX_SCENE_TEXTURE_BYTES`].
fn admit(
    m: &PbrMaterial,
    textures: &TextureTable,
    sizes: &[(u32, u32)],
) -> Result<(), PbrLoadError> {
    m.check("")
        .map_err(|e| PbrLoadError::Material(e.to_string()))?;
    over_budget(textures.bytes(), sizes).map_or(Ok(()), Err)
}

/// The refusal of maps of `sizes` added to a table holding `held` bytes, when they pass [`MAX_SCENE_TEXTURE_BYTES`].
fn over_budget(held: u64, sizes: &[(u32, u32)]) -> Option<PbrLoadError> {
    let adding = retained_bytes(sizes.iter().copied());
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

/// Loads `m`'s maps (paths relative to the sidecar, read by `read`) and builds the material ([`build`]). A map that is
/// missing, not PNG or JPEG, does not decode or is over the limits fails the whole material (the draw keeps its plain
/// texture).
///
/// # Errors
/// A material out of range ([`PbrMaterial::check`]), the first map that failed, the maps' working memory ([`working_bytes`],
/// counted from their headers before any is decoded) over [`MAX_MATERIAL_BYTES`], or the scene's textures over
/// [`MAX_SCENE_TEXTURE_BYTES`] with them.
pub fn load(
    m: &PbrMaterial,
    prefix: &str,
    read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    textures: &mut TextureTable,
) -> Result<HdMaterial, PbrLoadError> {
    admit(m, textures, &[])?;
    let separate = m.orm.is_none();
    let paths: [Option<&String>; 8] = [
        m.base.as_ref(),
        m.normal.as_ref(),
        m.orm.as_ref(),
        m.occlusion.as_ref().filter(|_| separate),
        m.roughness_map.as_ref().filter(|_| separate),
        m.metal_map.as_ref().filter(|_| separate),
        m.height.as_ref(),
        m.emissive.as_ref(),
    ];
    // Every map's file and header first: their working memory is checked before any is decoded.
    let mut files: Vec<Option<(&String, images::Kind, Vec<u8>)>> = Vec::with_capacity(8);
    let mut total = 0u64;
    let mut sizes = Vec::with_capacity(8);
    for path in paths {
        let Some(path) = path else {
            files.push(None);
            continue;
        };
        let kind = images::Kind::of_name(path).ok_or_else(|| PbrLoadError::Kind(path.clone()))?;
        let bytes = read(path).ok_or_else(|| PbrLoadError::Missing(path.clone()))?;
        let (w, h) = images::size_of(kind, &bytes).ok_or_else(|| PbrLoadError::Decode {
            path: path.clone(),
            error: "no readable header".to_string(),
        })?;
        if w > MAX_MAP_SIDE || h > MAX_MAP_SIDE {
            return Err(PbrLoadError::TooBig {
                path: path.clone(),
                w,
                h,
            });
        }
        total += working_bytes([(w, h)]);
        if total > MAX_MATERIAL_BYTES {
            return Err(PbrLoadError::TooMuch(total));
        }
        sizes.push((w, h));
        files.push(Some((path, kind, bytes)));
    }
    admit(m, textures, &sizes)?;
    let mut decoded = files
        .into_iter()
        .map(|f| -> Result<Option<MapImage>, PbrLoadError> {
            let Some((path, kind, bytes)) = f else {
                return Ok(None);
            };
            let b = images::decode(kind, &bytes).map_err(|e| PbrLoadError::Decode {
                path: path.clone(),
                error: e.to_string(),
            })?;
            Ok(Some(MapImage {
                rgba: b.to_rgba8(),
                width: b.width,
                height: b.height,
            }))
        });
    let mut next = || decoded.next().unwrap_or(Ok(None));
    let maps = MapImages {
        base: next()?,
        normal: next()?,
        orm: next()?,
        occlusion: next()?,
        roughness: next()?,
        metal: next()?,
        height: next()?,
        emissive: next()?,
    };
    build(m, prefix, maps, textures)
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
    let all = [
        ("base", &maps.base),
        ("normal", &maps.normal),
        ("orm", &maps.orm),
        ("occlusion", &maps.occlusion),
        ("roughness", &maps.roughness),
        ("metal", &maps.metal),
        ("height", &maps.height),
        ("emissive", &maps.emissive),
    ];
    // Each map's size first: the memory sums below take sides within the limit.
    for &(role, map) in &all {
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
    let sizes: Vec<(u32, u32)> = all
        .iter()
        .filter_map(|(_, m)| m.as_ref().map(|i| (i.width, i.height)))
        .collect();
    let total = working_bytes(sizes.iter().copied());
    if total > MAX_MATERIAL_BYTES {
        return Err(PbrLoadError::TooMuch(total));
    }
    admit(m, textures, &sizes)?;
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
    let mut put = |role: &str, d: &MapImage, levels: Vec<Vec<u8>>| {
        textures.intern(Texture {
            path: format!("{prefix}#{role}"),
            width: d.width,
            height: d.height,
            levels: Arc::new(levels),
        })
    };
    let mut out = HdMaterial::of_factors(m);
    out.base = maps
        .base
        .as_ref()
        .map(|d| put("base", d, color_levels(&d.rgba, d.width, d.height)));
    out.emissive = maps
        .emissive
        .as_ref()
        .map(|d| put("emissive", d, color_levels(&d.rgba, d.width, d.height)));
    out.height = maps
        .height
        .as_ref()
        .map(|d| put("height", d, linear_levels(&d.rgba, d.width, d.height)));
    let normal_chain = maps.normal.as_ref().map(|d| {
        (
            d,
            normal_levels(&d.rgba, d.width, d.height, m.normal_convention),
        )
    });
    out.orm = orm.as_ref().map(|d| {
        let lengths = normal_chain
            .as_ref()
            .map(|(n, (_, lengths))| (n.width, n.height, lengths.as_slice()));
        put("orm", d, orm_levels(&d.rgba, d.width, d.height, lengths))
    });
    if let Some((d, (levels, _))) = normal_chain {
        out.normal = Some(put("normal", d, levels));
    }
    Ok(out)
}

#[cfg(test)]
#[path = "pbr_tests.rs"]
mod tests;
