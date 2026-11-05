//! A `TerrainBlock` over a legacy `.ter`: its collision on a game's server, the same collision in the client's prediction, and
//! its ground drawn in the world view.
//!
//! - **The block** ([`Terrain`]): the file's heightfield at TGE's placement (`TerrainBlock::onAdd`: sample (0, 0) at
//!   `-128 * squareSize` in x and y, at height 0), not repeated, its squares cut as the file's grid alternates them, with TGE's
//!   type bits ([`TERRAIN_TYPE_MASK`]).
//! - **On the server** ([`install`]): a `TerrainBlock` added by a mission reads its `terrainFile` from the server's files and
//!   enters the container with its collision ([`torque_game::scene::ext::MapObject`]): Players stand on it, vehicles' bodies hit
//!   it and their wheels' rays find it, `containerRayCast` meets it. A block whose file does not read is left without one.
//! - **On the client** ([`TerrainSet`]): a game's static index ([`torque_client::predict::StaticIndex`]) offers the block to
//!   the prediction as a static object (the client is not sent a TerrainBlock: it reads the same file), so the client's own
//!   Player and the vehicles it runs meet the ground the server's do.
//! - **Drawn** ([`Terrain::draws`]): the block in square chunks of [`CHUNK`] squares, smooth normals from the heightfield, one
//!   texture repeated every `tile` units in x and y (the world view's [`crate::WorldView::set_terrain`]).
//!
//! Credit: the placement and the type bits follow `TerrainBlock::onAdd` in TGE 1.4's `terrain/terrData.cc` (Copyright (C)
//! GarageGames.com, Inc.) as Torque3D carries it under the MIT license (`Engine/source/terrain/terrData.cpp`, Copyright (c)
//! 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use std::rc::Rc;
use std::sync::Arc;

use torque_render_scene::{Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

use torque_client::predict::{GameSource, Object, Source, StaticSet, view_of};
use torque_collide::convex::{ConvexSource, ObjectView};
use torque_collide::map::{HeightField, MapShape, TerrainShape};
use torque_collide::math::{Box3, F3, IDENTITY};
use torque_collide::types::{STATIC, STATIC_RENDERED, TERRAIN};
use torque_collide::{Exhausted, ObjectId};
use torque_game::scene::container::Container;
use torque_game::scene::{ext, object as scene};
use torque_mapcore::collision::HeightfieldPlacement;
use torque_maps::{legacy, ter};
use torque_ts_vm::classes::EngineClass;
use torque_ts_vm::host::FileRead;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

/// A terrain's type bits (`TerrainObjectType | StaticObjectType | StaticRenderedObjectType`).
pub const TERRAIN_TYPE_MASK: u32 = TERRAIN | STATIC | STATIC_RENDERED;
/// The squares on a side of one drawn chunk.
pub const CHUNK: usize = 32;
/// TGE's `squareSize` when a mission names none.
pub const DEFAULT_SQUARE_SIZE: i32 = 8;

/// A terrain block: its heightfield placed as TGE places it (module docs).
#[derive(Debug, Clone)]
pub struct Terrain {
    shape: Rc<MapShape>,
}

impl Terrain {
    /// The block of the `.ter` file `bytes`, `square_size` units a square.
    pub fn read(bytes: &[u8], square_size: i32) -> Result<Terrain, String> {
        let file = ter::parse(bytes).map_err(|e| format!("the terrain: {e}"))?;
        let field = Arc::new(HeightField::new(&legacy::heightfield(&file)));
        let place: HeightfieldPlacement = legacy::terrain_placement(square_size, false, &[]);
        Ok(Terrain {
            shape: Rc::new(MapShape::Terrain(TerrainShape::new(field, &place))),
        })
    }

    fn block(&self) -> &TerrainShape {
        match &*self.shape {
            MapShape::Terrain(t) => t,
            MapShape::Interior(_) => unreachable!("a terrain's shape is a terrain"),
        }
    }

    /// The collision shape.
    pub fn shape(&self) -> Rc<MapShape> {
        Rc::clone(&self.shape)
    }

    /// The ground's height at world (x, y), None off the block.
    pub fn height(&self, x: f32, y: f32) -> Option<f32> {
        self.block().height(x, y)
    }

    /// The block's triangles under the square of `reach` round (x, y) of `at` (the ground a decal or a tire track lies on).
    pub fn triangles_near(&self, at: [f32; 3], reach: f32) -> Vec<[F3; 3]> {
        let b = self.block();
        let square = b.square();
        let o = b.corner(0, 0);
        let (reach, at) = (reach.max(0.0), [at[0] - o[0], at[1] - o[1]]);
        if !(at[0].is_finite() && at[1].is_finite() && reach.is_finite()) {
            return Vec::new();
        }
        let cell = |v: f32| (v / square).floor() as i64;
        let (x0, x1) = (cell(at[0] - reach), cell(at[0] + reach));
        let (y0, y1) = (cell(at[1] - reach), cell(at[1] + reach));
        let mut out = Vec::new();
        for y in y0..=y1.min(y0 + 64) {
            for x in x0..=x1.min(x0 + 64) {
                if b.has_square(x, y) {
                    out.push(b.triangle(x, y, 0));
                    out.push(b.triangle(x, y, 1));
                }
            }
        }
        out
    }

    /// The box the block lies in.
    pub fn world_box(&self) -> Box3 {
        self.block().world_box()
    }

    /// The block's container view: placed by the identity, its box the world's, [`TERRAIN_TYPE_MASK`].
    pub fn view(&self) -> ObjectView {
        view_of(IDENTITY, self.world_box(), [1.0; 3], TERRAIN_TYPE_MASK)
    }

    /// The ground's normal at sample (x, y) of the grid (central differences, the grid clamped at the block's edges).
    fn normal(&self, x: i64, y: i64) -> [f32; 3] {
        let b = self.block();
        let side = ter::SIZE as i64;
        let h = |x: i64, y: i64| b.corner(x.clamp(0, side), y.clamp(0, side))[2];
        let dx = h(x + 1, y) - h(x - 1, y);
        let dy = h(x, y + 1) - h(x, y - 1);
        let n = [-dx, -dy, 2.0 * b.square()];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
        n.map(|v| v / l)
    }

    /// The block drawn: one draw a chunk of [`CHUNK`] x [`CHUNK`] squares, lit, the scene's texture `texture` (if any) repeated
    /// every `tile` units in x and y, each square cut as its collision is.
    pub fn draws(&self, texture: Option<usize>, tile: f32) -> Vec<Draw> {
        let b = self.block();
        let sampler = Sampler {
            repeat: true,
            mag: Filter::Linear,
            min: Filter::Linear,
            mip: Filter::Linear,
            mipmaps: true,
        };
        let chunks = ter::SIZE / CHUNK;
        let mut out = Vec::with_capacity(chunks * chunks);
        for cy in 0..chunks {
            for cx in 0..chunks {
                let mut vertices = Vec::with_capacity(CHUNK * CHUNK * 6);
                let corner = |x: i64, y: i64| {
                    let p = b.corner(x, y);
                    Vertex {
                        position: p,
                        normal: self.normal(x, y),
                        color: [255; 4],
                        uv: [p[0] / tile, -p[1] / tile],
                    }
                };
                for y in (cy * CHUNK) as i64..((cy + 1) * CHUNK) as i64 {
                    for x in (cx * CHUNK) as i64..((cx + 1) * CHUNK) as i64 {
                        if !b.has_square(x, y) {
                            continue;
                        }
                        let (a, bb, c, d) = (
                            corner(x, y),
                            corner(x + 1, y),
                            corner(x + 1, y + 1),
                            corner(x, y + 1),
                        );
                        // Clockwise seen from above, each square cut along the diagonal its collision is cut along.
                        if ter::split45(x as usize, y as usize) {
                            vertices.extend([a, c, bb, a, d, c]);
                        } else {
                            vertices.extend([a, d, bb, bb, d, c]);
                        }
                    }
                }
                out.push(Draw {
                    label: format!("terrain chunk {cx} {cy}"),
                    vertices,
                    lighting: true,
                    tex_env: if texture.is_some() {
                        TexEnv::Modulate
                    } else {
                        TexEnv::None
                    },
                    texture: texture.map(|i| (i, sampler)),
                    blend: Blend::Opaque,
                    cull: Cull::None,
                    depth_test: true,
                    depth_write: true,
                    fog: true,
                    lights: Vec::new(),
                    vertex_key: None,
                });
            }
        }
        out
    }
}

// ---- the server

/// The `TerrainBlock` class on a server: when a mission adds one, its collision from its `terrainFile` (module docs).
struct TerrainClass {
    container: Rc<Container>,
}

impl EngineClass for TerrainClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let field = |vm: &Vm, name: &[u8]| vm.objects().field(id, name, b"").unwrap_or_default();
        let path = field(vm, b"terrainFile");
        let square = std::str::from_utf8(&field(vm, b"squareSize"))
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .filter(|&s| s > 0)
            .unwrap_or(DEFAULT_SQUARE_SIZE);
        let bytes = match torque_game::charge::charged_host(vm, |h| h.read(&path))? {
            FileRead::Found(bytes) => bytes,
            _ => {
                vm.error(format!(
                    "TerrainBlock: the terrain file {} was not found",
                    String::from_utf8_lossy(&path)
                ));
                return Ok(true);
            }
        };
        torque_game::charge::pay(vm, bytes.len() as u64 + ter::SAMPLES as u64)?;
        let terrain = match Terrain::read(&bytes, square) {
            Ok(t) => t,
            Err(e) => {
                vm.error(format!("TerrainBlock: {e}"));
                return Ok(true);
            }
        };
        let view = terrain.view();
        let w = view.world_box;
        if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
            e.type_mask = view.type_mask;
            if let Some(s) = e.scene.as_mut() {
                s.object_min = view.obj_box.min;
                s.object_max = view.obj_box.max;
                s.world_min = w.min;
                s.world_max = w.max;
                s.world_center = [0, 1, 2].map(|k| (w.min[k] + w.max[k]) * 0.5);
                s.world_radius = view.world_radius;
            }
            e.map = Some(ext::MapObject {
                view,
                shape: Some(terrain.shape()),
            });
        }
        scene::add_to_scene(vm, &self.container, id);
        Ok(true)
    }
}

/// Gives `TerrainBlock` its collision on a server whose game classes put their objects in `container`.
pub fn install(vm: &mut Vm, container: &Rc<Container>) {
    vm.set_class_behaviour(
        "TerrainBlock",
        Rc::new(TerrainClass {
            container: Rc::clone(container),
        }),
    );
}

// ---- the client

/// The client's id of the `k`th static map object (above any ghost's id).
pub fn static_id(k: u32) -> ObjectId {
    ObjectId(0x7f00_0000 | k)
}

/// A terrain as a static object of the client's container: the container's convexes come from its [`MapShape`]; a ray asks it
/// in its space (the world's: it is placed by the identity).
#[derive(Debug)]
struct TerrainSource(Rc<MapShape>);

impl GameSource for TerrainSource {
    fn convex(&self) -> ConvexSource {
        ConvexSource::Map
    }

    fn cast_ray(&self, start: F3, end: F3, _first: bool) -> Option<(f32, F3)> {
        self.0.cast_ray(start, end, &mut |_| true).ok().flatten()
    }
}

/// The map objects a game's static index offers the client's prediction (module docs): every one whose box a query reaches.
#[derive(Debug, Clone, Default)]
pub struct TerrainSet {
    objects: Vec<(ObjectId, Object)>,
}

impl TerrainSet {
    /// The set of `terrains`.
    pub fn new(terrains: &[Terrain]) -> TerrainSet {
        let objects = terrains
            .iter()
            .enumerate()
            .map(|(k, t)| {
                let shape = t.shape();
                (
                    static_id(k as u32),
                    Object {
                        view: t.view(),
                        source: Source::Game(Rc::new(TerrainSource(Rc::clone(&shape)))),
                        shape: None,
                        map: Some(shape),
                    },
                )
            })
            .collect();
        TerrainSet { objects }
    }

    /// Whether the set holds no object.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

impl StaticSet for TerrainSet {
    fn near(&self, lo: F3, hi: F3, work: &mut u64, budget: u64) -> Result<Vec<u32>, Exhausted> {
        let mut out = Vec::new();
        for (i, (_, o)) in self.objects.iter().enumerate() {
            *work = work.saturating_add(1);
            if *work > budget {
                return Err(Exhausted);
            }
            let w = &o.view.world_box;
            if (0..3).all(|k| lo[k] <= w.max[k] && hi[k] >= w.min[k]) {
                out.push(i as u32);
            }
        }
        Ok(out)
    }

    fn get(&self, i: u32) -> &(ObjectId, Object) {
        &self.objects[i as usize]
    }

    fn find(&self, id: ObjectId) -> Option<u32> {
        self.objects
            .iter()
            .position(|(o, _)| *o == id)
            .map(|i| i as u32)
    }

    fn set_view(&mut self, i: u32, view: ObjectView) {
        if let Some((_, o)) = self.objects.get_mut(i as usize) {
            o.view = view;
        }
    }
}
