//! The executor interface: what an [`Executor`] reads and changes of the `Vm` it runs on (see [`crate::exec`]).
//! Frames, calls, object creation and fields, the budgets and the log are in the modules they belong to; this
//! is the plain state an opcode loop needs besides them.

use std::any::Any;
use std::rc::Rc;

use super::{MAX_CALL_DEPTH, Vm, VmError, WORK_PER_STEP};
use crate::exec::Executor;
use crate::ns::Namespaces;
use crate::sim::ObjId;
use crate::vars::Dictionary;

impl Vm {
    pub(crate) fn policies(&self) -> &crate::policy::Policies {
        &self.policies
    }

    pub(crate) fn policies_mut(&mut self) -> &mut crate::policy::Policies {
        &mut self.policies
    }

    /// The executor this `Vm` runs scripts with.
    #[inline]
    pub fn executor(&self) -> &Rc<dyn Executor> {
        &self.executor
    }

    /// The executor as its own type, when it is a `T`.
    pub fn executor_as<T: Executor>(&self) -> Option<&T> {
        let any: &dyn Any = &*self.executor;
        any.downcast_ref::<T>()
    }

    /// `CallDepth` when a block starting now would pass the call-depth budget (the top-level script is level 0;
    /// `MAX_CALL_DEPTH` nested calls beyond it are allowed).
    #[inline]
    pub fn check_call_depth(&self) -> Result<(), VmError> {
        if self.depth > MAX_CALL_DEPTH {
            return Err(VmError::CallDepth);
        }
        Ok(())
    }

    /// Counts one instruction and the work done since the last one (the objects', the namespaces' and
    /// `executor_work`, the executor's own) against the step budget.
    #[inline]
    pub fn charge_instruction(&mut self, executor_work: u64) -> Result<(), VmError> {
        let work = self
            .objects
            .take_work()
            .saturating_add(self.namespaces.take_work())
            .saturating_add(executor_work);
        if let Some(limit) = self.step_limit {
            self.steps = self
                .steps
                .saturating_add(1)
                .saturating_add(work / WORK_PER_STEP);
            if self.steps > limit {
                return Err(VmError::StepLimit);
            }
        }
        if self.objects.memory().take_refused() {
            return Err(VmError::MemoryLimit);
        }
        Ok(())
    }

    /// The global variables.
    #[inline]
    pub fn global_table(&self) -> &Dictionary {
        &self.globals
    }

    /// The global variables, to change.
    #[inline]
    pub fn global_table_mut(&mut self) -> &mut Dictionary {
        &mut self.globals
    }

    /// The local-variable frames of the running blocks that have one, outermost first.
    #[inline]
    pub fn local_frames(&self) -> &[Dictionary] {
        &self.frames
    }

    /// The local-variable frames, to change.
    #[inline]
    pub fn local_frames_mut(&mut self) -> &mut [Dictionary] {
        &mut self.frames
    }

    /// The object of the method that is running (TGE's `thisObject`).
    #[inline]
    pub fn this_object(&self) -> Option<ObjId> {
        self.this_object
    }

    /// Sets the object of the method that is running; returns the one before.
    #[inline]
    pub fn set_this_object(&mut self, object: Option<ObjId>) -> Option<ObjId> {
        std::mem::replace(&mut self.this_object, object)
    }

    /// The namespaces and packages.
    #[inline]
    pub fn namespaces(&self) -> &Namespaces {
        &self.namespaces
    }

    /// The namespaces and packages, to change.
    #[inline]
    pub fn namespaces_mut(&mut self) -> &mut Namespaces {
        &mut self.namespaces
    }

    /// The stamp of this `Vm`'s call-site bindings ([`CodeBlock::bound_site`](crate::code::CodeBlock::bound_site)).
    #[inline]
    pub fn binding_epoch(&self) -> u64 {
        self.binding_epoch
    }

    /// Makes every call-site binding made so far stale: each resolved call site of every loaded block binds
    /// again on its next run (tagged literals keep their conversions).
    pub fn reset_call_bindings(&mut self) {
        self.binding_epoch = super::NEXT_VM_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// The tick counter, the wall clock and the time zone the natives read.
    pub fn clock(&self) -> &dyn crate::host::Clock {
        &*self.clock
    }

    /// The installed file rules.
    pub fn file_rules(&self) -> &Rc<dyn crate::policy::FileRules> {
        &self.policies.files
    }

    /// What tells this `Vm` apart from others (tagged-literal conversions are stamped with it).
    #[inline]
    pub fn vm_id(&self) -> u64 {
        self.id
    }

    /// Whether a script may declare the function `namespace::name`; `trusted` for the top level of a compiled file
    /// run by `exec`.
    #[inline]
    pub fn may_declare(&self, namespace: &[u8], name: &[u8], trusted: bool) -> bool {
        self.policies.declare.may_declare(namespace, name, trusted)
    }

    /// The allocation policy's hooks (see [`AllocPolicy`](crate::policy::AllocPolicy)): each runs with the policy's
    /// `Rc` cloned out, so the policy may run script. The frame hooks are out of line: they are on the call path of
    /// every function.
    #[inline(never)]
    pub(crate) fn alloc_frame_entered(&mut self) {
        if let Some(policy) = self.policies.alloc.clone() {
            policy.frame_entered(self);
        }
    }

    #[inline(never)]
    pub(crate) fn alloc_frame_left(&mut self) {
        if let Some(policy) = self.policies.alloc.clone() {
            policy.frame_left(self);
        }
    }

    pub(crate) fn alloc_admit(&mut self, id: ObjId) -> bool {
        match self.policies.alloc.clone() {
            Some(policy) => policy.admit(self, id),
            None => true,
        }
    }

    pub(crate) fn alloc_released(&mut self, id: ObjId) {
        if let Some(policy) = self.policies.alloc.clone() {
            policy.released(self, id);
        }
    }

    pub(crate) fn alloc_refuses_schedule(&mut self) -> bool {
        match self.policies.alloc.clone() {
            Some(policy) => policy.refuses_schedule(self),
            None => false,
        }
    }

    pub(crate) fn alloc_schedule_taken(&mut self, seq: u32) {
        if let Some(policy) = self.policies.alloc.clone() {
            policy.schedule_taken(self, seq);
        }
    }

    pub(crate) fn alloc_event_gone(&mut self, seq: u32) {
        if let Some(policy) = self.policies.alloc.clone() {
            policy.event_gone(self, seq);
        }
    }

    pub(crate) fn alloc_events_dropped(&mut self, seqs: &[u32]) {
        if let Some(policy) = self.policies.alloc.clone() {
            policy.events_dropped(self, seqs);
        }
    }

    /// Deletes `id` now, or marks it to be deleted when the method that holds it returns (`deleteObject`).
    pub fn delete_or_defer(&mut self, id: ObjId) -> Result<(), VmError> {
        if self.objects.defer_delete(id) {
            return Ok(());
        }
        self.delete_now(id, None)
    }

    /// Takes the event `seq` out of the queue without running it; true when it was there.
    pub fn remove_event(&mut self, seq: u32) -> bool {
        self.events.remove(seq).is_some()
    }
}
