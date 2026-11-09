//! The end of a run and the natives around it: `quit` (TGE's `Platform::postQuitMessage`: the
//! host's loop stops after the current tick), `deleteDataBlocks`, and [`Vm::shutdown`], the deterministic end
//! of a `Vm` (TGE's `Sim::shutdown` after `onExit`). Also the small public helpers a host's natives
//! use (`charge_work`, `file_host`, `error_with_backtrace`).
//!
//! Follows Torque3D's MIT-licensed `console/simManager.cpp` (Copyright (c) 2012 GarageGames, LLC) and
//! `console/simDatablock.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (c) 2015 Faust Logic, Inc.); the code
//! here is this crate's own.

use super::{LogLine, Native, Vm, VmError};
use crate::sim::{FIRST_DATABLOCK_ID, ObjId};

/// The usage string of `quit` (TGE's console function; a host that replaces the native passes this one on).
pub const QUIT_USAGE: &str = "quit() - asks the main loop to stop";

/// The usage string of `deleteDataBlocks` (TGE's; two spaces after "datablocks,").
pub const DELETE_DATABLOCKS_USAGE: &str =
    "deleteDataBlocks() - deletes every datablock, as before loading another set of them";

pub(super) fn register(vm: &mut Vm) {
    vm.add_native_effects("", "quit", crate::effects::Effects::ENGINE);
    vm.add_native_effects("", "deleteDataBlocks", crate::effects::Effects::ENGINE);
    // TGE's `ConsoleFunction(quit, void, 1, 1, ...)`: asks the main loop to stop; returns at once and the script goes
    // on (an event scheduled after it never fires because the host's loop ends; `onExit` runs). The host's loop asks
    // `quit_requested`. A host may replace this native to print its shutdown lines first (and call `request_quit`).
    vm.register("quit", 1, 1, QUIT_USAGE, |vm, _| {
        vm.request_quit();
        Native::Void
    });
    // TGE 1.4's `deleteDataBlocks`.
    vm.register(
        "deleteDataBlocks",
        1,
        1,
        DELETE_DATABLOCKS_USAGE,
        |vm, _| match vm.delete_datablocks() {
            Ok(()) => Native::Void,
            Err(e) => Native::Fail(e),
        },
    );
}

impl Vm {
    /// Asks the host to stop (the `quit` native): see [`quit_requested`](Self::quit_requested). A host that
    /// replaces `quit` calls this too.
    pub fn request_quit(&mut self) {
        self.quit_requested = true;
    }

    /// Whether a script called `quit()`: the host's loop finishes the tick it is in (`advance_time` itself does
    /// not look at it, as TGE's `Sim::advanceTime` does not), runs `onExit` and calls
    /// [`shutdown`](Self::shutdown).
    pub fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    /// `deleteDataBlocks` (as Torque3D's MIT-licensed `console/simDatablock.cpp`, Copyright (c) 2012 GarageGames, LLC;
    /// Copyright (c) 2015 Faust Logic, Inc.): `deleteObject` on every member of DataBlockGroup,
    /// the last first (a datablock a running method holds stays until the method returns), then the
    /// next datablock id is 3 again. Public for a host that wraps the native. Every datablock is charged, and
    /// the step budget is asked after each, so a group of millions ends at the step limit.
    ///
    /// Each datablock deleted now posts a delete event; one a running method holds posts it when the method
    /// returns.
    ///
    /// A call from inside a removal this loop is carrying out (a behaviour's `on_remove` that asks for the
    /// datablocks to go again) does nothing: the outer loop is deleting them, and a nested one would recurse
    /// once per datablock.
    #[inline(never)]
    pub fn delete_datablocks(&mut self) -> Result<(), VmError> {
        if self.deleting_datablocks {
            return Ok(());
        }
        self.deleting_datablocks = true;
        let result = self.delete_datablock_members();
        self.deleting_datablocks = false;
        result?;
        self.objects.set_next_datablock_id(FIRST_DATABLOCK_ID);
        Ok(())
    }

    /// The loop of [`delete_datablocks`](Self::delete_datablocks), from the last member down: what a removal
    /// does to the group meanwhile is seen at once (the next index is read from the live list).
    #[inline(never)]
    fn delete_datablock_members(&mut self) -> Result<(), VmError> {
        let group = self.objects.datablock_group();
        let count = self.objects.members(group).map_or(0, |m| m.len());
        for index in (0..count).rev() {
            self.objects.add_work(1);
            self.ensure_run_budget()?;
            let member = self.objects.members(group).and_then(|m| m.get(index));
            if let Some(id) = member {
                self.delete_object(id)?;
            }
        }
        Ok(())
    }

    /// The end of the `Vm` (TGE's `Sim::shutdown`, after `onExit`): every object is deleted, RootGroup's
    /// tree first (a group's members by descending id, as a cascade), then whatever is left by descending
    /// id, with no script callback (no `onRemove` runs at shutdown) but every class behaviour's
    /// `on_remove`, which releases Rust state (a behaviour's error is ignored: nothing can run after it, and
    /// the object is out of the tables whatever it says). Pending events never fire: the `Vm` is consumed.
    /// Returns the log the `Vm` still holds ([`Vm::drain_log`]: what a host took earlier, and what the cap dropped,
    /// are not in it), with whatever the behaviours printed.
    ///
    /// Shutdown is the host's, not a script's: it is not charged to a step budget, and its cost is that of
    /// deleting what the script built (which was charged when it was built).
    pub fn shutdown(mut self) -> Vec<LogLine> {
        // `iter` is in ascending id order, RootGroup's (`u32::MAX`) last: the last id first makes it the first,
        // and its cascade takes the whole tree (a group's members by descending id); what is outside the tree
        // follows, the last id first. A deletion that is given an error already on its way out runs no script
        // callback.
        let all: Vec<ObjId> = self.objects.iter().map(|(id, _)| id).collect();
        for id in all.into_iter().rev() {
            let _ = self.delete_now(id, Some(VmError::StepLimit));
        }
        self.drain_log()
    }

    /// Charges `units` of work (see [`WORK_PER_STEP`](super::WORK_PER_STEP)): what a native does in proportion to its
    /// data (a host's natives too), and says whether the budget can still pay for it; false once it cannot, and the
    /// caller should then stop before it does the work (a native ends the run with [`VmError::StepLimit`]). Loops in
    /// natives call it at every round.
    ///
    /// Inside a run the run's step budget pays. Outside one, a host budget opened with
    /// [`begin_host_budget`](Self::begin_host_budget) pays (the process list's pass opens one for each object it ticks:
    /// a tick runs outside any run); its refusals are sticky until it is closed. Outside both nothing is billed
    /// (`enter_run` discards what was counted) and the answer is true.
    #[inline(never)]
    pub fn charge_work(&self, units: u64) -> bool {
        if self.running == 0
            && let Some(limit) = self.host_budget.limit
        {
            let used = self.host_budget.used.get().saturating_add(units);
            self.host_budget.used.set(used);
            return used <= limit;
        }
        self.objects.add_work(units);
        self.ensure_run_budget().is_ok()
    }

    /// Opens a budget of `units` of work for the host's own work outside a run: until
    /// [`end_host_budget`](Self::end_host_budget), [`charge_work`](Self::charge_work) outside a run counts against it
    /// and refuses once the count passes it. A script the host runs meanwhile (a callback) is a run of its own and pays
    /// from the run's budget. Budgets nest: the one this replaces comes back when this one ends (pass what this returns
    /// to `end_host_budget`).
    pub fn begin_host_budget(&mut self, units: u64) -> HostBudgetSaved {
        self.open_host_budget(units, None)
    }

    /// [`begin_host_budget`](Self::begin_host_budget), and the scripts the host runs meanwhile are charged too (a
    /// tick's callbacks): every outermost run started while this budget is open runs under a step limit of the smaller
    /// of the `Vm`'s own ([`set_step_limit`](Self::set_step_limit)) and what is left of `run_steps`, and the steps it
    /// took (at most what was left) are taken from `run_steps` when it ends ([`host_run_steps`](Self::host_run_steps)).
    /// Once `run_steps` is spent the refusal is sticky until the budget is closed: a later run's first instruction ends
    /// it with [`VmError::StepLimit`] (a native it calls first is refused at its first charge). A run nested in a run
    /// pays from that run, as before.
    pub fn begin_host_budget_with_runs(&mut self, units: u64, run_steps: u64) -> HostBudgetSaved {
        self.open_host_budget(units, Some(run_steps))
    }

    fn open_host_budget(&mut self, units: u64, runs: Option<u64>) -> HostBudgetSaved {
        let saved = HostBudgetSaved {
            limit: self.host_budget.limit,
            used: self.host_budget.used.get(),
            runs: self.host_budget.runs,
            runs_used: self.host_budget.runs_used,
        };
        self.host_budget.limit = Some(units);
        self.host_budget.used.set(0);
        self.host_budget.runs = runs;
        self.host_budget.runs_used = 0;
        saved
    }

    /// The steps the outermost runs started under the open host budget have taken from its `run_steps`
    /// ([`begin_host_budget_with_runs`](Self::begin_host_budget_with_runs)); 0 for a budget with none. Read it before
    /// [`end_host_budget`](Self::end_host_budget).
    pub fn host_run_steps(&self) -> u64 {
        self.host_budget.runs_used
    }

    /// Whether host work outside a run is paid by an open host budget ([`begin_host_budget`](Self::begin_host_budget)) now.
    pub(super) fn host_budget_pays(&self) -> bool {
        self.running == 0 && self.host_budget.limit.is_some()
    }

    /// Closes the host budget [`begin_host_budget`](Self::begin_host_budget) opened (the one before it is open again)
    /// and returns the work charged to it (more than its limit once it refused: the refused charges are counted).
    pub fn end_host_budget(&mut self, saved: HostBudgetSaved) -> u64 {
        let used = self.host_budget.used.replace(saved.used);
        self.host_budget.limit = saved.limit;
        self.host_budget.runs = saved.runs;
        self.host_budget.runs_used = saved.runs_used;
        used
    }

    /// The `Vm`'s step limit for one outermost run ([`set_step_limit`](Self::set_step_limit)).
    pub fn step_limit(&self) -> Option<u64> {
        self.step_limit
    }

    /// `enter_run`'s part for an outermost run under a host budget with run steps: the run's limit is lowered to what
    /// is left of them (the `Vm`'s own comes back at [`end_charged_run`](Self::end_charged_run)). Out of line: off the
    /// recursion path's frames.
    #[inline(never)]
    pub(super) fn begin_charged_run(&mut self) {
        let Some(runs) = self.host_budget.runs else {
            return;
        };
        let left = runs.saturating_sub(self.host_budget.runs_used);
        self.host_budget.replaced = Some(self.step_limit);
        self.step_limit = Some(self.step_limit.map_or(left, |own| own.min(left)));
    }

    /// `leave_run`'s part for an outermost run [`begin_charged_run`](Self::begin_charged_run) lowered: its steps (the
    /// work of the native it ended with billed) are taken from the host budget's run steps, at most what was left (a
    /// refused charge was not done), and the `Vm`'s own limit is back.
    #[inline(never)]
    pub(super) fn end_charged_run(&mut self) {
        let Some(own) = self.host_budget.replaced.take() else {
            return;
        };
        if let Some(runs) = self.host_budget.runs {
            self.host_budget.runs_used = self
                .host_budget
                .runs_used
                .saturating_add(self.steps)
                .min(runs);
        }
        self.step_limit = own;
    }

    /// `ensure_budget` inside a run; outside one `steps` is the last run's and nothing is billed (as
    /// `compile_source` has it), so there is nothing to check.
    fn ensure_run_budget(&self) -> Result<(), VmError> {
        if self.running > 0 {
            self.ensure_budget()
        } else {
            Ok(())
        }
    }

    /// The files scripts see (for a host's natives and behaviours: no message is printed, nothing is
    /// charged).
    pub fn file_host(&self) -> &dyn crate::host::FileHost {
        self.files.as_ref()
    }

    /// A blank line, `text` at error level, a BackTrace and a blank line: a loud report of a script's mistake, for
    /// a host's natives; an error of a host-replaced
    /// `backtrace` is returned.
    pub fn error_with_backtrace(&mut self, text: String) -> Result<(), VmError> {
        self.print(b"");
        self.error(text);
        self.print_backtrace()?;
        self.print(b"");
        Ok(())
    }
}

/// The budget a [`Vm::begin_host_budget`] replaced, given back to [`Vm::end_host_budget`].
#[derive(Debug)]
#[must_use]
pub struct HostBudgetSaved {
    limit: Option<u64>,
    used: u64,
    runs: Option<u64>,
    runs_used: u64,
}

/// The host budget of [`Vm::begin_host_budget`].
#[derive(Debug, Default)]
pub(super) struct HostBudget {
    limit: Option<u64>,
    used: std::cell::Cell<u64>,
    /// The steps the outermost runs started under it may take together ([`Vm::begin_host_budget_with_runs`]), and what
    /// they took.
    runs: Option<u64>,
    runs_used: u64,
    /// The `Vm`'s own step limit while an outermost run under `runs` is in progress.
    replaced: Option<Option<u64>>,
}
