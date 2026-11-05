//! The client's ghosts as the world view's objects, and the eye.
//!
//! The figures, the pistols they hold (in the hand, and the client's own before its eye), the bolts with their trails and the
//! holes they leave are the engine's ([`GhostScene`]): the arena sets how they look ([`options`]: the recoil's kick, the
//! sights, a bolt's curve out of the muzzle, its growth and spin, ribbon trails, the shot's flare, multiplied bolt holes). The
//! arena draws its own:
//!
//! - **Cubes** (StaticShapes) are scaled as their records say, their texture coordinates made from where each vertex is (one
//!   grid tile a unit) and tinted by height. A cube's object is kept until its record moves it. A map that asks for sharp
//!   texels ([`TextureFilter::Nearest`], the mission's `textureFilter`) has its cubes magnified with nearest-neighbor
//!   filtering; minified, they stay linear between and within the mip levels, so the far floor does not shimmer.
//! - **Interiors** (InteriorInstances) are their `.dif`'s drawn surfaces (the legacy importer's triangles) placed by their
//!   records, drawn as the cubes are: the grid's tiles from the surfaces' texture planes, tinted a warm sand for the
//!   `arena_floor` material and a cool slate lighter the higher for the rest, magnified as the map's filter says. Their faces
//!   are surfaces a bolt's hole may lie on too. An interior's objects are kept until its record moves it.
//! - **The eye**: the client's own figure's eye (the engine places it at the drawn figure's `eye` node), its field of view
//!   narrowing from [`FOV`] to [`AIM_FOV`] as the sights come up ([`ArenaScene::turn_scale`] slows the mouse's turn by as
//!   much), else a view over the arena.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use torque::app::ghosts::{
    BoltPath, DecalLimits, FrameInput, GhostOptions, GhostScene, Kick, ShotFlare, Sights,
};
use torque::app::view::{SceneShape, WorldView};
use torque::client::conn::tge::TgeWorld;
use torque::client::predict::ghost_players::GhostPlayers;
use torque::ghost_world::tge::TgeClientSet;
use torque::ghost_world::world::{ClientWorld, Pose};
use torque::ghosts::{TgeClass, TgeRecord};
use torque::mapcore::Importer as _;
use torque::mapcore::geometry::Surfaces;
use torque_example_common::{KitAssets, kit_path};
use torque_render_scene::decals::Triangle;
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::math::Mat;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::source::Eye;
use torque_render_scene::{Assets as _, Draw, Filter, Vertex};

pub use torque::app::ghosts::{aim_ease, bolt_trail, fire_emitter, hit_burst};

/// The field of view (degrees, horizontal).
pub const FOV: u8 = 90;
/// Aiming down the sights: how long the pistol takes to come up to the sights (seconds), the field of view there (degrees,
/// horizontal), and how far ahead of the eye the rear sight is held. The pistol is placed there from its `sightRear` and
/// `sightFront` nodes: turned so the line through them runs along the eye's, and moved so that line is the eye's (both
/// sights on the crosshair). A pistol without them is held at [`AIM_OFFSET`] (the image's place in the eye's frame).
pub const AIM_SECONDS: f32 = 0.18;
pub const AIM_FOV: f32 = 60.0;
pub const AIM_REAR_DISTANCE: f32 = 0.36;
pub const AIM_OFFSET: [f32; 3] = [0.0, 0.42, -0.057];

/// How fast a bolt turns about its flight (radians a second).
pub const BOLT_SPIN: f32 = std::f32::consts::TAU;
/// How far a bolt flies before it is drawn on its flight (units): until then it is drawn curving there from the shooter's muzzle
/// (the server fires it along the eye's line, so it goes where the crosshair is).
pub const BOLT_BLEND: f32 = 12.0;
/// A bolt leaving the muzzle grows from this share of its size to its whole size over its first [`BOLT_GROW`] units, so it
/// comes out of the barrel instead of filling the view beside it.
pub const BOLT_START_SIZE: f32 = 0.3;
pub const BOLT_GROW: f32 = 1.5;
/// The first-person pistol's kick: how long it takes to reach its height and how fast it settles after (seconds), and how far it
/// goes at its height: back (m) and up (radians). In the hand the figure's arm kicks instead (the pistol's fire state's
/// `stateRecoil` plays the figure's `light_recoil`).
pub const RECOIL_RISE: f32 = 0.04;
pub const RECOIL_SETTLE: f32 = 0.08;
pub const RECOIL_BACK: f32 = 0.06;
pub const RECOIL_UP: f32 = 0.14;

/// The recoil as the engine kicks the first-person pistol.
pub const KICK: Kick = Kick {
    rise: RECOIL_RISE,
    settle: RECOIL_SETTLE,
    back: RECOIL_BACK,
    up: RECOIL_UP,
    up_hand: 0.0,
};

/// How far into its kick the recoil is `t` seconds after a shot: 0 at rest, 1 at its height (a quick rise, then a settle).
pub fn recoil(t: f32) -> f32 {
    KICK.at(t)
}

/// A shot's flare at the muzzle: how long it shows (seconds) and the star seen head on (its width, units). It shrinks and fades
/// over the time.
pub const FLARE_SECONDS: f32 = 0.07;
pub const FLARE_STAR: f32 = 0.26;
/// The flare's texture.
pub const FLARE_STAR_TEXTURE: &str = "kit/flare_star";

/// The tint of a cube whose top is at `top`: the floor a warm sand, the walls and blocks a cool slate, lighter the higher.
pub fn cube_tint(top: f32) -> [u8; 3] {
    if top <= 0.05 {
        return [214, 198, 170];
    }
    let t = (top / 5.0).clamp(0.0, 1.0);
    let low = [132.0, 146.0, 168.0];
    let high = [196.0, 206.0, 224.0];
    [0, 1, 2].map(|i| (low[i] + (high[i] - low[i]) * t).round() as u8)
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

/// How a map's textures are drawn: the map's `textureFilter` (`$Arena::TextureFilter` once the server named it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextureFilter {
    /// Linear filtering, the engine's default.
    #[default]
    Smooth,
    /// Sharp texels: nearest-neighbor filtering where a texel covers more than a pixel.
    Nearest,
}

impl TextureFilter {
    /// The filter a map names (`nearest`, any case); anything else is smooth.
    pub fn named(name: &str) -> TextureFilter {
        if name.trim().eq_ignore_ascii_case("nearest") {
            TextureFilter::Nearest
        } else {
            TextureFilter::Smooth
        }
    }
}

fn retexture(d: &mut Draw, tint: [u8; 3], filter: TextureFilter) {
    for v in &mut d.vertices {
        v.uv = world_uv(v);
        v.color = [tint[0], tint[1], tint[2], 255];
    }
    if let Some((_, sampler)) = d.texture.as_mut() {
        sampler.repeat = true;
        if filter == TextureFilter::Nearest {
            sampler.mag = Filter::Nearest;
        }
    }
}

/// A cube's object, kept with the pose it was made at.
struct KeptCube {
    pose: Pose,
    object: DynamicObject,
}

/// An interior's objects (one a material), kept with the pose they were made at, and its faces in the world (counter-clockwise
/// seen from the front, as the decals take them).
struct KeptInterior {
    pose: Pose,
    objects: Vec<DynamicObject>,
    faces: Arc<Vec<Triangle>>,
}

/// The id of an interior's object of material `k` among the frame's objects.
fn interior_id(index: u32, k: usize) -> u32 {
    0x5000_0000 | (index << 6) | (k as u32 & 0x3f)
}

/// Which ghost is the client's own figure, where the client predicts it and where its view is turned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnLook {
    pub ghost: u32,
    pub yaw: f32,
    /// Positive looks down.
    pub pitch: f32,
    /// The feet as the client predicts them (none before the prediction starts: the record's).
    pub position: Option<[f32; 3]>,
}

/// The most bolt holes kept, how long each stays and how long of that it fades out (seconds).
pub const MAX_DECALS: usize = 64;
pub const DECAL_SECONDS: f32 = 25.0;
pub const DECAL_FADE: f32 = 4.0;

/// The id of a cube's object among the frame's objects (the engine's objects are their ghosts' indexes).
fn cube_id(index: u32) -> u32 {
    0x4000_0000 | index
}

/// The engine's options for the arena's look.
pub fn options() -> GhostOptions {
    GhostOptions {
        kick: Some(KICK),
        sights: Sights {
            seconds: AIM_SECONDS,
            rear: AIM_REAR_DISTANCE,
            fallback: AIM_OFFSET,
        },
        bolt: BoltPath {
            blend: BOLT_BLEND,
            start_size: BOLT_START_SIZE,
            grow: BOLT_GROW,
            spin: BOLT_SPIN,
        },
        ribbon_trails: true,
        flare: Some(ShotFlare {
            texture: FLARE_STAR_TEXTURE.to_string(),
            seconds: FLARE_SECONDS,
            size: FLARE_STAR,
        }),
        decals: DecalLimits {
            max: MAX_DECALS,
            seconds: DECAL_SECONDS,
            fade: DECAL_FADE,
            multiply: true,
        },
        // The server's figures have hitboxes: drawn as posed for hits.
        synced_poses: true,
        ..GhostOptions::default()
    }
}

/// The arena's scene: the engine's ghosts and the arena's cubes.
pub struct ArenaScene {
    ghosts: GhostScene,
    shapes: HashMap<String, Option<SceneShape>>,
    cubes: BTreeMap<u32, KeptCube>,
    interiors: BTreeMap<u32, KeptInterior>,
    /// The interiors' files read (by path; `None` for one that did not read).
    interior_files: HashMap<String, Option<Arc<Surfaces>>>,
    /// How the cubes and interiors are drawn.
    filter: TextureFilter,
}

/// The engine's scene: the figures, the bolts and their effects as the last frame left them.
impl std::ops::Deref for ArenaScene {
    type Target = GhostScene;

    fn deref(&self) -> &GhostScene {
        &self.ghosts
    }
}

impl std::ops::DerefMut for ArenaScene {
    fn deref_mut(&mut self) -> &mut GhostScene {
        &mut self.ghosts
    }
}

impl Default for ArenaScene {
    fn default() -> ArenaScene {
        ArenaScene::new()
    }
}

impl ArenaScene {
    /// A scene with nothing in it yet.
    pub fn new() -> ArenaScene {
        ArenaScene {
            ghosts: GhostScene::new(options()),
            shapes: HashMap::new(),
            cubes: BTreeMap::new(),
            interiors: BTreeMap::new(),
            interior_files: HashMap::new(),
            filter: TextureFilter::Smooth,
        }
    }

    /// Forgets every ghost's object (a new connection's ghosts are new).
    pub fn clear(&mut self) {
        self.ghosts.clear();
        self.cubes.clear();
        self.interiors.clear();
    }

    /// Draws the cubes with `filter` from now on (the ones made with another are made again).
    pub fn set_texture_filter(&mut self, filter: TextureFilter) {
        if filter != self.filter {
            self.filter = filter;
            self.cubes.clear();
            self.interiors.clear();
        }
    }

    /// The frame's motion by `dt` seconds, with nothing drawn: the figures, the pistols, the bolts and their effects
    /// ([`GhostScene::update`]), the eye at the own figure's with the sights' field of view.
    pub fn update(
        &mut self,
        assets: &KitAssets,
        world: &mut TgeWorld,
        ghosts: &GhostPlayers,
        dt: f32,
    ) {
        let interiors: Vec<Arc<Vec<Triangle>>> = self
            .interiors
            .values()
            .map(|k| Arc::clone(&k.faces))
            .collect();
        let near = move |w: &ClientWorld<TgeClientSet>, at: [f32; 3], reach: f32| {
            let mut out = cube_triangles_near(w, at, reach);
            for faces in &interiors {
                out.extend(faces.iter().filter(|t| {
                    (0..3).all(|c| {
                        let lo = t[0][c].min(t[1][c]).min(t[2][c]);
                        let hi = t[0][c].max(t[1][c]).max(t[2][c]);
                        at[c] >= lo - reach && at[c] <= hi + reach
                    })
                }));
            }
            out
        };
        let input = FrameInput {
            ghosts: Some(ghosts),
            fov: Some(self.fov().round() as u8),
            dt,
            surfaces: &near,
        };
        self.ghosts.update(assets, world, &input);
    }

    /// The frame's objects: the engine's ghosts as [`ArenaScene::update`] left them and `world`'s cubes; their effects into
    /// `view`.
    pub fn objects(
        &mut self,
        view: &mut WorldView,
        assets: &KitAssets,
        world: &TgeWorld,
    ) -> Vec<DynamicObject> {
        let mut out = self.ghosts.objects(view, assets, world);
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
            if let Some(cube) = make_cube(view, assets, &shape, pose, index, self.filter) {
                self.cubes.insert(index, cube);
            }
        }
        self.cubes.retain(|i, _| seen.contains(i));
        out.extend(self.cubes.values().map(|c| c.object.clone()));
        let mut seen = Vec::new();
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
            let Some(surfaces) = self
                .interior_files
                .entry(path.clone())
                .or_insert_with(|| {
                    let bytes = assets.read(&path)?;
                    torque::maps::legacy::IMPORTER
                        .surfaces(&bytes)
                        .ok()
                        .map(Arc::new)
                })
                .clone()
            else {
                continue;
            };
            let Some(cube) = self.shape(assets, kit_path("cube.dts").as_bytes()) else {
                continue;
            };
            if let Some(k) = make_interior(view, assets, &cube, &surfaces, pose, index, self.filter)
            {
                self.interiors.insert(index, k);
            }
        }
        self.interiors.retain(|i, _| seen.contains(i));
        out.extend(
            self.interiors
                .values()
                .flat_map(|k| k.objects.iter().cloned()),
        );
        let eye = self.eye();
        self.ghosts.effects(view, assets, &eye);
        out
    }

    fn shape(&mut self, assets: &KitAssets, path: &[u8]) -> Option<SceneShape> {
        let path = String::from_utf8_lossy(path).replace('\\', "/");
        self.shapes
            .entry(path.clone())
            .or_insert_with(|| SceneShape::load(assets, &path))
            .clone()
    }

    /// The field of view now (degrees, horizontal): [`FOV`], narrowing to [`AIM_FOV`] as the sights come up.
    pub fn fov(&self) -> f32 {
        f32::from(FOV) + (AIM_FOV - f32::from(FOV)) * self.aim()
    }

    /// How much slower the mouse turns the view now: the narrower view's share of the full one (`tan` of the half angles).
    pub fn turn_scale(&self) -> f32 {
        let half = |deg: f32| (deg.to_radians() * 0.5).tan();
        half(self.fov()) / half(f32::from(FOV))
    }

    /// The eye: at the own figure's eye, else over the arena.
    pub fn eye(&self) -> Eye {
        match self.ghosts.own() {
            Some(o) => Eye {
                position: o.eye,
                pitch: o.pitch,
                yaw: o.yaw,
                fov: Some(self.fov().round() as u8),
            },
            None => self.overview_eye(),
        }
    }

    /// An eye looking at the whole arena from above and to its south.
    pub fn overview_eye(&self) -> Eye {
        let mut eye = look_at([0.0, -34.0, 22.0], [0.0, 0.0, 0.0]);
        eye.fov = Some(FOV);
        eye
    }
}

/// The faces (as triangles) of the map's cubes (its StaticShapes, each the kit's unit cube scaled and turned as its record
/// says) whose bounds come within `reach` of `at`: the surface a decal there may lie on.
fn cube_triangles_near(
    world: &ClientWorld<TgeClientSet>,
    at: [f32; 3],
    reach: f32,
) -> Vec<Triangle> {
    let mut out = Vec::new();
    for ghost in world.ghosts.values() {
        let (Some(TgeClass::StaticShape), Some(pose)) = (ghost.class, ghost.pose()) else {
            continue;
        };
        let Some((m, scale)) = pose_matrix(&pose, None) else {
            continue;
        };
        let half = scale.map(|s| s.abs() * 0.5);
        let radius = (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt();
        let center = m.column(3);
        let far = (0..3)
            .map(|k| (center[k] - at[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        if far > radius + reach {
            continue;
        }
        let corner = |i: usize| {
            let local = [0, 1, 2].map(|k| if i >> k & 1 != 0 { 0.5 } else { -0.5 } * scale[k]);
            m.mul_p(local)
        };
        // Each face's corners counter-clockwise seen from outside (for a positive scale).
        for f in [
            [0, 4, 6, 2],
            [1, 3, 7, 5],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 2, 3, 1],
            [4, 5, 7, 6],
        ] {
            out.push([corner(f[0]), corner(f[1]), corner(f[2])]);
            out.push([corner(f[0]), corner(f[2]), corner(f[3])]);
        }
    }
    out
}

/// The material of an interior's floor (the kit's arena's): drawn in the floor's sand.
pub const FLOOR_MATERIAL: &str = "arena_floor";

/// The tint of an interior's triangle of `material` whose highest point is at `top`: the floor's sand, else as a cube's.
fn interior_tint(material: &str, top: f32) -> [u8; 3] {
    if material.eq_ignore_ascii_case(FLOOR_MATERIAL) {
        cube_tint(0.0)
    } else {
        cube_tint(top.max(0.06))
    }
}

/// An interior's drawn surfaces placed by `pose`: one object a material, each made from the kit's cube (its draw, its grid
/// texture) with the cube's vertices replaced by the interior's triangles in the world, clockwise seen from the front.
fn make_interior(
    view: &mut WorldView,
    assets: &KitAssets,
    cube: &SceneShape,
    surfaces: &Surfaces,
    pose: Pose,
    index: u32,
    filter: TextureFilter,
) -> Option<KeptInterior> {
    let (m, scale) = torque_example_common::prediction::placed(&pose)?;
    let instance = cube.instance()?;
    let mut objects = Vec::new();
    let mut faces = Vec::new();
    for (k, (material, corners)) in surfaces.by_material.iter().enumerate() {
        let mut vertices = Vec::with_capacity(corners.len());
        for t in corners.chunks_exact(3) {
            let mut v = [0, 1, 2].map(|c| {
                let (p, n, uv) = t[c];
                let normal = m.mul_v([n[0] / scale[0], n[1] / scale[1], n[2] / scale[2]]);
                let len =
                    (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
                Vertex {
                    position: m.mul_p([p[0] * scale[0], p[1] * scale[1], p[2] * scale[2]]),
                    normal: normal.map(|x| x / len.max(1e-12)),
                    color: [255; 4],
                    uv,
                }
            });
            let e1 = [0, 1, 2].map(|c| v[1].position[c] - v[0].position[c]);
            let e2 = [0, 1, 2].map(|c| v[2].position[c] - v[0].position[c]);
            let face = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            if (0..3).map(|c| face[c] * v[0].normal[c]).sum::<f32>() > 0.0 {
                v.swap(1, 2);
            }
            let top = v.iter().map(|x| x.position[2]).fold(f32::MIN, f32::max);
            let tint = interior_tint(material, top);
            for x in &mut v {
                x.color = [tint[0], tint[1], tint[2], 255];
            }
            faces.push([v[0].position, v[2].position, v[1].position]);
            vertices.extend(v);
        }
        let mut first = Some(vertices);
        objects.push(view.shape_object_with(
            assets,
            cube,
            &instance,
            &Mat::IDENTITY,
            [1.0; 3],
            interior_id(index, k),
            &mut |d| {
                d.vertices = first.take().unwrap_or_default();
                if let Some((_, sampler)) = d.texture.as_mut() {
                    sampler.repeat = true;
                    if filter == TextureFilter::Nearest {
                        sampler.mag = Filter::Nearest;
                    }
                }
            },
        ));
    }
    Some(KeptInterior {
        pose,
        objects,
        faces: Arc::new(faces),
    })
}

fn make_cube(
    view: &mut WorldView,
    assets: &KitAssets,
    shape: &SceneShape,
    pose: Pose,
    index: u32,
    filter: TextureFilter,
) -> Option<KeptCube> {
    let (m, scale) = pose_matrix(&pose, None)?;
    let instance = shape.instance()?;
    let center = m.column(3);
    let top = center[2] + scale[2].abs() * 0.5;
    let tint = cube_tint(top);
    let object = view.shape_object_with(
        assets,
        shape,
        &instance,
        &m,
        scale,
        cube_id(index),
        &mut |d| retexture(d, tint, filter),
    );
    Some(KeptCube { pose, object })
}

#[cfg(test)]
mod tests {
    use torque_render_scene::{Blend, Cull, Sampler, TexEnv};

    use super::*;

    fn textured() -> Draw {
        let linear = Sampler {
            repeat: false,
            mag: Filter::Linear,
            min: Filter::Linear,
            mip: Filter::Linear,
            mipmaps: true,
        };
        Draw {
            label: String::new(),
            vertices: Vec::new(),
            lighting: true,
            tex_env: TexEnv::Modulate,
            texture: Some((0, linear)),
            blend: Blend::Opaque,
            cull: Cull::None,
            depth_test: true,
            depth_write: true,
            fog: true,
            lights: Vec::new(),
            vertex_key: None,
        }
    }

    #[test]
    fn the_recoil_kicks_quickly_and_settles() {
        assert_eq!(recoil(0.0), 0.0);
        assert_eq!(recoil(RECOIL_RISE), 1.0);
        assert!(recoil(RECOIL_RISE * 0.5) > 0.4);
        let later = [0.08, 0.15, 0.3].map(recoil);
        assert!(
            later[0] > later[1] && later[1] > later[2] && later[2] < 0.05,
            "{later:?}"
        );
        assert_eq!(recoil(2.0), 0.0);
        assert_eq!(recoil(f32::MAX), 0.0);
    }

    #[test]
    fn a_map_names_its_texture_filter() {
        assert_eq!(TextureFilter::named("nearest"), TextureFilter::Nearest);
        assert_eq!(TextureFilter::named(" Nearest "), TextureFilter::Nearest);
        assert_eq!(TextureFilter::named(""), TextureFilter::Smooth);
        assert_eq!(TextureFilter::named("linear"), TextureFilter::Smooth);
    }

    #[test]
    fn sharp_cubes_are_magnified_nearest_and_minified_linear() {
        let mut d = textured();
        retexture(&mut d, [255; 3], TextureFilter::Nearest);
        let (_, s) = d.texture.unwrap();
        assert!(s.repeat && s.mipmaps);
        assert_eq!(
            (s.mag, s.min, s.mip),
            (Filter::Nearest, Filter::Linear, Filter::Linear)
        );

        let mut d = textured();
        retexture(&mut d, [255; 3], TextureFilter::Smooth);
        let (_, s) = d.texture.unwrap();
        assert_eq!(
            (s.mag, s.min, s.mip),
            (Filter::Linear, Filter::Linear, Filter::Linear)
        );
    }
}
