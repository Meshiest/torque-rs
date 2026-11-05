//! The client's ghosts as the world view's objects, and the eye.
//!
//! The figures, the pistols they hold (in the hand, and the client's own before its eye), the bolts with their trails and the
//! holes they leave are the engine's ([`GhostScene`]): the arena sets how they look ([`options`]: ribbon trails, multiplied bolt
//! holes; [`style`]: the recoil's kick, the sights, a bolt's curve out of the muzzle, its growth and spin, the shot's flare).
//! The arena draws its own:
//!
//! - **Cubes** (StaticShapes) are scaled as their records say, their texture coordinates made from where each vertex is (one
//!   grid tile a unit) and tinted by height. A cube's object is kept until its record moves it. A map that asks for sharp
//!   texels ([`TextureFilter::Nearest`], the mission's `textureFilter`) has its cubes magnified with nearest-neighbor
//!   filtering; minified, they stay linear between and within the mip levels, so the far floor does not shimmer.
//! - **Interiors** (InteriorInstances) are the engine's ([`GhostScene::objects`]); the arena's style tints their walls lighter
//!   the higher as the cubes are and magnifies them as the map's filter says ([`ArenaStyle::nearest`]).
//! - **The eye**: the client's own figure's eye (the engine places it at the drawn figure's `eye` node), its field of view
//!   narrowing from [`FOV`] to [`AIM_FOV`] as the sights come up ([`ArenaScene::turn_scale`] slows the mouse's turn by as
//!   much), else a view over the arena.

use std::collections::{BTreeMap, HashMap};

use torque::app::ghosts::{DecalLimits, FrameInput, GhostOptions, GhostScene};
use torque::app::view::{SceneShape, WorldView};
use torque::client::conn::tge::TgeWorld;
use torque::client::predict::ghost_players::GhostPlayers;
use torque::ghost_world::tge::TgeClientSet;
use torque::ghost_world::world::{ClientWorld, Pose};
use torque::ghosts::TgeClass;
use torque_example_common::KitAssets;
use torque_render_scene::decals::Triangle;
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::mounts::pose_matrix;
use torque_render_scene::source::Eye;
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Draw, Filter, Vertex};

use crate::style::{ArenaStyle, BoltPath, Kick, ShotFlare, Sights};

pub use crate::style::aim_ease;
pub use torque::app::ghosts::{bolt_trail, fire_emitter, hit_burst};

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

/// The arena's own touches on the engine's ghosts: the recoil, the sights, the bolts' curve and the shots' flares.
pub fn style() -> ArenaStyle {
    ArenaStyle::new(
        Some(KICK),
        Sights {
            seconds: AIM_SECONDS,
            rear: AIM_REAR_DISTANCE,
            fallback: AIM_OFFSET,
        },
        BoltPath {
            blend: BOLT_BLEND,
            start_size: BOLT_START_SIZE,
            grow: BOLT_GROW,
            spin: BOLT_SPIN,
        },
        Some(ShotFlare {
            texture: FLARE_STAR_TEXTURE.to_string(),
            seconds: FLARE_SECONDS,
            size: FLARE_STAR,
        }),
    )
}

/// The engine's options for the arena's look.
pub fn options() -> GhostOptions {
    GhostOptions {
        ribbon_trails: true,
        decals: DecalLimits {
            max: MAX_DECALS,
            seconds: DECAL_SECONDS,
            fade: DECAL_FADE,
            multiply: true,
        },
        // The server's figures have hitboxes: drawn as posed for hits.
        synced_poses: true,
        // The drawn eye, not the steady one, until a shot carries the aim the player saw (the steady eye would part the
        // crosshair from the server's shot by the head's bob).
        steady_eye: false,
        ..GhostOptions::default()
    }
}

/// The arena's scene: the engine's ghosts and the arena's cubes.
pub struct ArenaScene {
    ghosts: GhostScene<ArenaStyle>,
    shapes: HashMap<String, Option<SceneShape>>,
    cubes: BTreeMap<u32, KeptCube>,
    /// How the cubes and interiors are drawn.
    filter: TextureFilter,
    /// Whether the style's own textures (the shot's flare) were loaded.
    style_loaded: bool,
}

/// The engine's scene: the figures, the bolts and their effects as the last frame left them.
impl std::ops::Deref for ArenaScene {
    type Target = GhostScene<ArenaStyle>;

    fn deref(&self) -> &GhostScene<ArenaStyle> {
        &self.ghosts
    }
}

impl std::ops::DerefMut for ArenaScene {
    fn deref_mut(&mut self) -> &mut GhostScene<ArenaStyle> {
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
            ghosts: GhostScene::with_style(options(), style()),
            shapes: HashMap::new(),
            cubes: BTreeMap::new(),
            filter: TextureFilter::Smooth,
            style_loaded: false,
        }
    }

    /// Forgets every ghost's object (a new connection's ghosts are new).
    pub fn clear(&mut self) {
        self.ghosts.clear();
        self.cubes.clear();
    }

    /// `dt` seconds of the sights' blend toward the sights (`aiming`) or the hip; returns how far up they are (eased).
    pub fn update_aim(&mut self, aiming: bool, dt: f32) -> f32 {
        self.ghosts.style.update_aim(aiming, dt)
    }

    /// How far the sights are up (eased, 0 to 1).
    pub fn aim(&self) -> f32 {
        self.ghosts.style.aim()
    }

    /// Where the own figure's pistol's sights (`sightFront`, `sightRear`) were drawn in the last frame.
    pub fn own_sights(&self) -> Option<[[f32; 3]; 2]> {
        self.ghosts.style.own_sights()
    }

    /// Draws the cubes and the interiors with `filter` from now on (the ones made with another are made again).
    pub fn set_texture_filter(&mut self, filter: TextureFilter) {
        if filter != self.filter {
            self.filter = filter;
            self.cubes.clear();
            self.ghosts.style.nearest = filter == TextureFilter::Nearest;
            self.ghosts.restyle_interiors();
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
        let input = FrameInput {
            ghosts: Some(ghosts),
            fov: Some(self.fov().round() as u8),
            dt,
            surfaces: &cube_triangles_near,
        };
        // The flare's texture loaded with the map, not in the first shot's frame (the datablocks' are the engine's to load).
        if !std::mem::replace(&mut self.style_loaded, true)
            && let Some(flare) = &self.ghosts.style.flare
        {
            let _ = TextureSet::new(assets).get_modulated(&flare.texture);
        }
        self.ghosts.update(assets, world, &input);
    }

    /// The frame's objects: the engine's ghosts and interiors as [`ArenaScene::update`] left them and `world`'s cubes; their
    /// effects into
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

/// The boxes (min, max) of the map's cubes (its StaticShapes, each the kit's unit cube scaled and turned as its record says): the
/// camera behind the figure stays in front of them.
pub fn cube_boxes(world: &ClientWorld<TgeClientSet>) -> Vec<([f32; 3], [f32; 3])> {
    let mut out = Vec::new();
    for ghost in world.ghosts.values() {
        let (Some(TgeClass::StaticShape), Some(pose)) = (ghost.class, ghost.pose()) else {
            continue;
        };
        let Some((m, scale)) = pose_matrix(&pose, None) else {
            continue;
        };
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for corner in 0..8 {
            let local = [0, 1, 2].map(|k| if corner >> k & 1 != 0 { 0.5 } else { -0.5 } * scale[k]);
            let p = m.mul_p(local);
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        out.push((lo, hi));
    }
    out
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
