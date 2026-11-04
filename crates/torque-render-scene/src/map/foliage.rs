//! A map's replicators (`fxFoliageReplicator`, `fxShapeReplicator`; the engine's own placement).
//!
//! **Placement.** Each replica is tried up to the object's retries times: a point at a random angle and a random distance between
//! the inner and outer radii (an ellipse each way) around the object, dropped from above onto the highest ground it may stand on:
//! the terrain when `AllowOnTerrain` (the first block under it; a hole, a point off a block that does not repeat, or a slope
//! steeper than `AllowedTerrainSlope` is a miss) and the interiors' hulls when `AllowOnInteriors` (the collision's rays,
//! [`collide::map`]); the ground a replicator may not stand on is passed through, as TGE's ray mask passes it. The draws come
//! from the engine's own random sequence seeded by the object's `Seed` (TGE's is not followed: the same map always places the same
//! way on the engine). Statics and water are not ground yet. A replicator's rays are bounded by [`PLACE_WORK`], and all of a
//! map's shape replicators' (and apart, its foliage replicators') by [`PLACE_WORK_TOTAL`].
//!
//! **Foliage** is drawn as billboards turning about z to face the camera, within `ViewDistance` of the eye (fading over
//! `FadeOutRegion` at its edge), the nearest [`MAX_BILLBOARDS`] of a replicator, back to front, blended, lit as the ground.
//! **Shape replicas** are drawn as their shapes at rest ([`super::statics`]).

use std::sync::Arc;

use collide::map::{InteriorGeom, InteriorShape};

use super::terrain::Block;
use super::{LABEL, MARK_MISSING, MISSING_COLOR, Object, SAMPLER, find_texture, folder_of};
use super::{MapClass, MapProfile};
use crate::math::Mat;
use crate::textures::TextureSet;
use crate::{Blend, Cull, Draw, TexEnv, Vertex};

/// The most replicas one replicator places (the samples' largest asks 20,000).
pub const MAX_REPLICAS: usize = 50_000;
/// The most replicas (shape replicas, and foliage billboards, each counted apart) all of a map's replicators place together: a
/// map with many replicators shares this, so the work and memory they cost is the engine's to bound, not the map's.
pub const MAX_REPLICAS_TOTAL: usize = 200_000;
/// The most ground tries all of a map's shape replicators make together, and its foliage replicators apart: a try costs the same
/// whether it lands or not, so the tries are what is counted.
pub const MAX_TRIES_TOTAL: usize = 2_000_000;
/// The most tries a replica gets.
pub const MAX_RETRIES: usize = 256;
/// The most billboards of one replicator drawn in a frame.
pub const MAX_BILLBOARDS: usize = 4_096;
/// The work the interior rays of one replicator may cost (`collide`'s units; the replicas past it are not placed).
pub const PLACE_WORK: u64 = 400_000_000;
/// The work the interior rays and box tests of all of a map's shape replicators may cost together, and of its foliage
/// replicators apart (each interior a try tests costs a unit): many replicators share it, as they share [`MAX_TRIES_TOTAL`].
pub const PLACE_WORK_TOTAL: u64 = 4_000_000_000;

/// The work the interiors' collision may cost one scene's replicas (`interior_ground`: a unit a byte read and each file's
/// hulls, `collide::map::interior::build_cost`); a file past it is not made, and the replicas do not land on it.
pub const GROUND_WORK: u64 = 4_000_000_000;

/// The interiors' collision, made only when a replicator may place on interiors (each `.dif` read once here, all of them
/// within [`GROUND_WORK`]).
pub(crate) fn interior_ground(
    objects: &[Object],
    profile: &MapProfile,
    assets: &dyn crate::Assets,
) -> Vec<InteriorShape> {
    let wanted = objects.iter().any(|o| {
        matches!(
            o.class,
            MapClass::FxFoliageReplicator | MapClass::FxShapeReplicator
        ) && o.flag("AllowOnInteriors")
    });
    if !wanted {
        return Vec::new();
    }
    let mut files: std::collections::HashMap<String, Option<Arc<InteriorGeom>>> =
        std::collections::HashMap::new();
    let mut out = Vec::new();
    let mut work = 0u64;
    let mut charge = |units: u64| {
        work = work.saturating_add(units);
        work <= GROUND_WORK
    };
    for o in objects
        .iter()
        .filter(|o| o.class == MapClass::InteriorInstance)
    {
        let name = o.text("interiorFile");
        let geom = files
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| {
                let bytes = assets.read(&name)?;
                if !charge(bytes.len() as u64) {
                    return None;
                }
                let set = profile.importer.convexes(&bytes).ok()?;
                charge(collide::map::interior::build_cost(&set))
                    .then(|| Arc::new(InteriorGeom::new(&set)))
            })
            .clone();
        let m = collide::map::xf_matrix(o.position, o.quat, o.negative_w);
        if let Some(g) = geom
            && let Some(s) = InteriorShape::new(g, m, o.scale())
        {
            out.push(s);
        }
    }
    out
}

/// The engine's random sequence (Numerical Recipes' 32-bit LCG).
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

/// The highest interior surface under (x, y) between `top` and `bottom`.
fn interior_at(
    interiors: &[InteriorShape],
    x: f32,
    y: f32,
    (top, bottom): (f32, f32),
    charge: &mut dyn FnMut(u64) -> bool,
) -> Result<Option<f32>, collide::Exhausted> {
    let mut best: Option<f32> = None;
    for i in interiors {
        if !charge(1) {
            return Err(collide::Exhausted);
        }
        let b = i.world_box();
        if x < b.min[0] || x > b.max[0] || y < b.min[1] || y > b.max[1] {
            continue;
        }
        if let Some((t, _)) = i.cast_ray([x, y, top], [x, y, bottom], charge)? {
            let z = top + t * (bottom - top);
            best = Some(best.map_or(z, |h: f32| h.max(z)));
        }
    }
    Ok(best)
}

/// The ground point a replica of `o` lands on with its terrain normal, for the replicator's radii; each try is taken from `tries`,
/// and none is made once it is spent; the work of its rays and box tests is added to `total`, and none is made past `cap`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn places(
    o: &Object,
    terrains: &[Block],
    interiors: &[InteriorShape],
    count: usize,
    retries: usize,
    tries: &mut usize,
    total: &mut u64,
    cap: u64,
) -> Vec<([f32; 3], f32)> {
    let mut out = Vec::new();
    let on_terrain = o.flag("AllowOnTerrain") && !terrains.is_empty();
    let on_interiors = o.flag("AllowOnInteriors") && !interiors.is_empty();
    if !on_terrain && !on_interiors {
        return out;
    }
    let span = interiors
        .iter()
        .fold((f32::NEG_INFINITY, f32::INFINITY), |(t, b), i| {
            let w = i.world_box();
            (t.max(w.max[2] + 1.0), b.min(w.min[2] - 1.0))
        });
    let mut work = 0u64;
    let mut charge = |u: u64| {
        work = work.saturating_add(u);
        *total = total.saturating_add(u);
        work <= PLACE_WORK && *total <= cap
    };
    let c = o.position();
    let mut rng = Rng(o.int("Seed") as u32);
    let radius = |inner: &str, outer: &str| {
        let (a, b) = (o.int(inner).max(0) as f32, o.int(outer).max(0) as f32);
        (a.min(b), a.max(b))
    };
    let (ix, ox) = radius("InnerRadiusX", "OuterRadiusX");
    let (iy, oy) = radius("InnerRadiusY", "OuterRadiusY");
    let slope = (o.int("AllowedTerrainSlope").clamp(0, 90) as f32)
        .to_radians()
        .cos();
    for _ in 0..count.min(MAX_REPLICAS) {
        for _ in 0..retries.clamp(1, MAX_RETRIES) {
            if *tries == 0 {
                return out;
            }
            *tries -= 1;
            let hx = rng.range(ix, ox);
            let hy = rng.range(iy, oy);
            let a = rng.range(0.0, std::f32::consts::TAU);
            let (x, y) = (c[0] + hx * a.cos(), c[1] + hy * a.sin());
            let ground = if on_terrain {
                terrains.iter().find_map(|b| b.surface(x, y))
            } else {
                None
            };
            let roof = if on_interiors {
                match interior_at(interiors, x, y, span, &mut charge) {
                    Ok(z) => z,
                    Err(_) => return out,
                }
            } else {
                None
            };
            let z = match (ground, roof) {
                (Some((g, _)), Some(r)) if r >= g => Some(r),
                (Some((g, n)), _) => (n[2] >= slope - 1e-6).then_some(g),
                (None, r) => r,
            };
            if let Some(z) = z {
                out.push(([x, y, z], rng.next()));
                break;
            }
        }
    }
    out
}

/// Each shape replica's shape, rotation and translation, and scale.
pub(crate) fn shape_replicas(
    objects: &[Object],
    terrains: &[Block],
    interiors: &[InteriorShape],
) -> Vec<(String, Mat, [f32; 3])> {
    let mut out = Vec::new();
    let mut placed_so_far = 0usize;
    let mut tries = MAX_TRIES_TOTAL;
    let mut work = 0u64;
    for o in objects
        .iter()
        .filter(|o| o.class == MapClass::FxShapeReplicator)
    {
        if o.flag("HideReplications") {
            continue;
        }
        let shape = o.text("ShapeFile");
        if placed_so_far >= MAX_REPLICAS_TOTAL || tries == 0 || work > PLACE_WORK_TOTAL {
            break;
        }
        let count = (o.int("ShapeCount").max(0) as usize).min(MAX_REPLICAS_TOTAL - placed_so_far);
        if count == 0 {
            continue;
        }
        let retries = o.int("ShapeRetries").max(1) as usize;
        let placed = places(
            o,
            terrains,
            interiors,
            count,
            retries,
            &mut tries,
            &mut work,
            PLACE_WORK_TOTAL,
        );
        placed_so_far += count.min(MAX_REPLICAS);
        let (smin, smax) = (o.point("ShapeScaleMin"), o.point("ShapeScaleMax"));
        let (rmin, rmax) = (o.point("ShapeRotateMin"), o.point("ShapeRotateMax"));
        let offset = o.int("OffsetZ") as f32;
        let fixed = o.flag("FixShapeAspect");
        let mut rng = Rng((o.int("Seed") as u32) ^ 0x5bd1_e995);
        for (p, _) in placed {
            let t = rng.next();
            let scale: [f32; 3] = std::array::from_fn(|k| {
                let f = if fixed { t } else { rng.next() };
                smin[k] + (smax[k] - smin[k]) * f
            });
            let rx = rng.range(rmin[0], rmax[0]).to_radians();
            let rz = rng.range(rmin[2], rmax[2]).to_radians();
            let mut m = Mat::mul(&Mat::euler_z(rz), &Mat::euler_x(rx));
            m.set_column3([p[0], p[1], p[2] + offset]);
            if scale.iter().all(|s| s.is_finite() && *s != 0.0) {
                out.push((shape.clone(), m, scale));
            }
        }
    }
    out
}

/// One foliage replicator's billboards.
pub(crate) struct Set {
    pub centre: [f32; 3],
    texture: Option<String>,
    name: String,
    /// Bottom centre, width, height, mirrored.
    items: Vec<([f32; 3], f32, f32, bool)>,
    view: f32,
    fade: f32,
}

pub(crate) fn sets(
    objects: &[Object],
    terrains: &[Block],
    interiors: &[InteriorShape],
    textures: &mut TextureSet<'_>,
) -> Vec<Set> {
    let mut out = Vec::new();
    let mut placed_so_far = 0usize;
    let mut tries = MAX_TRIES_TOTAL;
    let mut work = 0u64;
    for o in objects
        .iter()
        .filter(|o| o.class == MapClass::FxFoliageReplicator)
    {
        if o.flag("HideFoliage") {
            continue;
        }
        let name = o.text("FoliageFile");
        let found = find_texture(textures, &name, &[folder_of(&name)]);
        let aspect = found
            .as_ref()
            .and_then(|(t, _)| {
                Some(textures.height(*t)? as f32 / textures.width(*t)?.max(1) as f32)
            })
            .unwrap_or(1.0);
        if placed_so_far >= MAX_REPLICAS_TOTAL || tries == 0 || work > PLACE_WORK_TOTAL {
            break;
        }
        let count = (o.int("FoliageCount").max(0) as usize).min(MAX_REPLICAS_TOTAL - placed_so_far);
        if count == 0 {
            continue;
        }
        let retries = o.int("FoliageRetries").max(1) as usize;
        let placed = places(
            o,
            terrains,
            interiors,
            count,
            retries,
            &mut tries,
            &mut work,
            PLACE_WORK_TOTAL,
        );
        placed_so_far += count.min(MAX_REPLICAS);
        let (wmin, wmax) = (o.f32("MinWidth"), o.f32("MaxWidth"));
        let (hmin, hmax) = (o.f32("MinHeight"), o.f32("MaxHeight"));
        let offset = o.f32("OffsetZ");
        let mut rng = Rng((o.int("Seed") as u32) ^ 0x9e37_79b9);
        let items = placed
            .into_iter()
            .map(|(p, _)| {
                let (w, h) = if o.flag("FixSizeToMax") {
                    (wmax, hmax)
                } else {
                    let w = rng.range(wmin, wmax);
                    let h = if o.flag("FixAspectRatio") {
                        w * aspect
                    } else {
                        rng.range(hmin, hmax)
                    };
                    (w, h)
                };
                let flip = o.flag("RandomFlip") && rng.next() < 0.5;
                ([p[0], p[1], p[2] + offset], w.abs(), h.abs(), flip)
            })
            .filter(|i| i.1.is_finite() && i.2.is_finite())
            .collect();
        out.push(Set {
            centre: o.position(),
            texture: found.map(|(_, n)| n),
            name,
            items,
            view: o.f32("ViewDistance").max(0.0),
            fade: o.f32("FadeOutRegion").max(0.0),
        });
    }
    out
}

/// This frame's billboards of `set` seen from `eye`.
pub(crate) fn draw(set: &Set, eye: [f32; 3], textures: &mut TextureSet<'_>) -> Option<Draw> {
    let texture = set.texture.as_deref().and_then(|n| textures.get(n));
    let mark = MARK_MISSING.load(std::sync::atomic::Ordering::Relaxed);
    if texture.is_none() && !mark {
        return None;
    }
    let mut near: Vec<(f32, usize)> = set
        .items
        .iter()
        .enumerate()
        .filter_map(|(k, (p, ..))| {
            let d = ((p[0] - eye[0]).powi(2) + (p[1] - eye[1]).powi(2) + (p[2] - eye[2]).powi(2))
                .sqrt();
            (d < set.view).then_some((d, k))
        })
        .collect();
    if near.is_empty() {
        return None;
    }
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    near.truncate(MAX_BILLBOARDS);
    near.reverse();
    let mut vertices = Vec::with_capacity(near.len() * 6);
    for (d, k) in near {
        let (p, w, h, flip) = set.items[k];
        let (dx, dy) = (eye[0] - p[0], eye[1] - p[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-6);
        // Right of the billboard as the camera sees it.
        let r = [-dy / l * w * 0.5, dx / l * w * 0.5];
        let alpha = if set.fade > 0.0 {
            ((set.view - d) / set.fade).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let color = if texture.is_some() {
            [255, 255, 255, (alpha * 255.0).round() as u8]
        } else {
            MISSING_COLOR
        };
        let (u0, u1) = if flip { (1.0, 0.0) } else { (0.0, 1.0) };
        let v = |x: f32, y: f32, z: f32, u: f32, t: f32| Vertex {
            position: [x, y, z],
            normal: [0.0, 0.0, 1.0],
            color,
            uv: [u, t],
        };
        let q = [
            v(p[0] - r[0], p[1] - r[1], p[2], u0, 1.0),
            v(p[0] - r[0], p[1] - r[1], p[2] + h, u0, 0.0),
            v(p[0] + r[0], p[1] + r[1], p[2] + h, u1, 0.0),
            v(p[0] + r[0], p[1] + r[1], p[2], u1, 1.0),
        ];
        vertices.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
    }
    Some(Draw {
        label: format!(
            "{LABEL} foliage {}{}",
            set.name,
            if texture.is_none() {
                super::MISSING_SUFFIX
            } else {
                ""
            }
        ),
        vertices,
        lighting: texture.is_some(),
        tex_env: if texture.is_some() {
            TexEnv::Modulate
        } else {
            TexEnv::None
        },
        texture: texture.map(|t| {
            (
                t,
                crate::Sampler {
                    repeat: false,
                    ..SAMPLER
                },
            )
        }),
        blend: Blend::Alpha,
        cull: Cull::None,
        depth_test: true,
        depth_write: false,
        fog: texture.is_some(),
        lights: Vec::new(),
        vertex_key: None,
    })
}
