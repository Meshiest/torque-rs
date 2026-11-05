//! What the client's prediction asks a game of kit cubes: the cubes as the client's collision world, and the view drawn ahead
//! of the last tick by the mouse's waiting turn.
//!
//! - **The cubes** (StaticShapes) and **the interiors** (InteriorInstances) are the static objects of the examples' maps. The
//!   engine's client has no static collision world of its own (a game gives one, [`StaticIndex`]), so without them the own figure
//!   the client predicts falls through the floor between the server's corrections. Each cube's ghost becomes an object of the
//!   client's container as the server's container holds it: the cube shape's collision hulls (read from the kit), its pose and
//!   scale, a StaticShape's type bits. Each interior's ghost becomes its `.dif`'s hulls (read once and kept, at most 256
//!   files, each one's hulls within a work budget) at its pose and scale (none for a hostile one), with an interior's type
//!   bits, read as the server reads them ([`torque::game::scene::interior::read_interior`]).
//! - **A terrain** ([`CubeClient::with_terrain`]): a map's `TerrainBlock` read from the same file the server reads, offered as
//!   the index's static object ([`TerrainSet`]), so the own figure stands on it and the vehicles the client runs drive on it.
//! - **The figure's poses** ([`crate::figure_poses`]): crouched and prone, as the servers' datablocks give them.
//! - **Smooth aim** ([`PredictGame::smooth_aim`]): the view turns with the mouse within the frame, ahead of the last tick by
//!   what the next move will make of the waiting turn, instead of a tick or two behind it. The moves are unchanged.
//!
//! Credit: a cube's collision object follows `ShapeBase::buildConvex` and the StaticShape type bits in Torque3D's MIT-licensed
//! `T3D/shapeBase.cpp` and `T3D/staticShape.cpp` (each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015
//! Faust Logic, Inc.), an interior's its Torque3D 2.0 `interior/interiorInstance.cpp` (Copyright (c) 2012 GarageGames,
//! LLC). The code here is
//! the examples' own.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use torque::app::terrain::{Terrain, TerrainSet};
use torque::client::conn::tge::TgeWorld;
use torque::client::game::ClientGame;
use torque::client::predict::drive::ShapeCache;
use torque::client::predict::game::{PredictGame, Prediction};
use torque::client::predict::{FileReader, Object, Source, StaticIndex, StaticSet, view_of};
use torque::collide::convex::ConvexSource;
use torque::collide::map::{InteriorShape, MapShape};
use torque::collide::math::F3;
use torque::game::scene::interior::{INTERIOR_TYPE_MASK, KeptInterior, read_interior};
use torque::game::sim_classes::{PlayerKind, TorquePlayerKind};
use torque::ghost_world::Ghost;
use torque::ghost_world::tge::TgeClientSet;
use torque::ghosts::{TgeClass, TgeRecord};
use torque::script::vm::host::MemFiles;
use torque::script::vm::vm::Vm;
use torque_render_scene::mounts::pose_matrix;

/// A StaticShape's type bits (`StaticShapeObjectType`, `ShapeBaseObjectType`, `GameBaseObjectType`, `StaticObjectType`), as the
/// server's container has them.
pub const STATIC_SHAPE_TYPE_MASK: u32 = torque::collide::types::STATIC
    | torque::collide::types::GAME_BASE
    | torque::collide::types::SHAPE_BASE
    | torque::collide::types::STATIC_SHAPE;

/// The work a cube's shape may cost to read the first time.
const SHAPE_BUDGET: u64 = 50_000_000;
/// The work an interior's hulls may cost to make the first time, the most interior files kept (each a name a server sent), and
/// the longest name taken.
const INTERIOR_BUDGET: u64 = 50_000_000;
const MAX_INTERIORS: usize = 256;
const MAX_INTERIOR_PATH: usize = 1_024;

/// A game's answers to the client's engine half: Torque's, with the cubes' prediction.
pub struct CubeClient {
    prediction: Rc<Prediction<TgeWorld>>,
    files: MemFiles,
}

impl std::fmt::Debug for CubeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CubeClient")
    }
}

impl CubeClient {
    /// The answers over the game's files `files` (the kit's shapes the cubes collide with).
    pub fn new(files: MemFiles) -> CubeClient {
        CubeClient::with_terrain(files, None, None)
    }

    /// The answers over `files`, a map's terrain (its block as the server reads it from the same file) and the game's Player
    /// class (the one its server runs; none: TGE's).
    pub fn with_terrain(
        files: MemFiles,
        terrain: Option<&Terrain>,
        player: Option<Rc<dyn PlayerKind>>,
    ) -> CubeClient {
        let terrain = terrain.map_or_else(TerrainSet::default, |t| {
            TerrainSet::new(std::slice::from_ref(t))
        });
        CubeClient {
            prediction: Rc::new(Prediction(Rc::new(CubePrediction {
                files: files.clone(),
                terrain,
                player,
            }))),
            files,
        }
    }
}

impl ClientGame for CubeClient {
    fn prediction(&self) -> Option<Rc<dyn Any>> {
        Some(Rc::clone(&self.prediction) as Rc<dyn Any>)
    }

    /// The game's files: a vehicle's shape for its wheel count and the drive the client predicts.
    fn file_reader(&self) -> Option<FileReader> {
        let files = self.files.clone();
        Some(Rc::new(move |path: &[u8]| {
            files.get(&String::from_utf8_lossy(path).replace('\\', "/"))
        }))
    }
}

/// The prediction over the cubes, with smooth aim.
#[derive(Debug)]
pub struct CubePrediction {
    files: MemFiles,
    terrain: TerrainSet,
    player: Option<Rc<dyn PlayerKind>>,
}

impl PredictGame<TgeWorld> for CubePrediction {
    fn static_index(&self) -> Option<Box<dyn StaticIndex<TgeWorld>>> {
        let files = self.files.clone();
        let reader: FileReader = Rc::new(move |path: &[u8]| {
            files.get(&String::from_utf8_lossy(path).replace('\\', "/"))
        });
        Some(Box::new(CubeStatics {
            reader,
            shapes: ShapeCache::default(),
            interiors: Rc::default(),
            terrain: self.terrain.clone(),
        }))
    }

    fn smooth_aim(&self, vm: &Vm) -> bool {
        let _ = vm;
        true
    }

    /// TGE's products, with the figure's poses: the servers give them in fields a datablock record does not carry
    /// ([`crate::figure_poses`]).
    fn player_preload(&self, d: &mut torque::sim::player::PlayerData) {
        d.preload();
        d.poses = crate::figure_poses();
    }

    /// The game's Player class when it has its own, else TGE's.
    fn player_kind(&self, world: &TgeWorld, db: u32) -> Rc<dyn PlayerKind> {
        let _ = (world, db);
        self.player
            .clone()
            .unwrap_or_else(|| Rc::new(TorquePlayerKind))
    }
}

pub use torque::app::interiors::placed;

/// The cubes and interiors as the client's container holds them.
#[derive(Clone)]
pub struct CubeStatics {
    reader: FileReader,
    shapes: ShapeCache,
    /// The interiors' files read (by path; `None` for one that did not read).
    interiors: Rc<RefCell<HashMap<Vec<u8>, Option<KeptInterior>>>>,
    terrain: TerrainSet,
}

impl CubeStatics {
    /// The interior file `path` read (kept, read or refused once), at most [`MAX_INTERIORS`] kept: its hulls' cost charged
    /// against [`INTERIOR_BUDGET`].
    fn interior(&self, path: &[u8]) -> Option<KeptInterior> {
        let key = path.to_ascii_lowercase();
        if let Some(k) = self.interiors.borrow().get(&key) {
            return k.clone();
        }
        if self.interiors.borrow().len() >= MAX_INTERIORS || path.len() > MAX_INTERIOR_PATH {
            return None;
        }
        let mut work = 0u64;
        let read = (self.reader)(path).and_then(|bytes| {
            read_interior(&bytes, &mut |u| {
                work = work.saturating_add(u);
                work <= INTERIOR_BUDGET
            })
        });
        self.interiors.borrow_mut().insert(key, read.clone());
        read
    }

    /// An InteriorInstance ghost: its file's hulls at its pose and scale.
    fn interior_object(&self, g: &Ghost<TgeClientSet>) -> Option<Object> {
        let Some(TgeRecord::InteriorInstance(r)) = &g.record else {
            return None;
        };
        let kept = self.interior(&r.init.as_ref()?.file)?;
        let (m, scale) = placed(&g.pose()?)?;
        let shape = InteriorShape::new(Arc::clone(&kept.geom), m.m, scale)?;
        Some(Object {
            view: view_of(m.m, kept.bounds, scale, INTERIOR_TYPE_MASK),
            source: Source::Torque(ConvexSource::Map),
            shape: None,
            map: Some(Rc::new(MapShape::Interior(shape))),
        })
    }
}

impl StaticIndex<TgeWorld> for CubeStatics {
    fn clone_box(&self) -> Box<dyn StaticIndex<TgeWorld>> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    /// The terrain, if the map has one.
    fn set_near(
        &mut self,
        world: &TgeWorld,
        reader: Option<&FileReader>,
        near: F3,
        work: &mut u64,
        budget: u64,
    ) -> Option<Box<dyn StaticSet>> {
        let _ = (world, reader, near, work, budget);
        (!self.terrain.is_empty()).then(|| Box::new(self.terrain.clone()) as Box<dyn StaticSet>)
    }

    /// A StaticShape ghost: its shape's collision hulls at its pose and scale; an InteriorInstance ghost: its interior's hulls.
    fn ghost_object(
        &mut self,
        world: &TgeWorld,
        index: u32,
        g: &Ghost<TgeClientSet>,
        reader: Option<&FileReader>,
    ) -> Option<Object> {
        let _ = (index, reader);
        if g.class == Some(TgeClass::InteriorInstance) {
            return self.interior_object(g);
        }
        if g.class != Some(TgeClass::StaticShape) {
            return None;
        }
        let path = world.datablock_text(g.datablock()?, "shapeFile")?;
        let mut work = 0;
        let shape = self
            .shapes
            .get(path, Some(&self.reader), &mut work, SHAPE_BUDGET)?;
        let (m, scale) = pose_matrix(&g.pose()?, None)?;
        Some(Object {
            view: view_of(m.m, shape.bounds(), scale, STATIC_SHAPE_TYPE_MASK),
            source: Source::Torque(ConvexSource::Shape {
                has_shape: true,
                slots: shape.hull_slots(),
            }),
            shape: Some(shape),
            map: None,
        })
    }
}
