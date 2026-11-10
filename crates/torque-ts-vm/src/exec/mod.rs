//! The executor seam: the opcode loop of a script dialect, behind one interface.
//!
//! A [`Vm`] owns everything a script can see that is not the loop itself: variables, namespaces and packages,
//! objects and sets, datablocks, the event queue, tagged strings, console types, natives, the log, the step and
//! depth budgets, file loading. The loop that runs a code block, and whatever evaluation state it keeps between
//! instructions, is an [`Executor`] chosen when the `Vm` is made ([`Vm::with_executor`]); a `Vm` runs one
//! dialect. The runtime calls the executor where script runs from Rust (`invoke`, `call_method`, events, `exec`,
//! `eval`); the executor calls back into the `Vm` through the public methods of this module's `impl Vm`
//! (frames, variables, calls, object creation, fields, the budgets, the log).
//!
//! Two executors come with the runtime: [`torque3d`] runs Torque3D's instruction set, [`classic`] TGE 1.4's (with its
//! compiler binding, string stack and the rules another build of it can change).
//!
//! **State.** An executor keeps its evaluation state itself (one executor per `Vm`, shared by nested runs: a
//! native the loop calls can run script again through the same executor). It must not hold a borrow of that
//! state across a call into the `Vm` that can run script or print.

pub mod classic;
pub mod torque3d;

use std::any::Any;
use std::rc::Rc;

use crate::code::{CodeBlock, DsoFormat};
use crate::ns::{Callable, EntryRef, Holder};
use crate::sim::ObjId;
use crate::vm::{NativeCall, Vm, VmError};

/// What an executor keeps of the state of a run started from Rust ([`Executor::save`]), which it reads back in
/// [`Executor::restore`]: a plain value, so that saving one costs no allocation. Each executor has its own variant;
/// one that has nothing to put back uses `Nothing`.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub enum RunMark {
    #[default]
    Nothing,
    Classic(classic::ClassicMark),
    Torque3d(torque3d::Torque3dMark),
}

/// The opcode loop of one script dialect (see the module documentation).
pub trait Executor: Any {
    /// Runs `block` from code index `start`: a script function's body when `call` is given (its arguments and
    /// the entry it was called through), else a block's top level. Runs nested script calls itself.
    fn run(
        &self,
        vm: &mut Vm,
        block: &Rc<CodeBlock>,
        start: usize,
        call: Option<ScriptCall>,
        mode: RunMode,
    ) -> Result<(), VmError>;

    /// The text the last finished run left as its result (what `invoke` and `eval` return).
    fn result(&self) -> Vec<u8>;

    /// Puts `text` where the next result is read from: what the runtime leaves there itself (a call that
    /// warned and ran nothing leaves "", an object's `onRemove` callback is handed the object's id).
    fn set_result(&self, text: &[u8]);

    /// What a run started from Rust must be able to put back (see [`restore`](Self::restore)). It must not allocate
    /// per call: every call from the host into script takes one.
    fn save(&self) -> RunMark;

    /// The run started at `saved` has ended: `unwind` after an error (the evaluation state goes back to what the
    /// run found, so a native that swallows the error leaves the surrounding expression intact).
    fn restore(&self, saved: RunMark, unwind: bool);

    /// Work units the executor did since the last call (bytes its evaluation state wrote), for the step budget.
    fn take_work(&self) -> u64;

    /// Whether a value was cut at the executor's size cap since the last call.
    fn take_overflow(&self) -> bool;

    /// The compiled-file format of the executor's dialect: what `exec` loads and what its compiler writes.
    /// Torque3D's unless the executor says otherwise.
    fn format(&self) -> &dyn DsoFormat {
        crate::code::torque3d_format()
    }
}

/// A running script function: its arguments, the entry it was called through (whose namespace says where
/// `Parent::` looks) and where that entry's layer sat in the chain when the call was made. The entry is only
/// borrowed briefly to read its namespace, never while script code runs: the running code may redefine its
/// own function.
pub struct ScriptCall {
    pub argv: Vec<Vec<u8>>,
    pub entry: EntryRef,
    pub holder: Holder,
}

/// How a code block runs (the `noCalls` argument of TGE's `CodeBlock::exec` and a "trusted" flag for the
/// top level of a compiled file, see [`Vm::may_declare`]), and whether it gets a
/// local-variable frame of its own. One byte of flags: it is passed through every level of script recursion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunMode(u8);

impl RunMode {
    /// A function body, or evaluated text: no flags.
    pub const PLAIN: RunMode = RunMode(0);

    /// `exec(file, true)`: function declarations are skipped, `new` fails, calls give "" silently.
    const NO_CALLS: u8 = 1;
    /// The top level of a compiled file run by `exec`: the declare policy may allow names it refuses elsewhere
    /// (see [`Vm::may_declare`]).
    const TRUSTED: u8 = 2;
    /// A fresh frame for local variables. Script functions always get one; `exec` and `eval` run in the
    /// frame of whoever called them (eval reads and creates the caller's locals), and
    /// [`Vm::run`] gives the host's code one.
    const OWN_FRAME: u8 = 4;

    /// The top level of a file run by `exec` or of evaluated text.
    #[inline]
    pub fn top_level(no_calls: bool) -> RunMode {
        RunMode(u8::from(no_calls) * RunMode::NO_CALLS)
    }

    /// The same, trusted (a compiled file's top level).
    #[inline]
    pub fn trusted(self) -> RunMode {
        RunMode(self.0 | RunMode::TRUSTED)
    }

    /// The host's own run: it gets a local frame.
    #[inline]
    pub fn own_frame() -> RunMode {
        RunMode(RunMode::OWN_FRAME)
    }

    /// The same, with a local frame of its own (Torque3D runs every block in a frame of its own).
    #[inline]
    pub fn with_own_frame(self) -> RunMode {
        RunMode(self.0 | RunMode::OWN_FRAME)
    }

    #[inline]
    pub fn is_no_calls(self) -> bool {
        self.0 & RunMode::NO_CALLS != 0
    }

    #[inline]
    pub fn is_trusted(self) -> bool {
        self.0 & RunMode::TRUSTED != 0
    }

    #[inline]
    pub fn has_own_frame(self) -> bool {
        self.0 & RunMode::OWN_FRAME != 0
    }
}

/// The code block and offset of a script function (the entry is only borrowed for the copy); None for a
/// native.
#[inline(never)]
pub fn script_target(entry: &EntryRef) -> Option<(Rc<CodeBlock>, usize)> {
    match &entry.borrow().callable {
        Callable::Script { block, offset } => Some((Rc::clone(block), *offset)),
        Callable::Native { .. } => None,
    }
}

/// Whether `argc` (the function name counts as the first) fits a native's limits; `0` means no limit.
#[inline]
pub fn argc_ok(argc: usize, min: i32, max: i32) -> bool {
    let argc = i32::try_from(argc).unwrap_or(i32::MAX);
    !((min != 0 && argc < min) || (max != 0 && argc > max))
}

/// The declared name of a script function (the `FuncDecl`'s name operand); None for a native. TGE
/// prints the frame's `fnName`, the call site's first-interned spelling; this prints the declaration's spelling.
#[inline(never)]
pub fn script_name(entry: &EntryRef) -> Option<Vec<u8>> {
    script_name_ref(&entry.borrow().callable).map(<[u8]>::to_vec)
}

/// [`script_name`] without a copy.
pub fn script_name_ref(callable: &Callable) -> Option<&[u8]> {
    match callable {
        Callable::Script { block, offset } => block.code.get(*offset).and_then(|w| block.ident(*w)),
        Callable::Native { .. } => None,
    }
}

/// The code word at `ip`, or `CodeOutOfRange`. Inlined into the loops; the error is made out of line.
#[inline]
pub fn fetch(code: &[u32], ip: usize) -> Result<u32, VmError> {
    match code.get(ip) {
        Some(word) => Ok(*word),
        None => Err(out_of_range(ip)),
    }
}

#[cold]
#[inline(never)]
fn out_of_range(ip: usize) -> VmError {
    VmError::CodeOutOfRange { ip }
}

/// What an executor reads of a code block besides its code and identifiers.
impl CodeBlock {
    /// The code words (as [`code`](Self::code), inlined into the executor's loop).
    #[inline]
    pub fn words(&self) -> &[u32] {
        &self.code
    }

    /// The string table of the function bodies (`function`) or of the top level.
    #[inline]
    pub fn strings(&self, function: bool) -> &[u8] {
        if function {
            &self.function_strings
        } else {
            &self.global_strings
        }
    }

    /// The float table of the function bodies (`function`) or of the top level.
    #[inline]
    pub fn floats(&self, function: bool) -> &[f64] {
        if function {
            &self.function_floats
        } else {
            &self.global_floats
        }
    }

    /// The entry the plain call site at `ip` is bound to, when the binding was made under `epoch`
    /// ([`Vm::binding_epoch`]).
    #[inline]
    pub fn bound_site(&self, ip: usize, epoch: u64) -> Option<EntryRef> {
        self.cached_site(ip, epoch)
    }

    /// Binds the plain call site at `ip` to `entry` under `epoch`.
    #[inline]
    pub fn bind_site(&self, ip: usize, epoch: u64, entry: &EntryRef) {
        self.cache_site(ip, epoch, entry);
    }

    /// The text the tagged literal at `offset` became in the `Vm` `vm_id`, once it ran there.
    #[inline]
    pub fn tag_conversion(&self, function: bool, offset: u32, vm_id: u64) -> Option<Vec<u8>> {
        self.converted_tag(function, offset, vm_id)
    }

    /// Records what the tagged literal at `offset` became in the `Vm` `vm_id`.
    #[inline]
    pub fn set_tag_conversion(&self, function: bool, offset: u32, vm_id: u64, text: Vec<u8>) {
        self.convert_tag(function, offset, vm_id, text);
    }
}

/// What a native is told about a call made from a script of `block`.
#[inline(never)]
pub fn native_call(
    block: &CodeBlock,
    name: &[u8],
    argv: Vec<Vec<u8>>,
    entry: &EntryRef,
    this_object: Option<ObjId>,
) -> NativeCall {
    NativeCall {
        argv,
        this_object,
        from_bytecode: true,
        name: name.to_vec(),
        namespace: Rc::clone(&entry.borrow().namespace),
        file: block.name_shared(),
    }
}

/// An executor that runs nothing (`Vm::with_executor(Rc::new(NoExecutor))`): every run fails with
/// [`VmError::NoExecutor`]. Natives still run through [`Vm::invoke`].
pub struct NoExecutor;

impl Executor for NoExecutor {
    fn run(
        &self,
        _vm: &mut Vm,
        _block: &Rc<CodeBlock>,
        _start: usize,
        _call: Option<ScriptCall>,
        _mode: RunMode,
    ) -> Result<(), VmError> {
        Err(VmError::NoExecutor)
    }

    fn result(&self) -> Vec<u8> {
        Vec::new()
    }

    fn set_result(&self, _text: &[u8]) {}

    fn save(&self) -> RunMark {
        RunMark::Nothing
    }

    fn restore(&self, _saved: RunMark, _unwind: bool) {}

    fn take_work(&self) -> u64 {
        0
    }

    fn take_overflow(&self) -> bool {
        false
    }
}
