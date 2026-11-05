//! `schedule` and the event natives (`schedule`, `SimObject::schedule`, `cancel`, `isEventPending`, `getSimTime`),
//! and the host's clock (`Vm::advance_time`).
//!
//! A game can count the schedules a script makes: `schedule` and its method ask the allocation policy
//! ([`AllocPolicy`](crate::policy::AllocPolicy)) first and refuse with -1 when it says so, and tell it about the
//! event until it fires, is cancelled or is dropped with its object; [`Vm::schedule_global_with`] and
//! [`Vm::schedule_method_with`] are the same calls with the count on or off.
//!
//! TGE has no cap on pending events. The caps here are in `events`: at most
//! `MAX_PENDING_EVENTS` pending events holding `MAX_PENDING_BYTES` of arguments, and `EventLimits` for the
//! events one `advance_time` fires.
//!
//! Every path that reads or changes the queue first drops the events of objects unregistered since the last
//! look (`drop_events_of_unregistered`): TGE cancels them inside `unregisterObject`, the object table
//! here only records the ids.
//!
//! Follows Torque3D's MIT-licensed `console/simManager.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `console/simEvents.cpp` (Copyright (c) 2012 GarageGames, LLC) and `console/consoleFunctions.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use std::collections::HashMap;

use super::{Native, NativeCall, Vm, VmError};
use crate::events::{Event, EventKind, EventLimits, EventQueue};
use crate::sim::{ObjId, ROOT_GROUP_ID};
use crate::text::atoi;

fn arg(c: &NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&b""[..], Vec::as_slice)
}

/// The event id a native was given (`dAtoi`).
fn seq_arg(c: &NativeCall) -> u32 {
    atoi(arg(c, 1)) as u32
}

/// What one [`Vm::advance_time_counted`] did: the errors of the events that failed (as
/// [`Vm::advance_time`] returns them) and the steps all the events it fired used together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advance {
    pub errors: Vec<VmError>,
    pub steps: u64,
    /// The call ended on its time slice ([`EventLimits::per_pass_steps`]) with events still due: they fire first in the
    /// next call. Not an error.
    pub sliced: bool,
}

/// A native event as its handler gets it: the `tag` and `arg` it was posted with and the object it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeEvent {
    pub tag: u32,
    pub arg: u32,
    pub dest: ObjId,
}

/// The host's code for the native events ([`Vm::set_native_event_handler`]).
pub type NativeEventHandler = std::rc::Rc<dyn Fn(&mut Vm, NativeEvent) -> Result<(), VmError>>;

/// What a host's [`CallOverride`] is told of the pass it runs in (`Vm::advance_time_counted`'s limits and what its
/// events used so far), so that the work it does in place of several events stops where the pass would have stopped
/// them.
#[derive(Debug, Clone, Copy)]
pub struct PassRoom {
    /// When the pass began, when it has a time slice of the wall ([`EventLimits::per_pass_wall`]).
    pub started: Option<web_time::Instant>,
    pub wall: Option<std::time::Duration>,
    /// The pass's slice of steps ([`EventLimits::per_pass_steps`]) and its total budget of steps, when steps are counted.
    pub slice: Option<u64>,
    pub budget: Option<u64>,
    /// The steps the pass's events used before this one.
    pub steps: u64,
}

impl PassRoom {
    /// Whether the pass would stop after `more` steps of this event: past its slice of steps or its budget, or past its
    /// slice of the wall.
    pub fn is_spent(&self, more: u64) -> bool {
        let steps = self.steps.saturating_add(more);
        self.slice.is_some_and(|slice| steps > slice)
            || self.budget.is_some_and(|budget| steps >= budget)
            || self
                .started
                .zip(self.wall)
                .is_some_and(|(t, wall)| t.elapsed() > wall)
    }
}

/// What a [`CallOverride`] did: the steps its runs used together and their errors (as the events it stood for would have
/// returned them).
#[derive(Debug, Default)]
pub struct OverrideRun {
    pub steps: u64,
    pub errors: Vec<VmError>,
}

/// A host's code for a scheduled call of one global function ([`Vm::set_call_override`]): given the call's arguments and the
/// pass, it does the event's work itself and answers what it did, or None to leave the event to the function (it is then
/// called as any event is). It is called outside any run; the runs it makes are outermost runs of their own.
pub type CallOverride = std::rc::Rc<dyn Fn(&mut Vm, &[Vec<u8>], &PassRoom) -> Option<OverrideRun>>;

/// The handler of the native events and the host's call override (boxed in the `Vm`).
#[derive(Default)]
pub(super) struct NativeEvents {
    handler: Option<NativeEventHandler>,
    /// The folded name of the global function whose scheduled calls the override sees, and the override.
    call_override: Option<(Vec<u8>, CallOverride)>,
}

pub(super) fn register(vm: &mut Vm) {
    vm.register(
        "schedule",
        4,
        0,
        "schedule(delay, object, function, args...) - calls the function later; the object bounds the call's life",
        |vm, c| Native::Int(vm.schedule_global_with(c, true) as i32),
    );
    vm.register_in(
        "SimObject",
        "schedule",
        4,
        0,
        "object.schedule(delay, method, args...) - calls a method of the object later",
        |vm, c| Native::Int(vm.schedule_method_with(c, true) as i32),
    );
    vm.register(
        "cancel",
        2,
        2,
        "cancel(scheduleId) - drops a scheduled call",
        |vm, c| {
            vm.cancel_event(seq_arg(c));
            Native::Void
        },
    );
    vm.register(
        "isEventPending",
        2,
        2,
        "isEventPending(scheduleId) - whether the scheduled call is still waiting",
        |vm, c| {
            vm.drop_events_of_unregistered();
            Native::Bool(vm.events.get(seq_arg(c)).is_some())
        },
    );
    vm.register(
        "getSimTime",
        1,
        1,
        "getSimTime() - the simulation time in milliseconds",
        |vm, _| Native::Int(vm.events.now() as i32),
    );
}

impl Vm {
    /// The simulation time in milliseconds (`getSimTime`). It wraps at 2^32 like TGE's `U32`
    /// (49.7 days), and scripts read it as a signed `S32`.
    pub fn sim_time(&self) -> u32 {
        self.events.now()
    }

    /// Sets the code that runs the native events ([`EventKind::Native`]) when they fire; none until the host sets one (a native
    /// event then does nothing, as an engine delete event does).
    pub fn set_native_event_handler(&mut self, handler: NativeEventHandler) {
        self.native_events.handler = Some(handler);
    }

    /// Sets the host's override of the scheduled calls of the global function `name` (replacing one; None removes it):
    /// when such an event fires at the outermost level (no run in progress), the override is asked first
    /// ([`CallOverride`]). A call made any other way (`invoke`, bytecode, `call()`) is not overridden.
    pub fn set_call_override(&mut self, name: &[u8], call: Option<CallOverride>) {
        self.native_events.call_override = call.map(|f| (crate::text::fold(name), f));
    }

    /// The steps the last outermost run used (what `advance_time_counted` bills an event): for a host's override, which makes
    /// runs of its own and answers their total. 0 when no step limit is set.
    pub fn last_run_steps(&self) -> u64 {
        self.steps
    }

    /// Takes the event `seq` off the queue as `advance_time` would fire it next, when it is the first due event once the due
    /// engine delete events before it are passed (they fire as nothing, as `advance_time` fires them), it is a
    /// scheduled call of the global function `name` (ignoring case) and its object is still there. For a host's
    /// [`CallOverride`] that goes on with the event's work itself, so that the order of the events is the one the queue
    /// would have kept. False (and nothing but the delete events taken) otherwise.
    pub fn take_next_call(&mut self, seq: u32, name: &[u8]) -> bool {
        self.drop_events_of_unregistered();
        let now = self.events.now();
        while self
            .events
            .peek_due(now)
            .is_some_and(|e| matches!(e.kind, EventKind::Delete))
        {
            if let Some(e) = self.events.take_due(now) {
                self.alloc_event_gone(e.seq);
            }
        }
        let Some(event) = self.events.peek_due(now) else {
            return false;
        };
        let matches = event.seq == seq
            && self.objects.contains(event.dest)
            && matches!(&event.kind, EventKind::Call { argv, on_object: false }
                if argv.first().is_some_and(|n| n.eq_ignore_ascii_case(name)));
        if !matches {
            return false;
        }
        if let Some(e) = self.events.take_due(now) {
            self.alloc_event_gone(e.seq);
        }
        true
    }

    /// The host's override of `event`, when it has one for the event's call and takes it.
    #[inline(never)]
    fn try_override(&mut self, event: &Event, room: &PassRoom) -> Option<OverrideRun> {
        if self.running != 0 {
            return None;
        }
        let EventKind::Call {
            argv,
            on_object: false,
        } = &event.kind
        else {
            return None;
        };
        let (name, args) = argv.split_first()?;
        let call = match &self.native_events.call_override {
            Some((folded, call)) if name.eq_ignore_ascii_case(folded) => std::rc::Rc::clone(call),
            _ => return None,
        };
        if !self.objects.contains(event.dest) {
            return None;
        }
        call(self, args, room)
    }

    /// Queues a native event `delay_ms` from now (`time = now + (U32)delay`, as a script's `schedule` does) for `dest`
    /// and returns its sequence number, or None (with the warning `schedule` prints) when the queue is full. It takes
    /// the next number a `schedule` would, counts in `getNumSchedules`, and is dropped with `dest`.
    pub fn post_native_event(
        &mut self,
        delay_ms: u32,
        dest: ObjId,
        tag: u32,
        arg: u32,
    ) -> Option<u32> {
        self.drop_events_of_unregistered();
        let time = self.events.now().wrapping_add(delay_ms);
        let posted = self.events.post(time, dest, EventKind::Native { tag, arg });
        if posted.is_none() {
            // TGE cannot refuse an event; this queue has a cap, and says so as `schedule` does.
            self.warn(format!(
                "schedule: the event queue is full ({} events or {} bytes of arguments at most)",
                self.event_limits.pending, self.event_limits.pending_bytes
            ));
        }
        posted
    }

    /// Takes a pending native event out of the queue; false for a number
    /// that is not pending or is not a native event.
    pub fn cancel_native_event(&mut self, seq: u32) -> bool {
        match self.events.get(seq) {
            Some(event) if matches!(event.kind, EventKind::Native { .. }) => {
                self.events.remove(seq);
                true
            }
            _ => false,
        }
    }

    /// When the pending event with this sequence number is due (U32 milliseconds of simulation time), None if no such
    /// event is pending.
    pub fn event_due(&self, seq: u32) -> Option<u32> {
        self.events.get(seq).map(|event| event.time)
    }

    /// Whether an event with this sequence number is pending (of any kind).
    pub fn event_pending(&self, seq: u32) -> bool {
        self.events.get(seq).is_some()
    }

    /// Runs `body` as an outermost run (or a nested one when a run is in progress): the step budget restarts at 0 for
    /// the outermost, `charge_work` is billed inside it, and the callbacks it makes share its budget. For a host that
    /// does a native event's work itself and wants the work paid for.
    pub fn in_run<T>(
        &mut self,
        body: impl FnOnce(&mut Vm) -> Result<T, VmError>,
    ) -> Result<T, VmError> {
        let saved = self.enter_run();
        let result = body(self);
        self.leave_run(saved, result)
    }

    /// The scheduled events. The events of objects unregistered since the last look are dropped first,
    /// which is why this needs `&mut self`.
    pub fn events(&mut self) -> &EventQueue {
        self.drop_events_of_unregistered();
        &self.events
    }

    /// Sets how many events can be pending and how many one [`advance_time`](Self::advance_time) may fire
    /// (the defaults are [`EventLimits::default`]).
    pub fn set_event_limits(&mut self, limits: EventLimits) {
        self.event_limits = limits;
        self.events
            .set_max_pending(limits.pending, limits.pending_bytes);
    }

    /// Drops the pending events of objects unregistered since the last look (TGE's
    /// `unregisterObject` cancels them at once; the object table only records the ids). The dropping is
    /// charged to the step budget.
    #[inline(never)]
    pub fn drop_events_of_unregistered(&mut self) {
        for id in self.objects.take_unregistered() {
            let dropped = self.events.drop_dest_seqs(id);
            self.objects.add_work(dropped.len() as u64);
            self.alloc_events_dropped(&dropped);
        }
    }

    /// Queues a call `delay` ms from now (`time = now + (U32)delay`: a negative delay wraps, so at a clock
    /// near 0 it lies far in the future and never fires, and once the clock is past it, it is
    /// already due) and returns its id, or 0 with a warning when the queue is full. The arguments are
    /// charged to the step budget by size.
    fn post_call(&mut self, delay: i32, dest: ObjId, argv: Vec<Vec<u8>>, on_object: bool) -> u32 {
        let bytes: usize = argv.iter().map(Vec::len).sum();
        self.objects.add_work(bytes as u64);
        self.drop_events_of_unregistered();
        let time = self.events.now().wrapping_add(delay as u32);
        match self
            .events
            .post(time, dest, EventKind::Call { argv, on_object })
        {
            Some(seq) => seq,
            None => {
                self.warn(format!(
                    "schedule: the event queue is full ({} events or {} bytes of arguments at most)",
                    self.event_limits.pending, self.event_limits.pending_bytes
                ));
                0
            }
        }
    }

    /// `schedule(time, refobject, command, args...)`: a global call; the object (by id or name) only
    /// bounds the event's life. An object that does not exist gives 0 and no event, unless the text starts
    /// with '0' (then the event belongs to RootGroup).
    ///
    /// With `counted` (`schedule`) the allocation policy may refuse with -1 before anything else, and it is told of the
    /// event until it fires or is cancelled.
    pub fn schedule_global_with(&mut self, c: &NativeCall, counted: bool) -> u32 {
        if counted && self.alloc_refuses_schedule() {
            return u32::MAX;
        }
        let target = arg(c, 2);
        let dest = match self.objects.find(target) {
            Some(id) => id,
            None if target.first() == Some(&b'0') => ROOT_GROUP_ID,
            None => return 0,
        };
        let argv = c.argv.get(3..).unwrap_or_default().to_vec();
        let seq = self.post_call(atoi(arg(c, 1)), dest, argv, false);
        if counted && seq != 0 {
            self.alloc_schedule_taken(seq);
        }
        seq
    }

    /// `%obj.schedule(time, command, args...)`: a method call on the object when it fires. `counted` as for
    /// [`schedule_global_with`](Self::schedule_global_with).
    pub fn schedule_method_with(&mut self, c: &NativeCall, counted: bool) -> u32 {
        if counted && self.alloc_refuses_schedule() {
            return u32::MAX;
        }
        // The registered-object filter is a guard, not a decision: `this_object` always names a
        // registered object here (a held method's object, a deferred-delete object still in the table, an
        // `onRemove`'s removing object), so no test can reach its other branch.
        let Some(object) = c.this_object.filter(|id| self.objects.contains(*id)) else {
            return 0;
        };
        let argv = c.argv.get(3..).unwrap_or_default().to_vec();
        let seq = self.post_call(atoi(arg(c, 2)), object, argv, true);
        if counted && seq != 0 {
            self.alloc_schedule_taken(seq);
        }
        seq
    }

    /// `cancel(id)`: an unknown id is silent; the runtime's own events cannot be cancelled. It does not drop the
    /// events of unregistered objects first: all it does is take an event out, and the events that drop
    /// would remove are gone either way (the runtime's delete events belong to RootGroup, which stays).
    fn cancel_event(&mut self, seq: u32) {
        match self.events.get(seq) {
            Some(event) if !event.from_script() => {
                let line = self.messages().cancel_not_scheduled(seq as i32);
                self.error(line);
            }
            Some(_) => {
                self.events.remove(seq);
                self.alloc_event_gone(seq);
            }
            None => {}
        }
    }

    /// Moves the simulation clock `delta_ms` forward and fires every event due by then, in time order
    /// (first in first out among equal times), including events those events schedule for times not after
    /// the target. The clock reads the TARGET time while they run, and an event whose
    /// object is gone does not fire. A missing function is reported as `Vm::invoke` reports it, a missing method is
    /// silent. Each event is an outermost
    /// run with its own step budget (call this between runs, from the host, not from a native); the
    /// errors of failing events are returned, the others still run.
    ///
    /// The clock and the event times are `U32` milliseconds that wrap like TGE's (49.7 days); the
    /// comparison with the target is a plain unsigned one, so an advance that wraps the clock fires only the
    /// events whose time is not after the wrapped target.
    ///
    /// A host may slice passes by real time; this stops one
    /// after [`EventLimits`]: events (the per-call and the per-event-time count, so that a call that
    /// schedules itself again with delay 0, or with a negative delay once the clock is past it, cannot
    /// keep this call going) and the steps all of them used together (each event has a step budget of its
    /// own, so the counts alone allow a million of them a whole budget each). Only the events that can run
    /// script count (a script's calls and the host's native events); the runtime's delete events do nothing and are
    /// free. The remaining due events stay pending, the clock still moves to the target, and [`VmError::EventLimit`] is
    /// the last error returned; the next call goes on with them.
    ///
    /// **The time slice** ([`EventLimits::per_pass_steps`], None by default): a pass fires the due
    /// events in batches of [`Limits::pass_batch`](crate::policy::Limits::pass_batch), every event it takes counting,
    /// and after each batch stops once its events have used more than the slice's steps; it then sets the clock to the
    /// target all the same, and the events left (due, so now in the past) fire first in the next pass, with the clock
    /// reading that pass's target. An event scheduled for "now" during a pass is due at the target and fires in the
    /// same pass unless the pass stops first. Steps, not milliseconds: deterministic. Every event taken counts in the
    /// batch (the runtime's delete events too; the events of objects already gone are dropped before they are reached
    /// and do not). Stopping so is not an error ([`Advance::sliced`]).
    pub fn advance_time(&mut self, delta_ms: u32) -> Vec<VmError> {
        self.advance_time_counted(delta_ms).errors
    }

    /// [`advance_time`](Self::advance_time) with the steps the events it fired used (the quantity its
    /// `per_advance_steps` total is made of): what a host that drives the clock needs to bound the work of a
    /// whole run of ticks, since each tick is allowed up to four step limits. Only the events that can run script count (a
    /// script's calls and the host's native events; the runtime's delete events do nothing and cost none); each of them
    /// costs the steps its run took, with the work of the native it ended with (`export`, `strreplace`: no instruction
    /// runs after them to charge it, so the run's end bills it), and at least one step, whatever the run before it left
    /// behind: an event whose function is gone is not free, so a storm of them meets a total too. Zero when the host
    /// set no step limit (steps are not counted then).
    ///
    /// Called from inside a run (a host's native that ticks the clock from a script), the events are nested
    /// runs of that one: they share its step budget and count on from where it stood, and the work of a native
    /// they end with is billed by the outer run's next instruction, not to the event.
    pub fn advance_time_counted(&mut self, delta_ms: u32) -> Advance {
        let target = self.events.now().wrapping_add(delta_ms);
        // The clock reads the target while events run.
        self.events.set_now(target);
        let EventLimits {
            per_advance,
            per_time,
            per_advance_steps,
            per_pass_steps,
            per_pass_wall,
            ..
        } = self.event_limits;
        let started = per_pass_wall.map(|_| web_time::Instant::now());
        // Without a step limit no steps are counted, so no slice either.
        let slice = per_pass_steps.filter(|_| self.step_limit.is_some());
        let step_budget =
            per_advance_steps.or(self.step_limit.map(|limit| limit.saturating_mul(4)));
        let mut errors = Vec::new();
        let mut fired = 0usize;
        let mut steps = 0u64;
        let mut per_time_count: HashMap<u32, usize> = HashMap::new();
        let mut in_batch = 0usize;
        let mut sliced = false;
        loop {
            self.drop_events_of_unregistered();
            let Some(time) = self.events.next_due(target) else {
                break;
            };
            // The runtime's own delete events do nothing here and are not counted: only the events that can
            // run script (a script's calls, the host's native events) can keep the call going.
            let is_call = self.events.next_runs_script();
            if is_call {
                let at_time = per_time_count.entry(time).or_insert(0);
                if fired >= per_advance
                    || *at_time >= per_time
                    || step_budget.is_some_and(|budget| steps >= budget)
                {
                    errors.push(VmError::EventLimit);
                    break;
                }
                fired += 1;
                *at_time += 1;
            }
            let Some(event) = self.events.take_due(target) else {
                break;
            };
            // A run that never starts (a missing function, a gone object) leaves the count it found, so the
            // event's cost is what the count grew by: from 0 for the outermost run an event normally is (the
            // count is cleared first), from where it stood when a native ticks the clock from inside a run (the
            // events are then nested runs of that one, which keeps counting: resetting would let a loop that
            // ticks escape its step limit).
            let before = if self.running == 0 {
                self.steps = 0;
                0
            } else {
                self.steps
            };
            let seq = event.seq;
            let room = PassRoom {
                started,
                wall: per_pass_wall,
                slice,
                budget: step_budget,
                steps,
            };
            let overridden = self.try_override(&event, &room);
            if let Some(run) = &overridden {
                errors.extend(run.errors.iter().cloned());
                self.alloc_event_gone(seq);
                let used = if self.step_limit.is_some() {
                    run.steps.max(1)
                } else {
                    run.steps
                };
                steps = steps.saturating_add(used);
            } else {
                if let Err(e) = self.fire_event(event) {
                    errors.push(e);
                }
                // The event is destroyed after it ran: the allocation policy is told.
                self.alloc_event_gone(seq);
            }
            if is_call && overridden.is_none() {
                // The run just ended leaves its step count behind, with the work of its last native (0 when no
                // step limit is set). Every fired call costs at least a step, so that a storm of events that
                // run nothing still meets a total.
                let used = self.steps.saturating_sub(before);
                let used = if self.step_limit.is_some() {
                    used.max(1)
                } else {
                    used
                };
                steps = steps.saturating_add(used);
            }
            // The clock is looked at after every `pass_batch` events taken.
            in_batch += 1;
            if in_batch >= self.limits().pass_batch {
                in_batch = 0;
                let over_wall = started
                    .zip(per_pass_wall)
                    .is_some_and(|(t, wall)| t.elapsed() > wall);
                if slice.is_some_and(|slice| steps > slice) || over_wall {
                    self.drop_events_of_unregistered();
                    sliced = self.events.next_due(target).is_some();
                    break;
                }
            }
        }
        Advance {
            errors,
            steps,
            sliced,
        }
    }

    /// A native event: the host's handler runs it as a run of its own (an object that is gone, or no handler, does nothing).
    #[inline(never)]
    fn fire_native(&mut self, event: NativeEvent) -> Result<(), VmError> {
        if !self.objects.contains(event.dest) {
            return Ok(());
        }
        let Some(handler) = self.native_events.handler.clone() else {
            return Ok(());
        };
        self.in_run(|vm| handler(vm, event))
    }

    /// Runs one event as an outermost run (or nothing: a delete event, an object that is gone, a missing
    /// function or method). Out of line: `advance_time` holds the loop's state, this holds the argument
    /// vectors.
    #[inline(never)]
    fn fire_event(&mut self, event: Event) -> Result<(), VmError> {
        let (argv, on_object) = match event.kind {
            EventKind::Call { argv, on_object } => (argv, on_object),
            EventKind::Native { tag, arg } => {
                return self.fire_native(NativeEvent {
                    tag,
                    arg,
                    dest: event.dest,
                });
            }
            EventKind::Delete => return Ok(()),
        };
        if !self.objects.contains(event.dest) {
            return Ok(());
        }
        let Some((name, args)) = argv.split_first() else {
            return Ok(());
        };
        let started = self.profile.is_some().then(web_time::Instant::now);
        let result = if on_object {
            self.call_method(event.dest, name, args).map(|_| ())
        } else {
            self.invoke(None, name, args).map(|_| ())
        };
        if let Some(t) = started {
            self.profile_schedule(name, on_object, t.elapsed());
        }
        result
    }
}
