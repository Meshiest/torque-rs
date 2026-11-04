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
//!
//! **Approximations**: no wrapping onto a neighboring face; the bias and the fade are this crate's.
//!
//! Credit: follows TGE 1.4's `sim/decalManager.cc` and `game/projectile.cc` (Copyright (C) GarageGames.com, Inc.), as Torque3D
//! carries them under the MIT license: `T3D/decal/decalManager.cpp` and `T3D/decal/decalData.cpp` (each Copyright (c) 2012
//! GarageGames, LLC) and `T3D/projectile.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The
//! code here is this crate's own.

use std::collections::VecDeque;

use ghost_world::tge::TgeClientSet;
use ghost_world::world::ClientWorld;
use ghosts::Value;

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

/// A `DecalData` as the client holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct DecalData {
    /// `sizeX`, `sizeY`.
    pub size: [f32; 2],
    /// `textureName`, `/`-separated; none: nothing is drawn.
    pub texture: Option<String>,
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
    /// Seconds since it was laid.
    pub age: f32,
    /// Its mesh: triangles on the surface.
    pub triangles: Vec<[DecalPoint; 3]>,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The part of the polygon `poly` where `dot(p - at, axis) <= limit` (Sutherland-Hodgman against one plane).
fn clip(poly: &[[f32; 3]], at: [f32; 3], axis: [f32; 3], limit: f32) -> Vec<[f32; 3]> {
    let side = |p: &[f32; 3]| limit - dot([0, 1, 2].map(|k| p[k] - at[k]), axis);
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (da, db) = (side(&a), side(&b));
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push([0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t));
        }
    }
    out
}

/// The client's decals, oldest first.
#[derive(Debug, Clone)]
pub struct DecalManager {
    decals: VecDeque<Decal>,
    /// The most kept at once.
    pub max: usize,
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
            lifetime: DEFAULT_LIFETIME,
            fade: DEFAULT_FADE,
        }
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = v.iter().map(|c| c * c).sum::<f32>().sqrt();
    (len > 1e-6 && len.is_finite()).then(|| v.map(|c| c / len))
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
        let (Some(x), Some(y)) = (unit(d.half_x), unit(d.half_y)) else {
            return;
        };
        let depth = hx.max(hy);
        let n = d.normal;
        for tri in surface {
            let e1 = [0, 1, 2].map(|k| tri[1][k] - tri[0][k]);
            let e2 = [0, 1, 2].map(|k| tri[2][k] - tri[0][k]);
            let Some(face) = unit(cross(e1, e2)) else {
                continue;
            };
            if dot(face, n) < CLIP_ANGLE_COS {
                continue;
            }
            let mut poly = tri.to_vec();
            for (axis, limit) in [(x, hx), (y, hy), (n, depth)] {
                poly = clip(&poly, at, axis, limit);
                poly = clip(&poly, at, axis.map(|e| -e), limit);
            }
            if poly.len() < 3 {
                continue;
            }
            let point = |p: [f32; 3]| -> DecalPoint {
                let r = [0, 1, 2].map(|k| p[k] - at[k]);
                (p, [0.5 + 0.5 * dot(r, x) / hx, 0.5 - 0.5 * dot(r, y) / hy])
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
        while self.decals.len() >= self.max {
            self.decals.pop_front();
        }
        self.decals.push_back(d);
    }

    /// A decal's place and sides, with no mesh yet.
    fn lay(&self, data: &DecalData, at: [f32; 3], normal: [f32; 3], angle: f32) -> Option<Decal> {
        let n = unit(normal)?;
        if self.max == 0 || at.iter().any(|c| !c.is_finite()) {
            return None;
        }
        // A side in the surface's plane: across the world's up, or across x for a floor or a ceiling.
        let across = if n[2].abs() > 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        let u = unit(cross(across, n))?;
        let v = cross(n, u);
        let (s, c) = angle.sin_cos();
        let x = [0, 1, 2].map(|k| u[k] * c + v[k] * s);
        let y = [0, 1, 2].map(|k| v[k] * c - u[k] * s);
        Some(Decal {
            position: at,
            normal: n,
            half_x: x.map(|e| e * data.size[0] * 0.5),
            half_y: y.map(|e| e * data.size[1] * 0.5),
            texture: data.texture.clone(),
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
        for d in &self.decals {
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
                    texture: Some((
                        texture,
                        Sampler {
                            repeat: false,
                            mag: Filter::Linear,
                            min: Filter::Linear,
                            mip: Filter::Linear,
                            mipmaps: true,
                        },
                    )),
                    blend: Blend::Alpha,
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
}

#[cfg(test)]
#[path = "decals_tests.rs"]
mod tests;
