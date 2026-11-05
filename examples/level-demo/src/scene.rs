//! The client's ghosts as the world view's objects: each StaticShape and Player ghost whose datablock names a shape of the kit,
//! placed where its records put it.
//!
//! - **Cubes** (StaticShapes) are scaled as their records say. Their texture coordinates are made from where each vertex is in
//!   the world (one grid tile a unit, the face's two axes), so a long wall shows square tiles instead of one stretched grid, and
//!   each is tinted by its height: the ground gray-blue, higher cubes warmer. A cube's object is made once and kept until its
//!   record moves it.
//! - **Figures** (Players) are the engine's ([`GhostScene`]): the client's own where it predicts it (the cubes are its
//!   collision world, [`torque_example_common::prediction`]), each playing the actions its shape has for the way it moves
//!   (`root`, `run`, `back`, `side`, the jump and the fall) as TGE's client picks them.
//! - **Nearest texels** ([`crate::levels::nearest`]): the cubes' tiles and every texture of the figures and what they hold.
//! - **The camera's line of sight** ([`WorldScene::sight_hit`]) meets the cubes' boxes and the interiors' hulls (each
//!   InteriorInstance's `.dif` read as the server reads it, placed where its record puts it), so the camera behind the figure
//!   stays in front of a wall or a ceiling. It passes glass: a pane's own hull is left out, so the camera looks at the
//!   figure through a glass wall instead of jumping in front of it, while the walls and frames around the pane still stop
//!   it.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use torque::app::ghosts::{FrameInput, GhostScene, OwnView};
use torque::app::view::{SceneShape, WorldView};
use torque::client::conn::tge::TgeWorld;
use torque::client::predict::ghost_players::GhostPlayers;
use torque::collide::map::interior::{InteriorGeom, InteriorShape, hull_of};
use torque::game::scene::interior::{KeptInterior, read_interior};
use torque::ghost_world::world::Pose;
use torque::ghosts::{TgeClass, TgeRecord};
use torque_example_common::KitAssets;
use torque_example_common::prediction::placed;

use crate::levels::LevelStyle;
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::source::Eye;
use torque_render_scene::{Draw, Vertex};

/// A cube's object, kept with the pose it was made at.
struct KeptCube {
    pose: Pose,
    object: DynamicObject,
    /// Its box in the world (min, max): for the camera's line of sight.
    bounds: ([f32; 3], [f32; 3]),
}

/// An interior's hulls in the world, kept with the pose they were placed at.
struct PlacedInterior {
    pose: Pose,
    shape: InteriorShape,
}

/// The work an interior's hulls may cost to make the first time, the work one line of sight may cost, the most interior
/// files kept, and the most read in one update (the others wait for the next).
const INTERIOR_BUDGET: u64 = 50_000_000;
const SIGHT_BUDGET: u64 = 1_000_000;
const MAX_INTERIOR_FILES: usize = 64;
const MAX_INTERIOR_READS: usize = 2;

/// The objects of the client's ghosts, frame after frame.
pub struct WorldScene {
    ghosts: GhostScene<LevelStyle>,
    shapes: HashMap<String, Option<SceneShape>>,
    cubes: BTreeMap<u32, KeptCube>,
    /// The interior files read (by path; `None` for one that did not read) and each interior ghost's hulls.
    interior_files: HashMap<String, Option<KeptInterior>>,
    interiors: BTreeMap<u32, PlacedInterior>,
}

impl Default for WorldScene {
    fn default() -> WorldScene {
        WorldScene {
            ghosts: GhostScene::with_style(torque_example_common::ghost_options(), LevelStyle),
            shapes: HashMap::new(),
            cubes: BTreeMap::new(),
            interior_files: HashMap::new(),
            interiors: BTreeMap::new(),
        }
    }
}

/// `kept`'s hulls without the see-through ones (glass), for the camera's line of sight; as it is when the file has none. A
/// hull is see-through when a see-through face of the file lies on one of its own planes, facing the same way, with the
/// face's middle in the hull: the pane's own hull. A wall, sill or frame the pane's edge touches meets that face with a plane
/// facing the other way, and stays.
fn without_glass(bytes: &[u8], kept: KeptInterior) -> KeptInterior {
    use torque::mapcore::Importer as _;
    use torque::maps::dif::{PLANE_FLIPPED, surface_flags};
    let (Ok(dif), Ok(set)) = (
        torque::maps::dif::parse(bytes),
        torque::maps::legacy::IMPORTER.convexes(bytes),
    ) else {
        return kept;
    };
    let Some(i) = dif.details.first() else {
        return kept;
    };
    // Each see-through face: its middle and its outward normal.
    let faces: Vec<([f64; 3], [f64; 3])> = i
        .surfaces
        .iter()
        .filter(|f| f.flags & surface_flags::TRANSLUCENT != 0)
        .filter_map(|f| {
            let plane = i.planes.get(usize::from(f.plane & !PLANE_FLIPPED))?;
            let sign = if f.plane & PLANE_FLIPPED != 0 {
                -1.0
            } else {
                1.0
            };
            let normal = plane.normal.map(|c| sign * f64::from(c));
            let start = f.winding_start as usize;
            let corners = i.windings.get(start..start + f.winding_count as usize)?;
            let mut sum = [0.0f64; 3];
            for &c in corners {
                let p = i.points.get(c as usize)?;
                for k in 0..3 {
                    sum[k] += f64::from(p[k]);
                }
            }
            let n = corners.len().max(1) as f64;
            Some((sum.map(|x| x / n), normal))
        })
        .collect();
    if faces.is_empty() {
        return kept;
    }
    const ON: f64 = 1e-3;
    let pane = |h: &torque::collide::map::interior::Hull| {
        faces.iter().any(|(m, n)| {
            let side = |p: &[f64; 4]| p[0] * m[0] + p[1] * m[1] + p[2] * m[2] + p[3];
            let inside = h.planes.iter().all(|p| {
                let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-12);
                side(p) / len <= ON
            });
            inside
                && h.planes.iter().any(|p| {
                    let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-12);
                    let facing = (p[0] * n[0] + p[1] * n[1] + p[2] * n[2]) / len;
                    (side(p) / len).abs() <= ON && facing > 0.99
                })
        })
    };
    let mut glass = 0;
    let hulls = (0..set.hulls.len())
        .filter_map(|k| hull_of(&set, k))
        .filter(|h| {
            let see_through = pane(h);
            glass += usize::from(see_through);
            !see_through
        })
        .collect();
    if glass == 0 {
        return kept;
    }
    KeptInterior {
        geom: Arc::new(InteriorGeom::from_hulls(hulls)),
        ..kept
    }
}

/// The id of a cube's object among the frame's objects (the engine's objects are their ghosts' indexes).
fn cube_id(index: u32) -> u32 {
    0x4000_0000 | index
}

impl WorldScene {
    /// Forgets every ghost's object (a new connection's ghosts are new).
    pub fn clear(&mut self) {
        self.ghosts.clear();
        self.cubes.clear();
        self.interiors.clear();
    }

    /// How far along the unit `dir` from `from` the line first meets a cube's box or an interior's hull, if it does within
    /// `reach` (a box the line starts in does not count).
    pub fn sight_hit(&self, from: [f32; 3], dir: [f32; 3], reach: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        let mut take = |t: f32| {
            if t < reach && best.is_none_or(|b| t < b) {
                best = Some(t);
            }
        };
        for (min, max) in self.cube_bounds() {
            if let Some(t) = crate::client::ray_box(from, dir, min, max) {
                take(t);
            }
        }
        let end = [0, 1, 2].map(|i| from[i] + dir[i] * reach);
        let mut work = 0u64;
        for placed in self.interiors.values() {
            let mut pay = |u: u64| {
                work = work.saturating_add(u);
                work <= SIGHT_BUDGET
            };
            if let Ok(Some((t, _))) = placed.shape.cast_ray(from, end, &mut pay) {
                take(t * reach);
            }
        }
        best
    }

    /// The share of the way from `from` to `to` at which the line of sight first meets something ([`WorldScene::sight_hit`]).
    pub fn sight_share(&self, from: [f32; 3], to: [f32; 3]) -> Option<f32> {
        let d = [0, 1, 2].map(|i| to[i] - from[i]);
        let reach = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if reach < 1e-5 {
            return None;
        }
        self.sight_hit(from, d.map(|x| x / reach), reach)
            .map(|t| t / reach)
    }

    /// The interior ghosts of `world` placed for the camera's line of sight (each file read once).
    fn place_interiors(&mut self, assets: &KitAssets, world: &TgeWorld) {
        let mut seen = Vec::new();
        let mut reads = 0;
        for (&index, ghost) in &world.records.ghosts {
            let (Some(TgeRecord::InteriorInstance(r)), Some(pose)) = (&ghost.record, ghost.pose())
            else {
                continue;
            };
            let Some(init) = &r.init else { continue };
            seen.push(index);
            if self.interiors.get(&index).is_some_and(|k| k.pose == pose) {
                continue;
            }
            let path = String::from_utf8_lossy(&init.file).replace('\\', "/");
            if !self.interior_files.contains_key(&path)
                && self.interior_files.len() < MAX_INTERIOR_FILES
                && reads < MAX_INTERIOR_READS
            {
                reads += 1;
                let mut work = 0u64;
                let read = torque_render_scene::Assets::read(assets, &path).and_then(|bytes| {
                    read_interior(&bytes, &mut |u| {
                        work = work.saturating_add(u);
                        work <= INTERIOR_BUDGET
                    })
                    .map(|kept| without_glass(&bytes, kept))
                });
                self.interior_files.insert(path.clone(), read);
            }
            let Some(Some(kept)) = self.interior_files.get(&path) else {
                continue;
            };
            let Some((m, scale)) = placed(&pose) else {
                continue;
            };
            if let Some(shape) = InteriorShape::new(Arc::clone(&kept.geom), m.m, scale) {
                self.interiors.insert(index, PlacedInterior { pose, shape });
            }
        }
        self.interiors.retain(|i, _| seen.contains(i));
    }

    fn shape(&mut self, assets: &KitAssets, path: &[u8]) -> Option<SceneShape> {
        let path = String::from_utf8_lossy(path).replace('\\', "/");
        self.shapes
            .entry(path.clone())
            .or_insert_with(|| SceneShape::load(assets, &path))
            .clone()
    }

    /// The boxes of the cubes, for the camera's line of sight.
    pub fn cube_bounds(&self) -> impl Iterator<Item = ([f32; 3], [f32; 3])> + '_ {
        self.cubes.values().map(|c| c.bounds)
    }

    /// The client's own figure as the last frame placed it.
    pub fn own(&self) -> Option<OwnView> {
        self.ghosts.own()
    }

    /// The frame's motion by `dt` seconds, with nothing drawn: the figures posed where the client runs them.
    pub fn update(
        &mut self,
        assets: &KitAssets,
        world: &mut TgeWorld,
        ghosts: &GhostPlayers,
        dt: f32,
    ) {
        let input = FrameInput {
            ghosts: Some(ghosts),
            fov: None,
            dt,
            surfaces: &|_, _, _| Vec::new(),
        };
        self.ghosts.update(assets, world, &input);
        self.place_interiors(assets, world);
    }

    /// The frame's objects: the figures as [`WorldScene::update`] left them and `world`'s cubes.
    pub fn objects(
        &mut self,
        view: &mut WorldView,
        assets: &KitAssets,
        world: &TgeWorld,
        eye: &Eye,
    ) -> Vec<DynamicObject> {
        let mut out = self.ghosts.objects(view, assets, world);
        for object in &mut out {
            for draw in &mut object.draws {
                if let Some((_, sampler)) = draw.texture.as_mut() {
                    crate::levels::nearest(sampler);
                }
            }
        }
        let mut seen = Vec::new();
        for (&index, ghost) in &world.records.ghosts {
            let (Some(TgeClass::StaticShape), Some(_), Some(db)) =
                (ghost.class, ghost.record.as_ref(), ghost.datablock())
            else {
                continue;
            };
            let Some(file) = world.datablock_text(db, "shapeFile") else {
                continue;
            };
            let Some(pose) = ghost.pose() else { continue };
            seen.push(index);
            if self.cubes.get(&index).is_some_and(|c| c.pose == pose) {
                continue;
            }
            let Some(shape) = self.shape(assets, file) else {
                continue;
            };
            if let Some(cube) = make_cube(view, assets, &shape, pose, index) {
                self.cubes.insert(index, cube);
            }
        }
        self.cubes.retain(|i, _| seen.contains(i));
        out.extend(self.cubes.values().map(|c| c.object.clone()));
        self.ghosts.effects(view, assets, eye);
        out
    }
}

/// The tint of a cube whose top is at `top`: the ground gray-blue, warmer the higher it reaches.
pub fn cube_tint(top: f32) -> [u8; 3] {
    let t = (top / 6.0).clamp(0.0, 1.0);
    let ground = [196.0, 206.0, 222.0];
    let high = [236.0, 178.0, 120.0];
    let low = [218.0, 214.0, 200.0];
    let mix = |a: [f32; 3], b: [f32; 3], k: f32| {
        [0, 1, 2].map(|i| (a[i] + (b[i] - a[i]) * k).round() as u8)
    };
    if top <= 0.05 {
        mix(ground, ground, 0.0)
    } else {
        let a = mix(low, high, t);
        a.map(|v| v.max(1))
    }
}

/// Texture coordinates from where `v` is: the two world axes across the face its normal points out of, one tile a unit (the
/// grid texture holds four tiles a side).
fn world_uv(v: &Vertex) -> [f32; 2] {
    let [nx, ny, nz] = v.normal.map(f32::abs);
    let [x, y, z] = v.position;
    let (u, w) = if nz >= nx && nz >= ny {
        (x, y)
    } else if nx >= ny {
        (y, z)
    } else {
        (x, z)
    };
    [u / 4.0, -w / 4.0]
}

fn retexture(d: &mut Draw, tint: [u8; 3]) {
    for v in &mut d.vertices {
        v.uv = world_uv(v);
        v.color = [tint[0], tint[1], tint[2], 255];
    }
    if let Some((_, sampler)) = d.texture.as_mut() {
        sampler.repeat = true;
        crate::levels::nearest(sampler);
    }
}

fn make_cube(
    view: &mut WorldView,
    assets: &KitAssets,
    shape: &SceneShape,
    pose: Pose,
    index: u32,
) -> Option<KeptCube> {
    let (m, scale) = pose_matrix(&pose, None)?;
    let instance = shape.instance()?;
    // The box around the turned cube's eight corners.
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for corner in 0..8 {
        let local = [0, 1, 2].map(|i| {
            let sign = if corner & (1 << i) == 0 { -0.5 } else { 0.5 };
            sign * scale[i]
        });
        let p = m.mul_p(local);
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let bounds = (lo, hi);
    let tint = cube_tint(bounds.1[2]);
    let object = view.shape_object_with(
        assets,
        shape,
        &instance,
        &m,
        scale,
        cube_id(index),
        &mut |d| retexture(d, tint),
    );
    Some(KeptCube {
        pose,
        object,
        bounds,
    })
}
