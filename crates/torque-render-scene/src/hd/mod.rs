//! The enhanced renderer's plain data: the quality ladder ([`Quality`]) and the image settings ([`ImageSettings`]), the vertex and its
//! [`material`] word, draws and moving objects, the camera with its jitter, the physical sky's tables, the sky's irradiance as
//! spherical harmonics, the sky's parts, the volumetric clouds, the image formation (exposure, tonemap, bloom, grade table), the
//! anti-aliasing and ambient occlusion ladder, the shadow cascades, the texture table, the color-space conversions, the day's
//! light from the physical sky ([`scatter`]) and the engine's own light and sky ghosts ([`environment`]); and the frame itself
//! ([`HdScene`]) over a provider's static geometry ([`cells::StaticGeometry`]).

use std::collections::HashMap;

use glam::Vec3;

use crate::math::Mat;
use crate::{Blend, Cull, Draw, Sampler, TexEnv, Texture};

pub mod atmosphere;
pub mod camera;
pub mod cells;
pub mod clouds;
pub mod clusters;
pub mod environment;
pub mod grade;
pub mod kit;
pub mod lut;
pub mod objects;
pub mod origin;
pub mod particle_light;
pub mod post;
pub mod reflect;
pub mod scatter;
pub mod scene;
pub mod settings;
pub mod sh;
pub mod shadow;
pub mod sky;
pub mod sync;
pub mod view_cull;
pub mod view_scene;

pub use camera::HdCamera;
pub use origin::{RenderOrigin, Shift};
pub use scene::{HdFog, HdLight, HdScene, HdSpot, HdSun, WaterZone};
pub use settings::{HdMode, ImageSettings};
pub use view_scene::ViewScene;

/// The texture table: each distinct texture once, kept across syncs (the GPU uploads only the new ones).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextureTable {
    pub textures: Vec<Texture>,
    keys: HashMap<(String, u64), usize>,
    /// The same decoded levels (one `Arc`, the texture cache's) under the same path: found by their address without hashing
    /// their bytes again (a sync hashed every texture's level 0, 24 ms a sync for a big scene's textures) or copying the path. The
    /// table holds every `Arc` it keys, so no address is reused while it is a key; a hit is checked to be that `Arc` under that
    /// path.
    shared: HashMap<usize, usize>,
}

/// A texture's level 0 hashed 32 bytes at a time in four lanes (FNV a byte at a time took 0.76 ms of a 512 x 512 texture).
fn content_hash(bytes: &[u8]) -> u64 {
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut lanes = [
        0x243f_6a88_85a3_08d3u64,
        0x1319_8a2e_0370_7344,
        0xa409_3822_299f_31d0,
        0x082e_fa98_ec4e_6c89,
    ];
    let (chunks, rest) = bytes.as_chunks::<32>();
    for c in chunks {
        for (k, lane) in lanes.iter_mut().enumerate() {
            let mut w = [0u8; 8];
            w.copy_from_slice(&c[k * 8..k * 8 + 8]);
            *lane = (*lane ^ u64::from_le_bytes(w))
                .wrapping_mul(K)
                .rotate_left(29);
        }
    }
    let mut h = bytes.len() as u64;
    for l in lanes {
        h = (h ^ l).wrapping_mul(K).rotate_left(31);
    }
    for &b in rest {
        h = (h ^ u64::from(b)).wrapping_mul(K);
    }
    h ^ (h >> 32)
}

impl TextureTable {
    /// The table's index of `t` (added when new).
    pub fn intern(&mut self, t: Texture) -> usize {
        let shared = std::sync::Arc::as_ptr(&t.levels) as usize;
        if let Some(&i) = self.shared.get(&shared)
            && let Some(held) = self.textures.get(i)
            && std::sync::Arc::ptr_eq(&held.levels, &t.levels)
            && held.path == t.path
        {
            return i;
        }
        let key = (
            t.path.clone(),
            t.levels.first().map_or(0, |l| content_hash(l))
                ^ u64::from(t.width) << 32
                ^ u64::from(t.height),
        );
        if let Some(&i) = self.keys.get(&key) {
            // Kept only when the table holds these very levels (else the address could be reused once `t` is dropped).
            if std::sync::Arc::ptr_eq(&self.textures[i].levels, &t.levels) {
                self.shared.insert(shared, i);
            }
            return i;
        }
        self.textures.push(t);
        self.keys.insert(key, self.textures.len() - 1);
        self.shared.insert(shared, self.textures.len() - 1);
        self.textures.len() - 1
    }
}

/// sRGB to linear, as the GPU's sRGB views decode (IEC 61966-2-1).
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear to sRGB (what the tonemap pass encodes).
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// The floats of the frame uniform's surface block (`common.wgsl`'s `Frame.surf0` to `surf2`: three vec4s).
pub const SURFACE_FLOATS: usize = 12;

/// The quality ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Quality {
    Low,
    Medium,
    High,
    Ultra,
}

impl Quality {
    pub const ALL: [Quality; 4] = [Quality::Low, Quality::Medium, Quality::High, Quality::Ultra];

    pub fn name(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::Medium => "medium",
            Quality::High => "high",
            Quality::Ultra => "ultra",
        }
    }

    pub fn named(name: &str) -> Option<Quality> {
        Quality::ALL
            .into_iter()
            .find(|q| q.name().eq_ignore_ascii_case(name))
    }

    /// Whether translucent surfaces colour the sun's shadows (a transmittance map beside each cascade: High and Ultra).
    pub fn coloured_shadows(self) -> bool {
        self >= Quality::High
    }

    /// The most point lights one cluster shades.
    pub fn lights_per_cluster(self) -> u32 {
        match self {
            Quality::Low => 64,
            Quality::Medium => 128,
            Quality::High => 256,
            Quality::Ultra => 1024,
        }
    }

    pub fn next(self) -> Quality {
        match self {
            Quality::Low => Quality::Medium,
            Quality::Medium => Quality::High,
            Quality::High => Quality::Ultra,
            Quality::Ultra => Quality::Low,
        }
    }
}

/// The material word of an [`HdVertex`]: what the shader does with the surface. The renderer never reads bits 0-9 and 19-21,
/// nor bits 27-30 of kind 0 (bits 29 and 30 of the others are [`material::DECAL`] and [`material::LIT`], or a shape's or plane's
/// [`material::CUTOUT`], and a plane's [`material::LAYER`]; bit 27 a plane's [`material::LIGHTMAP`]): they are an extension's
/// (kind 0 is the extension's static geometry).
pub mod material {
    /// Emissive: drawn unlit at its colour (times the settings' emissive gain).
    pub const GLOW: u32 = 1 << 10;
    /// Not lit at all (the sky, coronas, flares, self-illuminating shape materials, lighting off).
    pub const UNLIT: u32 = 1 << 11;
    /// The normal was (about) zero (foliage cards drawn with a zero normal): the shader faces it to the eye.
    pub const ZERO_NORMAL: u32 = 1 << 12;
    /// Bits 13-14: the texture environment (0 none, 1 decal, 2 modulate, 3 replace).
    pub const TEX_ENV_SHIFT: u32 = 13;
    pub const TEX_ENV_MASK: u32 = 0x3;
    /// Bits 16-17: what the surface is (0 an extension's static geometry, 1 a shape, 2 a plane, 3 the sky or an emitter).
    pub const KIND_SHIFT: u32 = 16;
    pub const KIND_SHAPE: u32 = 1;
    pub const KIND_PLANE: u32 = 2;
    pub const KIND_SKY: u32 = 3;
    /// An emitter sprite (a corona, the sun's flares): drawn unlit, scaled into the HDR range the bloom picks up.
    pub const EMITTER: u32 = 1 << 15;
    /// A cloud layer of the sky (lit by the sun with [`linear_light`](crate::hd::ImageSettings::linear_light)).
    pub const CLOUD: u32 = 1 << 18;
    /// Bit 27 of a shape's vertex: drawn into the shadow maps only, never seen (the client's own Player in its first-person view
    /// still casts its shadow).
    pub const SHADOW_ONLY: u32 = 1 << 27;

    /// Whether a vertex's word is a shape's drawn only into the shadow maps.
    pub fn shadow_only(word: u32) -> bool {
        (word >> KIND_SHIFT) & 0x3 == KIND_SHAPE && word & SHADOW_ONLY != 0
    }
    /// Bit 28 of a shape's vertex: seen but cast into no shadow map (in the enhanced renderer a first-person image drawn before the
    /// eye: a [`SHADOW_ONLY`] copy casts where the body holds it).
    pub const VIEW_ONLY: u32 = 1 << 28;

    /// Whether a vertex's word is a shape's cast into no shadow map.
    pub fn view_only(word: u32) -> bool {
        (word >> KIND_SHIFT) & 0x3 == KIND_SHAPE && word & VIEW_ONLY != 0
    }
    /// A Player's or a bot's body (the silhouette rim light; its mounted images are plain shapes).
    pub const PLAYER: u32 = 1 << 22;
    /// a billboard placed every frame (the sun's flares): its position is its centre less the eye it was built for, its UV
    /// the corner, its `extra` word its half size (f32 bits; negative: scaled by the facing too, the light's link flag); the vertex
    /// shader puts it at the eye of the frame facing the frame's camera and dims it by the camera's facing.
    pub const BILLBOARD: u32 = 1 << 23;
    /// A particle (an emitter's sprite): it fades into the surface behind it, and it is never the translucent front
    /// layer (it is weighted with what is behind, so the TAA's history of the front layer does not follow it).
    pub const PARTICLE: u32 = 1 << 24;
    /// With [`PARTICLE`], a decal (a mark lying on a surface, drawn with the particles): it does not fade into the surface
    /// behind it.
    pub const DECAL: u32 = 1 << 29;
    /// With [`PARTICLE`] (not [`DECAL`]), a lit sprite ([`super::particle_light`]): its normal is its direction from the
    /// sprite's center, its `extra` the half diagonal (f32 bits); lit by the sun, the sky and the lights though [`UNLIT`].
    pub const LIT: u32 = 1 << 30;
    /// With [`BILLBOARD`], one anchored in the world (a light's corona): its position is its centre itself, not less
    /// the eye, and it is not dimmed by the facing (a corona at the light, facing the camera).
    pub const ANCHORED: u32 = 1 << 25;
    /// A draw of the sky built around the eye of a whole sync (the scene's sky eye): the vertex shader moves it by the
    /// frame's eye less that one, so the sky stays around the eye between syncs.
    pub const SKY_EYE: u32 = 1 << 26;
    /// Bit 30 of a plane's vertex: a terrain material layer drawn over the layers under it (alpha-blended, depth tested and not
    /// written). Its vertex alpha is the material's share of the weights so far at the ground's samples; the shader sharpens it
    /// around one half (an edge a [`LAYER_SHARPNESS`]th of the share's ramp wide) and lowers it where the texture's alpha is
    /// under one (the texture's dips, where the layer under it shows first). Not drawn into a light capture.
    pub const LAYER: u32 = 1 << 30;
    /// How much the shader steepens a [`LAYER`]'s share about one half, and how far a texture alpha of 0 lowers it.
    pub const LAYER_SHARPNESS: f32 = 8.0;
    pub const LAYER_DIP: f32 = 1.0;

    /// Whether a vertex's word is a plane's terrain layer.
    pub fn layer(word: u32) -> bool {
        (word >> KIND_SHIFT) & 0x3 == KIND_PLANE && word & LAYER != 0
    }
    /// Bit 27 of a plane's vertex: lit by the scene's lightmap ([`super::HdScene::lightmap`]) instead of the sun, the sky and the
    /// lights: its color times the lightmap's light at its `extra` word ([`super::lightmap_uv`]: the place in the lightmap,
    /// interpolated across the triangle). A light map of light 1 leaves the texture as it is; past 1 is brighter.
    pub const LIGHTMAP: u32 = 1 << 27;

    /// Whether a vertex's word is a plane's lit by the lightmap.
    pub fn lightmapped(word: u32) -> bool {
        (word >> KIND_SHIFT) & 0x3 == KIND_PLANE && word & LIGHTMAP != 0
    }
    /// Bit 29 of a shape's or a plane's vertex: cut out where its texture's alpha (times its color's) is under one half, drawn
    /// as opaque elsewhere (leaves, fences). Its draw is left out of the depth prepass and casts no shadow (neither reads
    /// the texture), and is drawn opaque in the main pass.
    pub const CUTOUT: u32 = 1 << 29;

    /// Whether a vertex's word is a shape's or a plane's cut out by its texture's alpha.
    pub fn cutout(word: u32) -> bool {
        matches!((word >> KIND_SHIFT) & 0x3, KIND_SHAPE | KIND_PLANE) && word & CUTOUT != 0
    }
    /// A shape's vertex in its mesh's own space ([`super::NodePlace`]): its `extra` word is its mesh's place in its object's
    /// [`super::DynamicObject::nodes`] (the renderer adds the object's first place in the frame's table); the vertex shaders
    /// place it, and its normal, there.
    pub const NODE: u32 = 1 << 31;

    pub fn tex_env(env: crate::TexEnv) -> u32 {
        let e = match env {
            crate::TexEnv::None => 0,
            crate::TexEnv::Decal => 1,
            crate::TexEnv::Modulate => 2,
            crate::TexEnv::Replace => 3,
        };
        e << TEX_ENV_SHIFT
    }
}

/// A lightmap place as a vertex's `extra` word ([`material::LIGHTMAP`]): `u` and `v` (0..1 over the lightmap) as 16-bit fractions,
/// `u` in the low half.
pub fn lightmap_uv(uv: [f32; 2]) -> u32 {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 65535.0).round() as u32;
    q(uv[0]) | (q(uv[1]) << 16)
}

/// A half float's bits of `v` (rounded to nearest, overflow to infinity, small values flushed to zero).
pub fn half(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32 - 127 + 15;
    let man = b & 0x7f_ffff;
    if v.is_nan() {
        return sign | 0x7e00;
    }
    if exp >= 31 {
        return sign | 0x7c00;
    }
    if exp <= 0 {
        return sign;
    }
    let rounded = (exp as u32) << 10 | (man >> 13);
    let rounded =
        rounded + u32::from(man & 0x1fff > 0x1000 || (man & 0x1fff == 0x1000 && rounded & 1 == 1));
    sign | rounded.min(0x7c00) as u16
}

/// A scene's lightmap ([`HdScene::lightmap`]): `width` x `height` texels of linear light (RGBA half floats, [`half`]; 1 = the
/// texture as it is), rows top down, sampled bilinearly and clamped. Each surface's place in it is its vertices' `extra` word.
#[derive(Debug, Clone, PartialEq)]
pub struct Lightmap {
    pub width: u32,
    pub height: u32,
    pub texels: std::sync::Arc<Vec<[u16; 4]>>,
}

/// A vertex of the enhanced renderer (36 bytes on the GPU): world position, octahedral normal (two snorm16), UV, RGBA8 colour (the
/// GL colour of a shape or plane, or an extension's), the [`material`] word, and the `extra` word (a [`material::NODE`] vertex's
/// place, a [`material::BILLBOARD`]'s half size, an extension's own; 0 for anything else).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdVertex {
    pub position: [f32; 3],
    pub normal: [i16; 2],
    pub uv: [f32; 2],
    pub color: [u8; 4],
    pub material: u32,
    pub extra: u32,
}

/// The GPU's bytes of a vertex.
pub const HD_VERTEX_BYTES: usize = 36;

impl HdVertex {
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.bytes());
    }

    /// The vertex as the GPU reads it (one copy into the output instead of ten).
    pub fn bytes(&self) -> [u8; HD_VERTEX_BYTES] {
        let mut b = [0u8; HD_VERTEX_BYTES];
        self.put(&mut b);
        b
    }

    /// The vertex's bytes into `b` (each field at its fixed place: no copy of a run of unknown length).
    fn put(&self, b: &mut [u8; HD_VERTEX_BYTES]) {
        b[0..4].copy_from_slice(&self.position[0].to_le_bytes());
        b[4..8].copy_from_slice(&self.position[1].to_le_bytes());
        b[8..12].copy_from_slice(&self.position[2].to_le_bytes());
        b[12..14].copy_from_slice(&self.normal[0].to_le_bytes());
        b[14..16].copy_from_slice(&self.normal[1].to_le_bytes());
        b[16..20].copy_from_slice(&self.uv[0].to_le_bytes());
        b[20..24].copy_from_slice(&self.uv[1].to_le_bytes());
        b[24..28].copy_from_slice(&self.color);
        b[28..32].copy_from_slice(&self.material.to_le_bytes());
        b[32..36].copy_from_slice(&self.extra.to_le_bytes());
    }

    /// [`HdVertex::extend_bytes`] with `base` added to each [`material::NODE`] vertex's place (its object's first place in the
    /// frame's table of [`NodePlace`]s).
    pub fn extend_bytes_based(vertices: &[HdVertex], base: u32, out: &mut Vec<u8>) {
        let at = out.len();
        HdVertex::extend_bytes(vertices, out);
        if base == 0 {
            return;
        }
        for (v, chunk) in vertices
            .iter()
            .zip(out[at..].as_chunks_mut::<HD_VERTEX_BYTES>().0)
        {
            if v.material & material::NODE != 0 {
                chunk[32..36].copy_from_slice(&v.extra.wrapping_add(base).to_le_bytes());
            }
        }
    }

    /// `vertices`' bytes added to `out` (as [`HdVertex::bytes`] one after another).
    pub fn extend_bytes(vertices: &[HdVertex], out: &mut Vec<u8>) {
        let at = out.len();
        out.resize(at + vertices.len() * HD_VERTEX_BYTES, 0);
        for (v, chunk) in vertices
            .iter()
            .zip(out[at..].as_chunks_mut::<HD_VERTEX_BYTES>().0)
        {
            v.put(chunk);
        }
    }
}

/// A unit normal as two snorm16 octahedral coordinates (Cigolle et al. 2014's mapping).
pub fn oct_encode(n: [f32; 3]) -> [i16; 2] {
    let l1 = n[0].abs() + n[1].abs() + n[2].abs();
    if l1.is_nan() || l1 <= 0.0 || !l1.is_finite() {
        return [0, 0];
    }
    let (mut x, mut y) = (n[0] / l1, n[1] / l1);
    if n[2] < 0.0 {
        let sign = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
        let (ox, oy) = (x, y);
        x = (1.0 - oy.abs()) * sign(ox);
        y = (1.0 - ox.abs()) * sign(oy);
    }
    let q = |v: f32| round_half_away(v.clamp(-1.0, 1.0) * 32767.0) as i16;
    [q(x), q(y)]
}

/// `v.round() as i32` for `|v|` under 2^23 (NaN gives 0), without the C library's `roundf` (a call a vertex on Windows: the
/// moving shapes' normals spent 3% of a frame in it). `v - trunc(v)` is exact there, so the halves round as `round` rounds them.
fn round_half_away(v: f32) -> i32 {
    let t = v as i32;
    let f = v - t as f32;
    t + i32::from(f >= 0.5) - i32::from(f <= -0.5)
}

/// The work one frame's corona tests against the shapes may do (a server decides the shapes and the lights): a shape's bounding
/// sphere, a draw and a triangle looked at are a unit each. Past it a corona is left as the static geometry's line of sight left
/// it (drawn).
pub const MAX_CORONA_SHAPE_WORK: u64 = 1 << 20;
/// How near the light a shape's surface may be without hiding its corona (units): a light held in a hand sits in the hand.
pub const CORONA_SHAPE_MARGIN: f32 = 0.3;

/// Whether an opaque shape (a Player, a held item, a vehicle: every object's opaque, depth-writing draws) lies on the line of
/// sight from `eye` to a corona at `target`, short of the last [`CORONA_SHAPE_MARGIN`] before it. The coronas are drawn without
/// the depth test (a line-of-sight ray against the static geometry hides them), so the shapes would show them through. `work`
/// counts the spheres, draws and triangles looked at; past [`MAX_CORONA_SHAPE_WORK`] the answer is false (not hidden).
pub fn shapes_hide(
    objects: &[DynamicObject],
    eye: [f32; 3],
    target: [f32; 3],
    work: &mut u64,
) -> bool {
    if *work >= MAX_CORONA_SHAPE_WORK {
        return false;
    }
    let eye = Vec3::from(eye);
    let d = Vec3::from(target) - eye;
    let len = d.length();
    if !len.is_finite() || len <= CORONA_SHAPE_MARGIN {
        return false;
    }
    let t_max = 1.0 - CORONA_SHAPE_MARGIN / len;
    for o in objects {
        if *work >= MAX_CORONA_SHAPE_WORK {
            return false;
        }
        *work += 1;
        // The segment against the bounding sphere first.
        let oc = Vec3::from(o.centre) - eye;
        let s = (oc.dot(d) / (len * len)).clamp(0.0, t_max);
        let near = oc - d * s;
        // (A NaN sphere is skipped.)
        let within = near.length_squared() <= o.radius * o.radius;
        if !within {
            continue;
        }
        for draw in &o.draws {
            if *work >= MAX_CORONA_SHAPE_WORK {
                return false;
            }
            *work += 1;
            if draw.blend != Blend::Opaque
                || !draw.depth_write
                || draw
                    .vertices
                    .first()
                    .is_some_and(|v| material::shadow_only(v.material))
            {
                continue;
            }
            let v = &draw.vertices;
            let tri = |k: usize| -> Option<[usize; 3]> {
                if draw.indices.is_empty() {
                    (3 * k + 2 < v.len()).then_some([3 * k, 3 * k + 1, 3 * k + 2])
                } else {
                    let i = &draw.indices;
                    (3 * k + 2 < i.len())
                        .then(|| [i[3 * k], i[3 * k + 1], i[3 * k + 2]].map(|x| x as usize))
                }
            };
            let mut k = 0;
            while let Some(t) = tri(k) {
                k += 1;
                if *work >= MAX_CORONA_SHAPE_WORK {
                    return false;
                }
                *work += 1;
                let (Some(a), Some(b), Some(c)) = (v.get(t[0]), v.get(t[1]), v.get(t[2])) else {
                    continue;
                };
                let [a, b, c] = [a, b, c].map(|p| Vec3::from(o.world_point(p)));
                // Möller-Trumbore, both faces.
                let (e1, e2) = (b - a, c - a);
                let p = d.cross(e2);
                let det = e1.dot(p);
                if det.abs() < 1e-12 {
                    continue;
                }
                let inv = 1.0 / det;
                let tv = eye - a;
                let u = tv.dot(p) * inv;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let q = tv.cross(e1);
                let w = d.dot(q) * inv;
                if w < 0.0 || u + w > 1.0 {
                    continue;
                }
                let t = e2.dot(q) * inv;
                if t > 1e-5 && t < t_max {
                    return true;
                }
            }
        }
    }
    false
}

/// The centre of the draws' box and the radius that holds every vertex.
pub fn bounding_sphere(draws: &[HdDraw]) -> ([f32; 3], f32) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for v in draws.iter().flat_map(|d| &d.vertices) {
        for k in 0..3 {
            lo[k] = lo[k].min(v.position[k]);
            hi[k] = hi[k].max(v.position[k]);
        }
    }
    if lo[0] > hi[0] {
        return ([0.0; 3], 0.0);
    }
    let c = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
    let r = draws
        .iter()
        .flat_map(|d| &d.vertices)
        .map(|v| {
            let d = [0, 1, 2].map(|k| v.position[k] - c[k]);
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .fold(0.0f32, f32::max);
    (c, r)
}

#[cfg(test)]
#[test]
fn lightmap_words_and_halves() {
    assert_eq!(lightmap_uv([0.0, 0.0]), 0);
    assert_eq!(lightmap_uv([1.0, 1.0]), 0xffff_ffff);
    assert_eq!(lightmap_uv([0.5, 2.0]) & 0xffff, 32768);
    assert_eq!(lightmap_uv([-1.0, f32::NAN]), 0);
    assert_eq!(half(1.0), 0x3c00);
    assert_eq!(half(4.0), 0x4400);
    assert_eq!(half(-2.0), 0xc000);
    assert_eq!(half(0.0), 0);
    assert_eq!(half(1e9), 0x7c00);
    assert_eq!(half(65504.0), 0x7bff);
    assert!(material::lightmapped(
        (material::KIND_PLANE << material::KIND_SHIFT) | material::LIGHTMAP
    ));
    assert!(!material::lightmapped(
        (material::KIND_SHAPE << material::KIND_SHIFT) | material::LIGHTMAP
    ));
    assert!(material::cutout(
        (material::KIND_SHAPE << material::KIND_SHIFT) | material::CUTOUT
    ));
    assert!(!material::cutout(
        (material::KIND_SKY << material::KIND_SHIFT) | material::CUTOUT
    ));
}

#[cfg(test)]
#[test]
fn rounding_without_roundf_matches_round() {
    let mut v = -40000.0f32;
    while v <= 40000.0 {
        for x in [
            v,
            v + 0.5,
            v - 0.5,
            v + 0.49999997,
            v + 0.50000006,
            -v - 0.5,
        ] {
            assert_eq!(round_half_away(x), x.round() as i32, "{x}");
        }
        v += 0.73;
    }
    assert_eq!(round_half_away(f32::NAN), 0);
    for x in [
        0.0,
        -0.0,
        0.5,
        -0.5,
        1.5,
        -1.5,
        32767.0,
        -32767.0,
        0.49999997,
        -0.49999997,
    ] {
        assert_eq!(round_half_away(x), x.round() as i32, "{x}");
    }
}

/// [`oct_encode`]'s inverse (what the shader computes).
pub fn oct_decode(e: [i16; 2]) -> [f32; 3] {
    let x = f32::from(e[0]) / 32767.0;
    let y = f32::from(e[1]) / 32767.0;
    let z = 1.0 - x.abs() - y.abs();
    let t = (-z).max(0.0);
    let x = x + if x >= 0.0 { -t } else { t };
    let y = y + if y >= 0.0 { -t } else { t };
    let l = (x * x + y * y + z * z).sqrt();
    [x / l, y / l, z / l]
}

/// The length under which a normal counts as zero (a foliage card's normals can be 0 to 7e-7 long).
pub const ZERO_NORMAL_LENGTH: f32 = 1e-4;

/// A unit normal and whether it was (about) zero.
pub fn unit_normal(n: [f32; 3]) -> ([f32; 3], bool) {
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l.is_finite() && l > ZERO_NORMAL_LENGTH {
        (n.map(|v| v / l), false)
    } else {
        ([0.0, 0.0, 1.0], true)
    }
}

/// A draw of the enhanced renderer: indexed triangles of [`HdVertex`]es, a texture from the scene's [`TextureTable`], the GL state's blend,
/// culling and depth.
#[derive(Debug, Clone, PartialEq)]
pub struct HdDraw {
    pub label: String,
    pub vertices: Vec<HdVertex>,
    pub indices: Vec<u32>,
    pub texture: Option<(usize, Sampler)>,
    pub blend: Blend,
    pub cull: Cull,
    pub depth_test: bool,
    pub depth_write: bool,
}

impl HdDraw {
    /// A fixed-function draw's triangles as an enhanced one: the same vertices (normals made unit, zero ones flagged), the GL colour, the
    /// texture environment and lighting as material bits of `kind`, its texture through `textures` (the frame's texture index to the table's).
    /// The same draw with `bits` set in every vertex's material word.
    pub fn with_material(mut self, bits: u32) -> HdDraw {
        for v in &mut self.vertices {
            v.material |= bits;
        }
        self
    }

    pub fn from_fixed(d: &Draw, kind: u32, textures: &[usize]) -> HdDraw {
        let mut h = HdDraw::empty();
        h.assign_fixed(d, kind, textures, 0);
        h
    }

    /// A draw of nothing, opaque, culled by nothing and depth tested and written: a start for [`HdDraw::assign_fixed`].
    pub fn empty() -> HdDraw {
        HdDraw {
            label: String::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
            texture: None,
            blend: Blend::Opaque,
            cull: Cull::None,
            depth_test: true,
            depth_write: true,
        }
    }

    /// [`HdDraw::from_fixed`] of `d` into this draw, its lists' room kept (a frame's particles made again into the last frame's),
    /// `bits` set in every vertex's material word.
    pub fn assign_fixed(&mut self, d: &Draw, kind: u32, textures: &[usize], bits: u32) {
        let mut base = (kind << material::KIND_SHIFT) | material::tex_env(d.tex_env) | bits;
        if d.texture.is_none() {
            base &= !(material::TEX_ENV_MASK << material::TEX_ENV_SHIFT);
        }
        if !d.lighting {
            base |= material::UNLIT;
        }
        self.vertices.clear();
        self.vertices.extend(d.vertices.iter().map(|v| {
            let (n, zero) = unit_normal(v.normal);
            HdVertex {
                position: v.position,
                normal: oct_encode(n),
                uv: v.uv,
                color: v.color,
                material: base | if zero { material::ZERO_NORMAL } else { 0 },
                extra: 0,
            }
        }));
        let n = (self.vertices.len() - self.vertices.len() % 3) as u32;
        self.indices.clear();
        self.indices.extend(0..n);
        self.label.clone_from(&d.label);
        self.texture = d
            .texture
            .and_then(|(i, s)| textures.get(i).map(|&g| (g, s)))
            .filter(|_| d.tex_env != TexEnv::None);
        self.blend = d.blend;
        self.cull = d.cull;
        self.depth_test = d.depth_test;
        self.depth_write = d.depth_write;
    }
}

/// A shape ghost: its draws in world space at its current transform, the transform and the one before (the motion vectors),
/// whether it has translucency, and its bounding sphere.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicObject {
    pub ghost: u32,
    pub transform: Mat,
    pub prev_transform: Mat,
    pub translucent: bool,
    pub centre: [f32; 3],
    pub radius: f32,
    pub draws: Vec<HdDraw>,
    /// Where each vertex of `draws` (in order, draw after draw) was in the world at the last frame: its animation's and its
    /// control pose's motion together (a walking person's swinging limbs). Set when the last frame's object of this ghost had draws
    /// of the same labels and vertex counts ([`DynamicObject::previous_from`]: a draw it lacked moves rigidly); None: its points
    /// moved rigidly with `prev_transform`.
    pub previous: Option<Vec<[f32; 3]>>,
    /// A control sync's stamp for these draws (the shape cache's): another object of the ghost with the same stamp has the
    /// same draws (the renderer leaves their vertices where it wrote them). None: made by a whole sync or by hand.
    pub stamp: Option<u64>,
    /// The places its draws' [`material::NODE`] vertices name, in order (empty: every vertex is in the world).
    pub nodes: Vec<NodePlace>,
}

/// Where one mesh of a shape's object is, for its vertices that stay in the mesh's own space ([`material::NODE`]; the GPU places
/// them, so a bot whose limbs swing keeps its vertices): its placement now and at the last frame (its node's pose times the
/// shape's placement: the motion of each limb), how its normals turn, and its vertices' sphere in its own space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodePlace {
    pub now: Mat,
    pub before: Mat,
    pub normal: Mat,
    pub sphere: [f32; 4],
    /// The kept build whose mesh it is (its call's stamp; 0: none): an object made afresh finds its meshes' last frame by it.
    pub build: u64,
}

impl NodePlace {
    /// Its sphere where it is now (the radius by the placement's largest scale).
    pub fn world_sphere(&self) -> ([f32; 3], f32) {
        let c = self
            .now
            .mul_p([self.sphere[0], self.sphere[1], self.sphere[2]]);
        let scale = (0..3)
            .map(|k| {
                let col = [self.now.m[k], self.now.m[4 + k], self.now.m[8 + k]];
                (col[0] * col[0] + col[1] * col[1] + col[2] * col[2]).sqrt()
            })
            .fold(0.0f32, f32::max);
        (c, self.sphere[3] * scale)
    }
}

/// The centre of the box of `nodes`' spheres (and of `points`) and the radius that holds them.
pub fn nodes_sphere(nodes: &[NodePlace], points: &[[f32; 3]]) -> ([f32; 3], f32) {
    let spheres: Vec<([f32; 3], f32)> = nodes
        .iter()
        .map(NodePlace::world_sphere)
        .chain(points.iter().map(|p| (*p, 0.0)))
        .collect();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for (c, r) in &spheres {
        for k in 0..3 {
            lo[k] = lo[k].min(c[k] - r);
            hi[k] = hi[k].max(c[k] + r);
        }
    }
    if !(lo[0] <= hi[0] && lo[1] <= hi[1] && lo[2] <= hi[2]) {
        return ([0.0; 3], 0.0);
    }
    let c = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
    let r = spheres
        .iter()
        .map(|(p, r)| {
            let d = [0, 1, 2].map(|k| p[k] - c[k]);
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() + r
        })
        .fold(0.0f32, f32::max);
    (c, r)
}

/// The most vertices a shape's previous positions are kept for (a bigger one moves rigidly: [`DynamicObject::motion`]).
pub const MAX_PREVIOUS_VERTICES: usize = 1 << 18;

impl DynamicObject {
    /// Where vertex `v` of its draws is in the world now (a [`material::NODE`] one where its place puts it).
    pub fn world_point(&self, v: &HdVertex) -> [f32; 3] {
        if v.material & material::NODE == 0 {
            return v.position;
        }
        self.nodes
            .get(v.extra as usize)
            .map_or(v.position, |n| n.now.mul_p(v.position))
    }

    /// The object with every vertex in the world, as the vertex shaders place a [`material::NODE`] one (its normal turned and
    /// made unit again; the checks compare it with the object the CPU placed).
    pub fn placed(&self) -> DynamicObject {
        let mut o = self.clone();
        for d in &mut o.draws {
            for v in &mut d.vertices {
                if v.material & material::NODE == 0 {
                    continue;
                }
                if let Some(n) = self.nodes.get(v.extra as usize) {
                    v.position = n.now.mul_p(v.position);
                    let (u, zero) = unit_normal(n.normal.mul_v(oct_decode(v.normal)));
                    if !zero {
                        v.normal = oct_encode(u);
                    }
                }
                v.material &= !material::NODE;
                v.extra = 0;
            }
        }
        o.nodes.clear();
        o
    }

    /// The last frame's world positions of this object's vertices, from `before` (the same ghost's object then): each draw that
    /// `before` has too (the same label and vertex count, in the same order: an item put away or a part shown or hidden by its
    /// animation leaves the others matched) takes its vertices there; one it lacks moves rigidly with `prev_transform`. None when
    /// no draw matches or there are more than [`MAX_PREVIOUS_VERTICES`].
    pub fn previous_from(&self, before: &DynamicObject) -> Option<Vec<[f32; 3]>> {
        let n: usize = self.draws.iter().map(|d| d.vertices.len()).sum();
        // (Vertices in their meshes' own space bring their places' last frame instead.)
        if n > MAX_PREVIOUS_VERTICES || !self.nodes.is_empty() || !before.nodes.is_empty() {
            return None;
        }
        let rigid = Mat::mul(
            &self.prev_transform,
            &post::affine_inverse_scaled(&self.transform),
        );
        let still = self.prev_transform == self.transform;
        let mut out = Vec::with_capacity(n);
        let mut next = 0;
        let mut matched = false;
        for d in &self.draws {
            let found = before.draws[next.min(before.draws.len())..]
                .iter()
                .position(|b| b.label == d.label && b.vertices.len() == d.vertices.len())
                .map(|k| next + k);
            match found {
                Some(k) => {
                    out.extend(before.draws[k].vertices.iter().map(|v| v.position));
                    next = k + 1;
                    matched = true;
                }
                None => out.extend(d.vertices.iter().map(|v| {
                    if still {
                        v.position
                    } else {
                        rigid.mul_p(v.position)
                    }
                })),
            }
        }
        matched.then_some(out)
    }
}
