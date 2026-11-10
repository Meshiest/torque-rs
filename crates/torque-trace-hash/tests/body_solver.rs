//! The client's vehicles step with the body solver the game picks for their datablock: a solver that runs TGE's behind a
//! counter is used for every tick the client predicts the buggy, and the run hashes as TGE's own does.

use std::cell::Cell;
use std::rc::Rc;

use torque_client::conn::WorldOf;
use torque_client::conn::tge::TgeConn;
use torque_client::game::ClientGame;
use torque_client::predict::FileReader;
use torque_client::predict::game::{PredictGame, Prediction};
use torque_trace_hash::rig::{TraceClient, files};
use torque_trace_hash::scenarios::{Baseline, buggy, buggy_with_client_game};
use torque_vehicle::solver::ContactHook;
use torque_vehicle::state::Body;
use torque_vehicle::{BodySolver, VehicleData, VehicleHost, VehicleState};

type World = WorldOf<TgeConn>;

/// TGE's solver, counting the ticks it begins.
#[derive(Debug, Clone, Default)]
struct Counted {
    inner: Body,
    ticks: Rc<Cell<u32>>,
}

impl BodySolver for Counted {
    fn begin_tick(
        &mut self,
        s: &VehicleState,
        host: &mut dyn VehicleHost,
    ) -> Result<(), torque_collide::Exhausted> {
        self.ticks.set(self.ticks.get() + 1);
        self.inner.begin_tick(s, host)
    }

    fn substep(
        &mut self,
        s: &mut VehicleState,
        db: &VehicleData,
        dt: f32,
        host: &mut dyn VehicleHost,
        on_contact: Option<&mut ContactHook<'_>>,
    ) -> Result<bool, torque_collide::Exhausted> {
        self.inner.substep(s, db, dt, host, on_contact)
    }

    fn end_tick(
        &mut self,
        s: &mut VehicleState,
        host: &mut dyn VehicleHost,
    ) -> Result<(), torque_collide::Exhausted> {
        self.inner.end_tick(s, host)
    }

    fn clone_box(&self) -> Box<dyn BodySolver> {
        Box::new(Counted {
            inner: Body::default(),
            ticks: Rc::clone(&self.ticks),
        })
    }
}

#[derive(Debug)]
struct Solving(Rc<Cell<u32>>);

impl PredictGame<World> for Solving {
    fn body_solver(&self, _world: &World, _db: u32) -> Option<Box<dyn BodySolver>> {
        Some(Box::new(Counted {
            inner: Body::default(),
            ticks: Rc::clone(&self.0),
        }))
    }
}

#[derive(Debug)]
struct Game(TraceClient, Rc<Cell<u32>>);

impl ClientGame for Game {
    fn file_reader(&self) -> Option<FileReader> {
        self.0.file_reader()
    }

    fn prediction(&self) -> Option<Rc<dyn std::any::Any>> {
        let game: Rc<dyn PredictGame<World>> = Rc::new(Solving(Rc::clone(&self.1)));
        Some(Rc::new(Prediction(game)))
    }
}

#[test]
fn the_client_steps_its_vehicle_with_the_games_solver() {
    let ticks = Rc::new(Cell::new(0));
    let solved =
        buggy_with_client_game(Rc::new(Game(TraceClient(files()), Rc::clone(&ticks)))).unwrap();
    assert!(
        ticks.get() >= solved.driven_ticks,
        "{} solver ticks for {} ticks driven",
        ticks.get(),
        solved.driven_ticks
    );
    assert_eq!(solved.hash, buggy(Baseline).unwrap().hash);
}
