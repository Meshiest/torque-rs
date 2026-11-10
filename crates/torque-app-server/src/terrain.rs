//! A `TerrainBlock` over a legacy `.ter`: its collision on a game's server. The client's prediction and the drawing of the
//! ground are in `torque-app` (`torque_app::terrain`, which wraps this block).
//!
//! - **The block** ([`Terrain`]): the file's heightfield at TGE's placement (`TerrainBlock::onAdd`: sample (0, 0) at
//!   `-128 * squareSize` in x and y, at height 0), not repeated, its squares cut as the file's grid alternates them, with TGE's
//!   type bits ([`TERRAIN_TYPE_MASK`]).
//! - **On the server** ([`install`]): a `TerrainBlock` added by a mission reads its `terrainFile` from the server's files and
//!   enters the container with its collision ([`torque_game::scene::ext::MapObject`]): Players stand on it, vehicles' bodies hit
//!   it and their wheels' rays find it, `containerRayCast` meets it. A block whose file does not read is left without one.
//!
//! Credit: the placement and the type bits follow `TerrainBlock::onAdd` in TGE 1.4's `terrain/terrData.cc` (Copyright (C)
//! GarageGames.com, Inc.) as Torque3D carries it under the MIT license (`Engine/source/terrain/terrData.cpp`, Copyright (c)
//! 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use std::rc::Rc;
use std::sync::Arc;

use torque_collide::convex::{ObjectView, view_of};
use torque_collide::map::{HeightField, MapShape, TerrainShape};
use torque_collide::math::{Box3, F3, IDENTITY};
use torque_collide::types::{STATIC, STATIC_RENDERED, TERRAIN};
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
/// TGE's `squareSize` when a mission names none.
pub const DEFAULT_SQUARE_SIZE: i32 = 8;

/// A terrain block: its heightfield placed as TGE places it (module docs).
#[derive(Debug, Clone)]
pub struct Terrain {
    shape: Rc<MapShape>,
    /// The file's materials in order: their names and weights ([`ter::SAMPLES`] each).
    layers: Rc<Vec<torque_mapcore::collision::Layer>>,
}

impl Terrain {
    /// The block of the `.ter` file `bytes`, `square_size` units a square.
    pub fn read(bytes: &[u8], square_size: i32) -> Result<Terrain, String> {
        let file = ter::parse(bytes).map_err(|e| format!("the terrain: {e}"))?;
        let heightfield = legacy::heightfield(&file);
        let field = Arc::new(HeightField::new(&heightfield));
        let place: HeightfieldPlacement = legacy::terrain_placement(square_size, false, &[]);
        Ok(Terrain {
            shape: Rc::new(MapShape::Terrain(TerrainShape::new(field, &place))),
            layers: Rc::new(heightfield.layers),
        })
    }

    /// The collision shape as the heightfield's own type (the squares and corners of the grid).
    pub fn block(&self) -> &TerrainShape {
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

    /// The file's material names, in order.
    pub fn materials(&self) -> Vec<&str> {
        self.layers.iter().map(|l| l.material.as_str()).collect()
    }

    /// Material `k`'s weight at sample (x, y) of the grid (wrapping), 0 for a material the file does not have.
    pub fn weight(&self, k: usize, x: i64, y: i64) -> u8 {
        let side = ter::SIZE as i64;
        let i = (x.rem_euclid(side) + y.rem_euclid(side) * side) as usize;
        self.layers
            .get(k)
            .and_then(|l| l.weights.get(i))
            .copied()
            .unwrap_or(0)
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
                water: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat version-2 `.ter` of `grass` and `dirt`, the dirt weighing 204 at sample (10, 20) and the grass the rest.
    fn two_materials() -> Vec<u8> {
        let mut out = vec![2u8];
        out.extend(std::iter::repeat_n([0u8, 2], ter::SAMPLES).flatten());
        out.extend(std::iter::repeat_n(0u8, ter::SAMPLES));
        for name in [&b"grass"[..], b"dirt"] {
            out.push(name.len() as u8);
            out.extend(name);
        }
        out.extend([0u8; 6]);
        let dirt: Vec<u8> = (0..ter::SAMPLES)
            .map(|i| if i == 10 + 20 * ter::SIZE { 204 } else { 0 })
            .collect();
        out.extend(dirt.iter().map(|&d| 255 - d));
        out.extend(dirt);
        out
    }

    #[test]
    fn a_block_reads_its_materials_weights_and_ground() {
        let t = Terrain::read(&two_materials(), 8).unwrap();
        assert_eq!(t.materials(), ["grass", "dirt"]);
        assert_eq!(t.weight(1, 10, 20), 204);
        assert_eq!(t.weight(0, 10, 20), 51);
        assert_eq!(t.weight(5, 10, 20), 0);
        assert_eq!(t.height(0.0, 0.0), Some(t.block().corner(128, 128)[2]));
        assert_eq!(t.view().type_mask, TERRAIN_TYPE_MASK);
        assert!(Terrain::read(b"not a terrain", 8).is_err());
    }
}
