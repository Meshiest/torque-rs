//! TGE's game classes on a server: torque-game's classes put together over one container, one shape loader and the server's
//! process list ([`install`]), the way a TGE server has them.
//!
//! - the shape loader ([`torque_game::shapes::register`]: a `ShapeBaseData` loads its shape at `preload`, and its computed
//!   values are the server's [`ServerState::db_values`]);
//! - the scene classes over one container ([`torque_game::scene::object`], the container natives, [`torque_game::scene::shapebase`],
//!   the Player, the Item, the camera methods), the movers' environment over the same container and shapes, set on the
//!   process list, and the write observer that rebuilds a Player's datablock after a script changes it;
//! - `GameBase::onAdd`: an object with no datablock is refused (its script `onRemove` runs first); otherwise it takes the type
//!   mask its class's constructor sets, the classes' `onNewDataBlock` run (the camera's, the shape's, the Player's, the Item's, the vehicles'), then the datablock's script
//!   `onNewDataBlock(%obj)`, and the object joins the process list at the tail. `onRemove` takes it out again.
//!
//! A network host ([`crate::net::attach_host`]) given the same container ghosts these objects; without one they still tick.
//!
//! Credit: `GameBase::onAdd` and `onRemove` follow TGE as Torque3D, its MIT-licensed release, keeps them:
//! `T3D/gameBase/gameBase.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is
//! this crate's own.

use std::cell::RefCell;
use std::rc::Rc;

use torque_game::scene::access::object_field;
use torque_game::scene::container::{Container, ContainerConfig};
use torque_game::scene::ext;
use torque_game::shapes::Shapes;
use torque_game::sim::ProcessHandle;
use torque_game::sim::player::tick::SimEnv;
use torque_scene::BinGeometry;
use torque_ts_vm::classes::EngineClass;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::state::{NetParts, ServerState};

/// What [`install`] made: the container the scene classes and the movers share, and the shapes the datablocks loaded.
#[derive(Clone)]
pub struct GameParts {
    pub container: Rc<Container>,
    pub shapes: Rc<Shapes>,
}

/// `GameBase`'s behaviour over the server's process list.
struct GameBaseClass {
    process: ProcessHandle,
}

impl EngineClass for GameBaseClass {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        let Some(db) = object_field(vm, id, b"dataBlock") else {
            vm.call_method(id, b"onRemove", &[])?;
            return Ok(false);
        };
        // The type mask the class's constructor sets (`StaticShapeObjectType`, `PlayerObjectType`, ...), which the datablock's
        // work and the movers' queries read.
        if let Some(mask) = torque_game::scene::shapebase::ctor_type_mask(vm, id)
            && let Some(e) = ext::get_mut(vm.objects_mut(), id)
        {
            e.type_mask |= mask;
        }
        torque_game::scene::camera::new_datablock(vm, id);
        torque_game::scene::shapebase::new_datablock(vm, id, db)?;
        torque_game::sim::player::new_datablock(vm, id, db)?;
        torque_game::sim::item::new_datablock(vm, id, db);
        torque_game::sim::vehicle::new_datablock(vm, id, db);
        vm.call_method(db, b"onNewDataBlock", &[id.to_string().into_bytes()])?;
        if let Some(e) = ext::get_mut(vm.objects_mut(), id) {
            e.process = Some(self.process.clone());
        }
        self.process.register(vm, id);
        Ok(true)
    }

    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        self.process.unregister(id);
        torque_game::scene::camera::orbit_removed(vm, id);
        Ok(())
    }
}

/// Gives `vm` (whose class table has TGE's game classes: [`torque_game::specs::install`]) the game classes' behaviours and
/// methods over a new container, and the server's process list the movers' environment. Run it before the scripts that make
/// datablocks and objects, and before a network host is attached (pass it [`GameParts::container`]).
pub fn install<N: NetParts>(vm: &mut Vm, state: &Rc<RefCell<ServerState<N>>>) -> GameParts {
    let shapes = torque_game::shapes::register(vm);
    state.borrow_mut().db_values = shapes.values().clone();
    let container = Rc::new(Container::new(ContainerConfig {
        bins: BinGeometry::default(),
        indexes: Vec::new(),
        candidate_order: None,
    }));
    let process = state.borrow().process.clone();
    torque_game::scene::object::register(vm, &container);
    vm.set_class_behaviour(
        "GameBase",
        Rc::new(GameBaseClass {
            process: process.clone(),
        }),
    );
    torque_game::scene::container::register_natives(vm, &container, &shapes);
    torque_game::scene::shapebase::register(vm, &container);
    torque_game::sim::player::register(vm, &container);
    torque_game::sim::item::register(vm, &container);
    torque_game::sim::vehicle::register(vm, &container);
    let env = Rc::new(SimEnv::new(
        Rc::clone(&container),
        Rc::clone(&shapes),
        torque_game::masks::type_masks(vm),
        torque_game::sim_classes::sim_classes(vm),
    ));
    torque_game::sim::image::register(vm, &env);
    process.set_env(env);
    vm.set_static_write_observer(Some(Rc::new(torque_game::sim::player::datablock_written)));
    torque_game::scene::camera::register(vm);
    GameParts { container, shapes }
}
