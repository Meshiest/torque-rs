//! The client's ghosts as the world view's objects: each StaticShape and Player ghost whose datablock names a shape of the kit,
//! placed where its records put it.
//!
//! - **Cubes** (StaticShapes) are scaled as their records say. Their texture coordinates are made from where each vertex is in
//!   the world (one grid tile a unit, the face's two axes), so a long wall shows square tiles instead of one stretched grid, and
//!   each is tinted by its height: the ground grey-blue, higher cubes warmer. A cube's object is made once and kept until its
//!   record moves it.
//! - **Figures** (Players) are the engine's ([`GhostScene`]): the client's own where it predicts it (the cubes are its
//!   collision world, [`torque_example_common::prediction`]), each playing the actions its shape has for the way it moves
//!   (`root`, `run`, `back`, `side`, the jump and the fall) as TGE's client picks them.

use std::collections::{BTreeMap, HashMap};

use torque::app::ghosts::{FrameInput, GhostOptions, GhostScene, OwnView};
use torque::app::view::{SceneShape, WorldView};
use torque::client::conn::tge::TgeWorld;
use torque::client::predict::ghost_players::GhostPlayers;
use torque::ghost_world::world::Pose;
use torque::ghosts::TgeClass;
use torque_example_common::KitAssets;
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

/// The objects of the client's ghosts, frame after frame.
pub struct WorldScene {
    ghosts: GhostScene,
    shapes: HashMap<String, Option<SceneShape>>,
    cubes: BTreeMap<u32, KeptCube>,
}

impl Default for WorldScene {
    fn default() -> WorldScene {
        WorldScene {
            ghosts: GhostScene::new(GhostOptions::default()),
            shapes: HashMap::new(),
            cubes: BTreeMap::new(),
        }
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

/// The tint of a cube whose top is at `top`: the ground grey-blue, warmer the higher it reaches.
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
