//! What the client's prediction asks this game: the arena's cubes as the client's collision world, and the view drawn ahead
//! of the last tick by the mouse's waiting turn.
//!
//! - **The cubes** (StaticShapes) are the only static objects of the arena. The engine's client has no static collision world of
//!   its own (a game gives one, [`StaticIndex`]), so without them the own figure the client predicts falls through the floor
//!   between the server's corrections. Each cube's ghost becomes an object of the client's container as the server's container
//!   holds it: the cube shape's collision hulls (read from the kit), its pose and scale, a StaticShape's type bits.
//! - **Smooth aim** ([`PredictGame::smooth_aim`]): the view turns with the mouse within the frame, ahead of the last tick by
//!   what the next move will make of the waiting turn, instead of a tick or two behind it. The moves are unchanged.
//!
//! Credit: a cube's collision object follows `ShapeBase::buildConvex` and the StaticShape type bits in Torque3D's MIT-licensed
//! `T3D/shapeBase.cpp` and `T3D/staticShape.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc. for
//! `shapeBase.cpp`). The code here is this example's own.

use std::any::Any;
use std::rc::Rc;

use torque::client::conn::tge::TgeWorld;
use torque::client::game::ClientGame;
use torque::client::predict::drive::ShapeCache;
use torque::client::predict::game::{PredictGame, Prediction};
use torque::client::predict::{FileReader, Object, Source, StaticIndex, view_of};
use torque::collide::convex::ConvexSource;
use torque::ghost_world::Ghost;
use torque::ghost_world::tge::TgeClientSet;
use torque::ghosts::TgeClass;
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

/// The example's answers to the client's engine half: Torque's, with the arena's prediction.
pub struct ArenaClient {
    prediction: Rc<Prediction<TgeWorld>>,
}

impl std::fmt::Debug for ArenaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArenaClient")
    }
}

impl ArenaClient {
    /// The answers over the game's files `files` (the kit's shapes the cubes collide with).
    pub fn new(files: MemFiles) -> ArenaClient {
        ArenaClient {
            prediction: Rc::new(Prediction(Rc::new(ArenaPrediction { files }))),
        }
    }
}

impl ClientGame for ArenaClient {
    fn prediction(&self) -> Option<Rc<dyn Any>> {
        Some(Rc::clone(&self.prediction) as Rc<dyn Any>)
    }
}

/// The arena's prediction: the cubes, and smooth aim.
#[derive(Debug)]
pub struct ArenaPrediction {
    files: MemFiles,
}

impl PredictGame<TgeWorld> for ArenaPrediction {
    fn static_index(&self) -> Option<Box<dyn StaticIndex<TgeWorld>>> {
        let files = self.files.clone();
        let reader: FileReader = Rc::new(move |path: &[u8]| {
            files.get(&String::from_utf8_lossy(path).replace('\\', "/"))
        });
        Some(Box::new(ArenaStatics {
            reader,
            shapes: ShapeCache::default(),
        }))
    }

    fn smooth_aim(&self, vm: &Vm) -> bool {
        let _ = vm;
        true
    }
}

/// The cubes as the client's container holds them.
#[derive(Clone)]
pub struct ArenaStatics {
    reader: FileReader,
    shapes: ShapeCache,
}

impl StaticIndex<TgeWorld> for ArenaStatics {
    fn clone_box(&self) -> Box<dyn StaticIndex<TgeWorld>> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    /// A StaticShape ghost: its shape's collision hulls at its pose and scale.
    fn ghost_object(
        &mut self,
        world: &TgeWorld,
        index: u32,
        g: &Ghost<TgeClientSet>,
        reader: Option<&FileReader>,
    ) -> Option<Object> {
        let _ = (index, reader);
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
