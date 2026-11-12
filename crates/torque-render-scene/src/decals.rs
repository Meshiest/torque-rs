//! Decals: small marks laid flat on the surfaces of the world (a bolt's hole in a wall), as TGE's client keeps them in its
//! `DecalManager` from the `DecalData` a projectile names.
//!
//! - **The data** ([`DecalData`]): `DecalData::packData`'s `sizeX`, `sizeY` and `textureName` ([`decal_data`]). A
//!   ProjectileData names up to six (`decals[0..5]`); one of those it names is picked at random for each hit
//!   ([`projectile_decal`], `Projectile::explode` on the client). A projectile that hits a Player or a vehicle leaves none
//!   (the caller decides: [`DecalManager::add`] lays one wherever it is told).
//! - **Laid** ([`DecalManager::add_clipped`]): a box of `sizeX` by `sizeY` (and as deep as it is wide) centered on the hit
//!   point, its face in the plane of the surface's normal, turned about it by an angle the caller gives. The surface's
//!   triangles the caller found near the hit (`DecalManager::addDecal`'s poly list from the container) are clipped to the
//!   box, each that faces within [`CLIP_ANGLE_COS`] of the normal, and the decal is the pieces left, its texture laid on
//!   them across the box: on the face it hit, cut off at that face's edges, never hanging in the air. [`DecalManager::add`]
//!   lays the whole square (no surface known).
//! - **Kept**: each for [`DecalManager::lifetime`] seconds, fading out over its last [`DecalManager::fade`] seconds; past
//!   [`DecalManager::max`] the oldest goes first (`smMaxNumDecals`).
//! - **Drawn** ([`DecalManager::draws`]): each its triangles, textured (`GL_MODULATE`, white with the fade as its alpha),
//!   blended by alpha, depth-tested without depth writes, unlit and fogged, at its distance from the eye so the frame sorts
//!   it with the other translucent draws (the particles' list). Flush with the surface, it is drawn [`DEPTH_BIAS`] of its
//!   distance nearer the eye along the eye's ray (a depth bias: it moves nothing on the screen). A decal whose texture does
//!   not load is not drawn.
//! - **Multiplied** ([`DecalData::blend`] [`Blend::Modulate2x`], this crate's choice beside TGE's alpha blend): drawn the same
//!   way but multiplying the surface under it by twice its texture (0.5 gray leaves it as it is), so it takes the surface's
//!   color, light and shadow; it fades toward 0.5 gray.
//!
//! - **Bending** ([`DecalData::distortion`], the engine protocol's block): a decal whose datablock bends is also a distorter of its
//!   triangles, fading with it ([`DecalManager::distorters`]: those in the view, the nearest first within the frame's
//!   distortion budget); one drawn as the bend alone (`distortionOnly`) draws no texture.
//! - **Projected** ([`DecalManager::set_projected`], the enhanced renderer at Medium and up): the decals are kept as boxes the
//!   renderer projects onto the opaque frame ([`DecalManager::boxes`], [`crate::hd::decals`]), up to
//!   [`DEFAULT_PROJECTED_DECALS`] by default and never more than [`MAX_PROJECTED_DECALS`]; a box is laid with no mesh
//!   ([`DecalManager::add_box`]), and only one that bends is still clipped to its surface for its distorter (at most
//!   [`MAX_PROJECTED_MESHES`] of those keep their mesh). Their fade and lifetime are the CPU path's.
//!
//! **Approximations**: no wrapping onto a neighboring face; the bias, the fade and the multiplied blend are this crate's.
//!
//! Credit: follows TGE 1.4's `sim/decalManager.cc` and `game/projectile.cc` (Copyright (C) GarageGames.com, Inc.), as Torque3D
//! carries them under the MIT license: `T3D/decal/decalManager.cpp` and `T3D/decal/decalData.cpp` (each Copyright (c) 2012
//! GarageGames, LLC) and `T3D/projectile.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The
//! code here is this crate's own.

use std::collections::{HashMap, VecDeque};

use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;
use ghosts::Value;

use glam::Vec3;

use crate::distorters::DistortionData;
use crate::hd::decals::HdDecal;
use crate::hd::distortion::{DistortionBudget, HdDistorter, nearest_first};
use crate::hd::{HdDraw, HdVertex, oct_encode};
use crate::math::unit_over;
use crate::particles::Rng;
use crate::textures::TextureSet;
use crate::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

/// A decal's depth bias: each vertex drawn this share of its distance nearer the eye, along the eye's ray.
pub const DEPTH_BIAS: f32 = 0.002;
/// A surface triangle takes a decal when it faces within this of the decal's normal (the cosine of 80 degrees).
pub const CLIP_ANGLE_COS: f32 = 0.17;
/// `DecalManager::smMaxNumDecals`'s default.
pub const DEFAULT_MAX_DECALS: usize = 256;
/// How long a decal stays by default (seconds), and how long of that it fades out.
pub const DEFAULT_LIFETIME: f32 = 25.0;
pub const DEFAULT_FADE: f32 = 3.0;
/// The largest side a decal may have (units): a server's datablock names it.
pub const MAX_DECAL_SIZE: f32 = 64.0;
/// The decals the projected path keeps by default.
pub const DEFAULT_PROJECTED_DECALS: usize = 4096;
/// The most decals the projected path ever keeps, whatever a script, a host or a server asks for ([`DecalManager::set_projected`]
/// holds its cap to it).
pub const MAX_PROJECTED_DECALS: usize = 16384;
/// The most decals the projected path keeps with a mesh (the bending ones, clipped for their distorters: TGE's decal cap);
/// past it the oldest mesh is dropped and its decal kept as a box.
pub const MAX_PROJECTED_MESHES: usize = DEFAULT_MAX_DECALS;
/// How a decal's texture is sampled.
const DECAL_SAMPLER: Sampler = Sampler {
    repeat: false,
    mag: Filter::Linear,
    min: Filter::Linear,
    mip: Filter::Linear,
    mipmaps: true,
};

/// A `DecalData` as the client holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct DecalData {
    /// `sizeX`, `sizeY`.
    pub size: [f32; 2],
    /// `textureName`, `/`-separated; none: nothing is drawn.
    pub texture: Option<String>,
    /// How it is drawn over its surface: [`Blend::Alpha`] (TGE's; the default) or [`Blend::Modulate2x`].
    pub blend: Blend,
    /// How it bends the picture behind it (none: not at all).
    pub distortion: Option<DistortionData>,
}

/// The `DecalData` `id` of `world` (`DecalData::unpackData`): a size the server did not send is the constructor's 1.
pub fn decal_data(world: &ClientWorld<TgeClientSet>, id: u32) -> Option<DecalData> {
    let db = world
        .datablocks
        .get(&id)
        .filter(|d| d.class == "DecalData")?;
    let size = |name: &str| match db.get(name, 0) {
        Some(Value::F32(v)) if v.is_finite() => v.clamp(0.0, MAX_DECAL_SIZE),
        _ => 1.0,
    };
    let texture = match db.get("textureName", 0) {
        Some(Value::Str(s)) if !s.is_empty() => Some(String::from_utf8_lossy(s).replace('\\', "/")),
        _ => None,
    };
    Some(DecalData {
        size: [size("sizeX"), size("sizeY")],
        texture,
        blend: Blend::Alpha,
        distortion: DistortionData::read(|name, i| db.get(name, i)),
    })
}

/// One of the decals ProjectileData `projectile` names, picked at random (`Projectile::explode`); none when it names none.
pub fn projectile_decal(
    world: &ClientWorld<TgeClientSet>,
    projectile: u32,
    rng: &mut Rng,
) -> Option<DecalData> {
    let db = world.datablocks.get(&projectile)?;
    let named: Vec<u32> = (0..6)
        .filter_map(|i| match db.get("decals", i) {
            Some(Value::Id(Some(id))) => Some(*id),
            Some(Value::Int(v)) => u32::try_from(*v).ok().filter(|v| *v != 0),
            _ => None,
        })
        .collect();
    if named.is_empty() {
        return None;
    }
    let pick = rng.i(0, named.len() as i32 - 1) as usize;
    decal_data(world, named[pick])
}

/// A surface triangle (world space).
pub type Triangle = [[f32; 3]; 3];

/// A point of a decal's mesh: where it is and its texture coordinates.
pub type DecalPoint = ([f32; 3], [f32; 2]);

/// A decal laid in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct Decal {
    /// The square's center, on the surface.
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Half the square along each of its sides (the side vectors scaled by half the size).
    pub half_x: [f32; 3],
    pub half_y: [f32; 3],
    pub texture: Option<String>,
    /// [`DecalData::blend`].
    pub blend: Blend,
    /// [`DecalData::distortion`].
    pub distortion: Option<DistortionData>,
    /// Seconds since it was laid.
    pub age: f32,
    /// Its mesh: triangles on the surface.
    pub triangles: Vec<[DecalPoint; 3]>,
}

/// The part of the polygon `poly` where `dot(p - at, axis) <= limit` (Sutherland-Hodgman against one plane).
pub(crate) fn clip(poly: &[[f32; 3]], at: [f32; 3], axis: [f32; 3], limit: f32) -> Vec<[f32; 3]> {
    let (at, axis) = (Vec3::from(at), Vec3::from(axis));
    let side = |p: &[f32; 3]| limit - (Vec3::from(*p) - at).dot(axis);
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (da, db) = (side(&a), side(&b));
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            let (a, b) = (Vec3::from(a), Vec3::from(b));
            out.push((a + (b - a) * t).to_array());
        }
    }
    out
}

/// The client's decals, oldest first.
#[derive(Debug, Clone)]
pub struct DecalManager {
    decals: VecDeque<Decal>,
    /// The most kept at once on the CPU path.
    pub max: usize,
    /// The projected path's most kept at once (held to [`MAX_PROJECTED_DECALS`]); None: the CPU path.
    projected: Option<usize>,
    /// Seconds each is kept.
    pub lifetime: f32,
    /// Seconds of the lifetime's end over which each fades out.
    pub fade: f32,
}

impl Default for DecalManager {
    fn default() -> DecalManager {
        DecalManager {
            decals: VecDeque::new(),
            max: DEFAULT_MAX_DECALS,
            projected: None,
            lifetime: DEFAULT_LIFETIME,
            fade: DEFAULT_FADE,
        }
    }
}

/// `v` at unit length (None at 1e-6 or less, or not finite).
fn unit(v: Vec3) -> Option<Vec3> {
    unit_over(v, 1e-6)
}

impl DecalManager {
    /// A manager keeping at most `max` decals, each `lifetime` seconds, fading over its last `fade`.
    pub fn new(max: usize, lifetime: f32, fade: f32) -> DecalManager {
        DecalManager {
            max,
            lifetime,
            fade,
            ..DecalManager::default()
        }
    }

    /// The decals kept, oldest first.
    pub fn decals(&self) -> impl Iterator<Item = &Decal> {
        self.decals.iter()
    }

    pub fn len(&self) -> usize {
        self.decals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.decals.is_empty()
    }

    /// Forgets every decal.
    pub fn clear(&mut self) {
        self.decals.clear();
    }

    /// Keeps the decals for the projected path, at most `max` (held to [`MAX_PROJECTED_DECALS`]), or with None for the CPU
    /// path at [`DecalManager::max`]: going back to it drops the boxes laid with no mesh. Past the cap the oldest go.
    pub fn set_projected(&mut self, max: Option<usize>) {
        self.projected = max.map(|m| m.min(MAX_PROJECTED_DECALS));
        if self.projected.is_none() {
            self.decals.retain(|d| !d.triangles.is_empty());
        }
        let over = self.decals.len().saturating_sub(self.cap());
        self.decals.drain(..over);
    }

    /// The projected path's cap, or None on the CPU path ([`DecalManager::set_projected`]).
    pub fn projected(&self) -> Option<usize> {
        self.projected
    }

    /// The most decals kept now.
    fn cap(&self) -> usize {
        self.projected.unwrap_or(self.max)
    }

    /// Lays the whole square of a decal of `data` at `at` on a surface facing `normal`, turned `angle` radians about the
    /// normal (see [`DecalManager::add_clipped`]).
    pub fn add(&mut self, data: &DecalData, at: [f32; 3], normal: [f32; 3], angle: f32) {
        let Some(mut d) = self.lay(data, at, normal, angle) else {
            return;
        };
        let c = |sx: f32, sy: f32, uv: [f32; 2]| -> DecalPoint {
            (
                [0, 1, 2].map(|k| d.position[k] + d.half_x[k] * sx + d.half_y[k] * sy),
                uv,
            )
        };
        let v = [
            c(-1.0, -1.0, [0.0, 1.0]),
            c(1.0, -1.0, [1.0, 1.0]),
            c(1.0, 1.0, [1.0, 0.0]),
            c(-1.0, 1.0, [0.0, 0.0]),
        ];
        d.triangles = vec![[v[0], v[1], v[2]], [v[0], v[2], v[3]]];
        self.push(d);
    }

    /// Lays the box of a decal of `data` at `at` on a surface facing `normal`, turned `angle` radians about the normal, with
    /// no mesh: the projected path's ([`DecalManager::boxes`]); the CPU path draws nothing of it.
    pub fn add_box(&mut self, data: &DecalData, at: [f32; 3], normal: [f32; 3], angle: f32) {
        if data.size.iter().any(|s| s.is_nan() || *s <= 0.0) {
            return;
        }
        if let Some(d) = self.lay(data, at, normal, angle) {
            self.push(d);
        }
    }

    /// Lays a decal of `data` at `at` on a surface facing `normal`, turned `angle` radians about the normal, on the parts of
    /// `surface`'s triangles inside its box (see the module docs); the oldest goes when there are [`DecalManager::max`].
    /// Nothing for a normal of no length, a point off the map's numbers, or no surface under the box.
    pub fn add_clipped(
        &mut self,
        data: &DecalData,
        at: [f32; 3],
        normal: [f32; 3],
        angle: f32,
        surface: &[Triangle],
    ) {
        let Some(mut d) = self.lay(data, at, normal, angle) else {
            return;
        };
        let (hx, hy) = (0.5 * data.size[0], 0.5 * data.size[1]);
        if hx <= 0.0 || hy <= 0.0 {
            return;
        }
        let (Some(x), Some(y)) = (unit(d.half_x.into()), unit(d.half_y.into())) else {
            return;
        };
        let depth = hx.max(hy);
        let n = Vec3::from(d.normal);
        for tri in surface {
            let [a, b, c] = tri.map(Vec3::from);
            let Some(face) = unit((b - a).cross(c - a)) else {
                continue;
            };
            if face.dot(n) < CLIP_ANGLE_COS {
                continue;
            }
            let mut poly = tri.to_vec();
            for (axis, limit) in [(x, hx), (y, hy), (n, depth)] {
                poly = clip(&poly, at, axis.to_array(), limit);
                poly = clip(&poly, at, (-axis).to_array(), limit);
            }
            if poly.len() < 3 {
                continue;
            }
            let point = |p: [f32; 3]| -> DecalPoint {
                let r = Vec3::from(p) - Vec3::from(at);
                (p, [0.5 + 0.5 * r.dot(x) / hx, 0.5 - 0.5 * r.dot(y) / hy])
            };
            for i in 1..poly.len() - 1 {
                d.triangles
                    .push([point(poly[0]), point(poly[i]), point(poly[i + 1])]);
            }
        }
        if !d.triangles.is_empty() {
            self.push(d);
        }
    }

    fn push(&mut self, d: Decal) {
        let cap = self.cap();
        while self.decals.len() >= cap {
            self.decals.pop_front();
        }
        let meshed = !d.triangles.is_empty();
        self.decals.push_back(d);
        // On the projected path a mesh is only a bending decal's: their count is held apart, the oldest made a box.
        if meshed && self.projected.is_some() {
            let meshes = self
                .decals
                .iter()
                .filter(|d| !d.triangles.is_empty())
                .count();
            if meshes > MAX_PROJECTED_MESHES
                && let Some(oldest) = self.decals.iter_mut().find(|d| !d.triangles.is_empty())
            {
                oldest.triangles = Vec::new();
            }
        }
    }

    /// A decal's place and sides, with no mesh yet.
    fn lay(&self, data: &DecalData, at: [f32; 3], normal: [f32; 3], angle: f32) -> Option<Decal> {
        let n = unit(normal.into())?;
        if self.cap() == 0 || at.iter().any(|c| !c.is_finite()) {
            return None;
        }
        // A side in the surface's plane: across the world's up, or across x for a floor or a ceiling.
        let across = if n.z.abs() > 0.9 { Vec3::X } else { Vec3::Z };
        let u = unit(across.cross(n))?;
        let v = n.cross(u);
        let (s, c) = angle.sin_cos();
        let x = u * c + v * s;
        let y = v * c - u * s;
        Some(Decal {
            position: at,
            normal: n.to_array(),
            half_x: (x * data.size[0] * 0.5).to_array(),
            half_y: (y * data.size[1] * 0.5).to_array(),
            texture: data.texture.clone(),
            blend: data.blend,
            distortion: data.distortion.clone(),
            age: 0.0,
            triangles: Vec::new(),
        })
    }

    /// `dt` seconds on: each ages, and the ones past their lifetime go.
    pub fn advance(&mut self, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        for d in &mut self.decals {
            d.age += dt;
        }
        let lifetime = self.lifetime;
        self.decals.retain(|d| d.age < lifetime);
    }

    /// A decal's opacity now: 1 until its fade starts, then down to 0 at the end of its lifetime.
    pub fn opacity(&self, d: &Decal) -> f32 {
        let left = self.lifetime - d.age;
        if self.fade <= 0.0 {
            return if left > 0.0 { 1.0 } else { 0.0 };
        }
        (left / self.fade).clamp(0.0, 1.0)
    }

    /// The decals as draws, each at its distance from `eye` (see the module docs).
    pub fn draws(&self, eye: [f32; 3], textures: &mut TextureSet<'_>) -> Vec<(f32, Draw)> {
        let mut out = Vec::new();
        for d in self
            .decals
            .iter()
            .filter(|d| !d.distortion.as_ref().is_some_and(|x| x.only))
        {
            if d.triangles.is_empty() {
                continue;
            }
            let Some(texture) = d.texture.as_deref().and_then(|t| textures.get_modulated(t)) else {
                continue;
            };
            let alpha = (self.opacity(d) * 255.0).round() as u8;
            if alpha == 0 {
                continue;
            }
            let vertex = |(p, uv): DecalPoint| Vertex {
                position: [0, 1, 2].map(|k| p[k] + (eye[k] - p[k]) * DEPTH_BIAS),
                normal: d.normal,
                color: [255, 255, 255, alpha],
                uv,
            };
            let vertices = d
                .triangles
                .iter()
                .flat_map(|t| t.map(vertex))
                .collect::<Vec<_>>();
            let dist = (0..3)
                .map(|k| (d.position[k] - eye[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            out.push((
                dist,
                Draw {
                    label: "decal".to_string(),
                    vertices,
                    lighting: false,
                    tex_env: TexEnv::Modulate,
                    texture: Some((texture, DECAL_SAMPLER)),
                    blend: if d.blend == Blend::Modulate2x {
                        Blend::Modulate2x
                    } else {
                        Blend::Alpha
                    },
                    cull: Cull::None,
                    depth_test: true,
                    depth_write: false,
                    fog: true,
                    lights: Vec::new(),
                    vertex_key: None,
                },
            ));
        }
        out
    }

    /// The decals as the projected path's boxes (see [`crate::hd::decals`]), oldest first: each whose texture loads into
    /// `textures` (an index into it) and that is not drawn as the bend alone, at its opacity in 255ths as the CPU path's
    /// vertex alpha has it; one faded out is left out.
    pub fn boxes(&self, textures: &mut TextureSet<'_>) -> Vec<HdDecal> {
        // The decals share a few textures: each name is looked up once.
        let mut loaded: HashMap<&str, Option<usize>> = HashMap::new();
        let mut out = Vec::with_capacity(self.decals.len());
        for d in &self.decals {
            if d.distortion.as_ref().is_some_and(|x| x.only) {
                continue;
            }
            let Some(name) = d.texture.as_deref() else {
                continue;
            };
            let texture = *loaded
                .entry(name)
                .or_insert_with(|| textures.get_modulated(name));
            let Some(texture) = texture else {
                continue;
            };
            let alpha = (self.opacity(d) * 255.0).round();
            if alpha <= 0.0 {
                continue;
            }
            out.push(HdDecal {
                center: d.position,
                half_x: d.half_x,
                half_y: d.half_y,
                half_z: (Vec3::from(d.normal) * half_depth(d)).to_array(),
                texture: (texture, DECAL_SAMPLER),
                blend: if d.blend == Blend::Modulate2x {
                    Blend::Modulate2x
                } else {
                    Blend::Alpha
                },
                opacity: alpha / 255.0,
            });
        }
        out
    }

    /// The decals that bend as distorters: each its triangles at its opacity (the bend's share), both sides, drawn nearer `eye`
    /// as the draws are ([`DEPTH_BIAS`]), its map's index into `textures`. Only those `sees` may show (a sphere's center and
    /// radius), the nearest to `eye` first within a [`DistortionBudget`]: a server's datablocks decide how many bend.
    pub fn distorters(
        &self,
        eye: [f32; 3],
        sees: impl Fn([f32; 3], f32) -> bool,
        textures: &mut TextureSet<'_>,
    ) -> Vec<HdDistorter> {
        let candidates: Vec<&Decal> = self
            .decals
            .iter()
            .filter(|d| {
                d.distortion.is_some()
                    && !d.triangles.is_empty()
                    && (self.opacity(d) * 255.0).round() > 0.0
                    && sees(d.position, box_radius(d))
            })
            .collect();
        let order = nearest_first(eye, candidates.iter().map(|d| (d.position, box_radius(d))));
        let mut budget = DistortionBudget::default();
        let mut out = Vec::new();
        for k in order {
            if budget.is_spent() {
                break;
            }
            let d = candidates[k];
            let Some(data) = &d.distortion else {
                continue;
            };
            let n = 3 * d.triangles.len();
            if !budget.take(1, n, n) {
                continue;
            }
            let alpha = (self.opacity(d) * 255.0).round() as u8;
            let mut draw = HdDraw::empty();
            draw.label = "decal distortion".to_string();
            draw.depth_write = false;
            let normal = oct_encode(d.normal);
            draw.vertices
                .extend(d.triangles.iter().flatten().map(|&(p, uv)| HdVertex {
                    position: [0, 1, 2].map(|k| p[k] + (eye[k] - p[k]) * DEPTH_BIAS),
                    normal,
                    uv,
                    color: [255, 255, 255, alpha],
                    material: 0,
                    extra: 0,
                }));
            draw.indices = (0..draw.vertices.len() as u32).collect();
            out.push(HdDistorter::new(draw, data.look(textures)));
        }
        out
    }
}

/// A decal's box's half depth along its normal: its longer half side (the CPU path's clip).
fn half_depth(d: &Decal) -> f32 {
    Vec3::from(d.half_x)
        .length()
        .max(Vec3::from(d.half_y).length())
}

/// The radius of a sphere around a decal's center holding its box.
fn box_radius(d: &Decal) -> f32 {
    let z = half_depth(d);
    (Vec3::from(d.half_x).length_squared() + Vec3::from(d.half_y).length_squared() + z * z).sqrt()
}

#[cfg(test)]
#[path = "decals_tests.rs"]
mod tests;
