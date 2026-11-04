//! `advanceObjects`: one pass over the process list for each 32 ms step.
//!
//! For each object, in list order (see `::sim::process`): a connection's control object runs the host's control branch
//! ([`ControlSide`]: the moves the connection credited, one tick each; no move, no tick), every other object gets
//! `processTick(NULL)` with a hold on it (its own `delete` waits for the tick to return). Objects the tick made are not visited
//! in this pass; objects it deleted are skipped.
//!
//! **Lag compensation** (engine-only, off by default): after each step's pass, a sample of every Player
//! ([`crate::scene::lag::record_step`]).
//!
//! **Budget** (engine-only): a tick runs outside any script run, so the pass gives each object's turn a host work budget
//! that `Vm::charge_work` enforces ([`TICK_OBJECT_WORK`], [`TICK_PASS_WORK`]) and charges each visit to the run's step budget
//! before the turn ([`advance_objects_within`]). The scripts a turn runs (every callback of the tick: `stateScript`, `onMount`,
//! `onCollision`, `onDamage`, `onTrigger`, a deferred delete's `onRemove`, ...) are outermost runs under that budget, so
//! `Vm::begin_host_budget_with_runs` charges them too: together at most the `Vm`'s own step limit and what is left of the
//! pass's budget, their steps added to the pass's.
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree:
//! `T3D/gameBase/processList.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.);
//! `T3D/gameBase/gameProcess.cpp` (Copyright (c) 2012 GarageGames, LLC). The code here is this crate's own.

use ts_vm::sim::ObjId;
use ts_vm::vm::{Vm, VmError, WORK_PER_STEP};

use super::{ProcessHandle, TickCx, Ticker};
use crate::scene::camera_tick::DirectCalls;

/// The control branch of the pass (the network host's part).
pub trait ControlSide {
    /// Before each step.
    fn begin_step(&mut self);

    /// If `obj` is the control object of a connection right now: its moves for this step are taken, `ticker` runs on each while
    /// the object is still that connection's control object and still exists, and the ones that ran are cleared (`getMoveList`,
    /// `processTick(move)`, `clearMoves`). True when `obj` was a control object (whether or not a move ran).
    fn run_control(&mut self, vm: &mut Vm, obj: ObjId, ticker: &mut Ticker) -> bool;

    /// After the batch of `steps` steps: the move credit.
    fn end_batch(&mut self, steps: u32);
}

/// No connections: nothing is a control object.
pub struct NoControl;

impl ControlSide for NoControl {
    fn begin_step(&mut self) {}

    fn run_control(&mut self, _vm: &mut Vm, _obj: ObjId, _ticker: &mut Ticker) -> bool {
        false
    }

    fn end_batch(&mut self, _steps: u32) {}
}

/// The work (units; [`WORK_PER_STEP`] make a step) one object's turn in a pass may do outside a script: its tick's container
/// queries, rays, hulls, GJK, animation and the skeleton, every move of a control object's turn together. A bound of the
/// engine's: an ordinary Player's or AIPlayer's turn uses at most about 45,000 units, so this
/// is 23 times that; a turn that asks for more (a ray to an object 1.5e10 away walks 2.9e7 bin segments, a second of work)
/// stops where the budget refuses, deterministically, as `::sim` stops a step whose query is refused (about 6 ms in a release
/// build).
pub const TICK_OBJECT_WORK: u64 = 1 << 20;

/// The work all the objects of one 32 ms step may do together outside scripts (engine-only): an object whose turn comes after
/// the step spent it ticks with no budget (every query refused: it stays where it is). With [`TICK_OBJECT_WORK`] it bounds a
/// step of N hostile objects at this much, whatever N is.
pub const TICK_PASS_WORK: u64 = 1 << 22;

/// What a pass did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The objects visited.
    pub visits: u64,
    /// The steps the pass charged: one per visit, paid before the object's turn, its tick's work ([`WORK_PER_STEP`] to a step,
    /// rounded up) and the steps of the scripts the turn ran. At most the budget the pass was given.
    pub steps: u64,
    /// The steps of the scripts the turns ran (part of `steps`).
    pub script_steps: u64,
    /// The work the ticks did outside scripts, in all and the most one object's turn did.
    pub work: u64,
    pub peak_object_work: u64,
    /// The pass ran out of its step budget: the objects not visited yet were left for the next pass (and the remaining steps of
    /// the batch were not run).
    pub stopped: bool,
    /// The errors of the scripts a deferred delete ran when a tick returned (`onRemove`).
    pub errors: Vec<VmError>,
}

/// [`advance_objects_within`] with no step budget of the run's (the per-object and per-step work limits still hold).
pub fn advance_objects(
    vm: &mut Vm,
    list: &ProcessHandle,
    control: &mut dyn ControlSide,
    steps: u32,
) -> Outcome {
    advance_objects_within(vm, list, control, steps, u64::MAX)
}

/// `steps` 32 ms steps over the list: sorted first when dirty, then one `advanceObjects` per step, then the move credit.
///
/// **Budget.** Each visit costs a step of `budget` before the object's turn, and the turn runs under a host work budget
/// ([`Vm::begin_host_budget_with_runs`]) of [`TICK_OBJECT_WORK`], less when the step's [`TICK_PASS_WORK`] or `budget` has less
/// left; the scripts the turn runs may take together the `Vm`'s step limit, less when `budget` has less left once the turn's
/// work limit is set aside (so a turn never takes the pass past `budget`); what the turn charged and its scripts' steps are
/// added to the steps. A script past its share stops with `VmError::StepLimit`, and every later one of the turn at its first
/// instruction. Once `budget` is spent the pass stops before the next object.
pub fn advance_objects_within(
    vm: &mut Vm,
    list: &ProcessHandle,
    control: &mut dyn ControlSide,
    steps: u32,
    budget: u64,
) -> Outcome {
    let mut out = Outcome::default();
    list.borrow_mut().order_list_if_dirty();
    'batch: for _ in 0..steps {
        control.begin_step();
        list.borrow_mut().begin_pass();
        let mut pass_work = 0u64;
        loop {
            if out.steps >= budget {
                out.stopped = true;
                list.borrow_mut().finish_pass();
                break 'batch;
            }
            let Some(visit) = list.borrow_mut().next_in_pass() else {
                break;
            };
            // A payload that is out means the object is being ticked further up (a tick that ran the pass again): not ticked
            // twice.
            let Some(mut ticker) = visit.payload else {
                continue;
            };
            out.visits = out.visits.saturating_add(1);
            out.steps = out.steps.saturating_add(1);
            let left = budget.saturating_sub(out.steps);
            let limit = TICK_OBJECT_WORK
                .min(TICK_PASS_WORK.saturating_sub(pass_work))
                .min(left.saturating_mul(WORK_PER_STEP));
            let scripts = vm
                .step_limit()
                .unwrap_or(u64::MAX)
                .min(left.saturating_sub(limit.div_ceil(WORK_PER_STEP)));
            let obj = visit.key;
            let saved = vm.begin_host_budget_with_runs(limit, scripts);
            // The control branch runs whatever `mProcessTick` says; `processTick(NULL)` only when it is set.
            if !control.run_control(vm, obj, &mut ticker) && visit.ticks {
                vm.hold(obj);
                {
                    let mut calls = DirectCalls;
                    let mut cx = TickCx {
                        vm: &mut *vm,
                        calls: &mut calls,
                    };
                    ticker.process_tick(&mut cx, None);
                }
                if let Err(e) = vm.release(obj) {
                    out.errors.push(e);
                }
            }
            // A refused charge was not done: what the turn did is at most its limit (and its scripts' steps at most their
            // share).
            let script_steps = vm.host_run_steps();
            let work = vm.end_host_budget(saved).min(limit);
            pass_work = pass_work.saturating_add(work);
            out.work = out.work.saturating_add(work);
            out.peak_object_work = out.peak_object_work.max(work);
            out.script_steps = out.script_steps.saturating_add(script_steps);
            out.steps = out
                .steps
                .saturating_add(work.div_ceil(WORK_PER_STEP))
                .saturating_add(script_steps);
            list.borrow_mut().restore(obj, visit.serial, ticker);
        }
        list.borrow_mut().finish_pass();
        if crate::scene::lag::wants_steps(vm) {
            let order = list.order();
            crate::scene::lag::record_step(vm, &order);
        }
    }
    control.end_batch(steps);
    out
}
