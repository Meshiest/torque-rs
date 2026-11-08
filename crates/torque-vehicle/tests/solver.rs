//! The solver seam: a solver the host hands the step is called in order, a delegate to TGE's body gives TGE's result bit for
//! bit, and a solver that runs out of budget stops the tick where it was.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::host::Ground;
use torque_collide::math::{Box3, IDENTITY};
use torque_collide::{Exhausted, ObjectId};
use torque_vehicle::data::FlyingVehicleData;
use torque_vehicle::flying::Flying;
use torque_vehicle::solver::ContactHook;
use torque_vehicle::state::Body;
use torque_vehicle::tick::{Vehicle, process_tick};
use torque_vehicle::{BodySolver, Move, VehicleData, VehicleHost, VehicleState};

fn craft() -> FlyingVehicleData {
    let mut db = FlyingVehicleData::default();
    db.base.shape.mass = 200.0;
    db.base.shape.drag = 1.7;
    db.base.integration = 4;
    db.maneuvering_force = 6400.0;
    db.steering_force = 1500.0;
    db.rotational_drag = 20.0;
    db.preload();
    db
}

fn at(z: f32, db: &FlyingVehicleData) -> Vehicle<Flying> {
    let mut m = IDENTITY;
    m[11] = z;
    let s = VehicleState::new(
        ObjectId(3),
        &db.base,
        Box3::new([-1.0, -1.5, -0.2], [1.0, 1.5, 0.2]),
        &m,
        [1.0; 3],
    );
    Vehicle::new(s, Flying::default())
}

/// TGE's solver behind a recorder; `fail_at` makes that sub-step (counting from 1) report the budget spent.
#[derive(Debug, Clone)]
struct Tally {
    inner: Body,
    log: Rc<RefCell<Vec<&'static str>>>,
    fail_at: Option<usize>,
}

impl BodySolver for Tally {
    fn begin_tick(
        &mut self,
        s: &VehicleState,
        host: &mut dyn VehicleHost,
    ) -> Result<(), Exhausted> {
        self.log.borrow_mut().push("begin");
        self.inner.begin_tick(s, host)
    }

    fn substep(
        &mut self,
        s: &mut VehicleState,
        db: &VehicleData,
        dt: f32,
        host: &mut dyn VehicleHost,
        on_contact: Option<&mut ContactHook<'_>>,
    ) -> Result<bool, Exhausted> {
        self.log.borrow_mut().push("substep");
        let n = self
            .log
            .borrow()
            .iter()
            .filter(|e| **e == "substep")
            .count();
        if self.fail_at == Some(n) {
            return Err(Exhausted);
        }
        self.inner.substep(s, db, dt, host, on_contact)
    }

    fn end_tick(
        &mut self,
        s: &mut VehicleState,
        host: &mut dyn VehicleHost,
    ) -> Result<(), Exhausted> {
        self.log.borrow_mut().push("end");
        self.inner.end_tick(s, host)
    }

    fn clone_box(&self) -> Box<dyn BodySolver> {
        Box::new(self.clone())
    }
}

fn moves() -> [Move; 3] {
    let mv = |y: f32, yaw: f32| Move {
        y,
        yaw,
        ..Default::default()
    };
    [mv(1.0, 0.0), mv(1.0, 0.3), mv(-0.5, -0.3)]
}

fn run(
    db: &FlyingVehicleData,
    ticks: usize,
    solver: Option<Tally>,
) -> (Vehicle<Flying>, Vec<Result<(), Exhausted>>) {
    let mut v = at(0.4, db);
    let mut host = Ground {
        make_solver: solver.map(|t| {
            Box::new(move || Box::new(t.clone()) as Box<dyn BodySolver>)
                as Box<dyn Fn() -> Box<dyn BodySolver>>
        }),
        ..Default::default()
    };
    let moves = moves();
    let results = (0..ticks)
        .map(|t| process_tick(&mut v, db, Some(&moves[t % moves.len()]), &mut host))
        .collect();
    (v, results)
}

#[test]
fn a_delegate_to_tge_gives_tges_result_and_is_called_in_order() {
    let db = craft();
    let log = Rc::default();
    let tally = Tally {
        inner: Body::default(),
        log: Rc::clone(&log),
        fail_at: None,
    };
    let (plain, _) = run(&db, 30, None);
    let (hosted, results) = run(&db, 30, Some(tally));
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(plain.s.rigid.to_words(), hosted.s.rigid.to_words());
    assert_eq!(plain.s.rest_count, hosted.s.rest_count);
    let log = log.borrow();
    assert_eq!(log.len(), 30 * 6);
    for tick in log.chunks(6) {
        assert_eq!(
            tick,
            ["begin", "substep", "substep", "substep", "substep", "end"]
        );
    }
    assert!(hosted.solver.is_none(), "the solver lasts one tick");
}

#[test]
fn a_solver_past_its_budget_stops_the_tick_and_still_ends_it() {
    let db = craft();
    let log = Rc::default();
    let tally = Tally {
        inner: Body::default(),
        log: Rc::clone(&log),
        fail_at: Some(3),
    };
    let (_, results) = run(&db, 1, Some(tally));
    assert_eq!(results, vec![Err(Exhausted)]);
    assert_eq!(
        *log.borrow(),
        ["begin", "substep", "substep", "substep", "end"]
    );
}
