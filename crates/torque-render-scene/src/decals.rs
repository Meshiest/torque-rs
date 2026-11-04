//! Decals: small marks laid flat on the surfaces of the world (a bolt's hole in a wall), as TGE's client keeps them in its
//! `DecalManager` from the `DecalData` a projectile names.
//!
//! - **The data** ([`DecalData`]): `DecalData::packData`'s `sizeX`, `sizeY` and `textureName` ([`decal_data`]). A
//!   ProjectileData names up to six (`decals[0..5]`); one of those it names is picked at random for each hit
//!   ([`projectile_decal`], `Projectile::explode` on the client). A projectile that hits a Player or a vehicle leaves none
//!   (the caller decides: [`DecalManager::add`] lays one wherever it is told).
//! - **Laid** ([`DecalManager::add`]): a square of `sizeX` by `sizeY` centered on the hit point, in the plane of the surface's
//!   normal, turned about it by an angle the caller gives, and lifted [`DECAL_LIFT`] off the surface along the normal so it
//!   does not fight the surface in the depth buffer.
//! - **Kept**: each for [`DecalManager::lifetime`] seconds, fading out over its last [`DecalManager::fade`] seconds; past
//!   [`DecalManager::max`] the oldest goes first (`smMaxNumDecals`).
//! - **Drawn** ([`DecalManager::draws`]): each a textured square (`GL_MODULATE`, white with the fade as its alpha), blended by
//!   alpha, depth-tested without depth writes, unlit and fogged, at its distance from the eye so the frame sorts it with the
//!   other translucent draws (the particles' list). A decal whose texture does not load is not drawn.
//!
//! **Approximations**: the square is laid flat at the hit point and not clipped to the surface's polygons (a decal at an edge
//! hangs over it); the lift and the fade are this crate's.
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

/// How far a decal is lifted off its surface along the normal (units).
pub const DECAL_LIFT: f32 = 0.01;
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

/// A decal laid in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct Decal {
    /// The square's center, lifted off the surface.
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Half the square along each of its sides (the side vectors scaled by half the size).
    pub half_x: [f32; 3],
    pub half_y: [f32; 3],
    pub texture: Option<String>,
    /// Seconds since it was laid.
    pub age: f32,
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

    /// Lays a decal of `data` at `at` on a surface facing `normal`, turned `angle` radians about the normal; the oldest goes
    /// when there are [`DecalManager::max`]. Nothing for a normal of no length or a point off the map's numbers.
    pub fn add(&mut self, data: &DecalData, at: [f32; 3], normal: [f32; 3], angle: f32) {
        let Some(n) = unit(normal) else {
            return;
        };
        if self.max == 0 || at.iter().any(|c| !c.is_finite()) {
            return;
        }
        // A side in the surface's plane: across the world's up, or across x for a floor or a ceiling.
        let across = if n[2].abs() > 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        let Some(u) = unit(cross(across, n)) else {
            return;
        };
        let v = cross(n, u);
        let (s, c) = angle.sin_cos();
        let x = [0, 1, 2].map(|k| u[k] * c + v[k] * s);
        let y = [0, 1, 2].map(|k| v[k] * c - u[k] * s);
        while self.decals.len() >= self.max {
            self.decals.pop_front();
        }
        self.decals.push_back(Decal {
            position: [0, 1, 2].map(|k| at[k] + n[k] * DECAL_LIFT),
            normal: n,
            half_x: x.map(|e| e * data.size[0] * 0.5),
            half_y: y.map(|e| e * data.size[1] * 0.5),
            texture: data.texture.clone(),
            age: 0.0,
        });
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
            let corner = |sx: f32, sy: f32, uv: [f32; 2]| Vertex {
                position: [0, 1, 2].map(|k| d.position[k] + d.half_x[k] * sx + d.half_y[k] * sy),
                normal: d.normal,
                color: [255, 255, 255, alpha],
                uv,
            };
            let v = [
                corner(-1.0, -1.0, [0.0, 1.0]),
                corner(1.0, -1.0, [1.0, 1.0]),
                corner(1.0, 1.0, [1.0, 0.0]),
                corner(-1.0, 1.0, [0.0, 0.0]),
            ];
            let dist = (0..3)
                .map(|k| (d.position[k] - eye[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            out.push((
                dist,
                Draw {
                    label: "decal".to_string(),
                    vertices: vec![v[0], v[1], v[2], v[0], v[2], v[3]],
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
