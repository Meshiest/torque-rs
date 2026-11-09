//! The runtime: variables, namespaces and packages, objects, natives, the log, the budgets and the entry points
//! that run script through the [`Executor`].
//!
//! Follows Torque3D's MIT-licensed `console/console.cpp` (Copyright (c) 2012 GarageGames, LLC),
//! `console/consoleInternal.cpp` (Copyright (c) 2012 GarageGames, LLC) and `console/torquescript/compiledEval.cpp`
//! (Copyright (c) 2013 GarageGames, LLC; Copyright (c) 2015 Faust Logic, Inc.; Copyright (c) 2021 TGEMIT Authors &
//! Contributors); the code here is this crate's own.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::code::CodeBlock;
use crate::events::{EventLimits, EventQueue};
pub(crate) use crate::exec::RunMode as Mode;
use crate::exec::{Executor, RunMark, ScriptCall, argc_ok, script_name_ref, script_target};
use crate::host::{Clock, Compiler, FileHost, NoCompiler, NoFiles, SystemClock};
use crate::memory::{DEFAULT_SCRIPT_BYTES, MEMORY_LIMIT_PREF};
use crate::ns::{Activation, Callable, EntryRef, Holder, Namespaces};
use crate::sim::{ClassId, ObjId, Objects, SimObject};
use crate::tags::TagTable;
use crate::text::until_nul;
use crate::vars::{Dictionary, Entry};

mod console;
mod file_natives;
mod files;
pub use files::MAX_DSO_BYTES;
pub use files::{EVAL_ERROR_TEXT_BYTES, EvalReport};
mod interface;
mod lifecycle;
pub use lifecycle::{DELETE_DATABLOCKS_USAGE, HostBudgetSaved, QUIT_USAGE};
mod math;
pub use math::{INITIAL_RANDOM_SEED, ftoi};
mod missing;
pub use missing::{count_called, missing_functions};
mod objops;
pub use objops::{AddedObject, Declaration, FieldTarget, SuspendedObject};
pub mod profile;
mod schedule;
pub use schedule::{Advance, CallOverride, NativeEvent, NativeEventHandler, OverrideRun, PassRoom};
mod sim_natives;
mod statics;
mod strings;
mod tag_natives;
pub mod textfns;
mod variables;
pub(crate) mod vectors;
pub mod words;

use objops::Creating;

/// Hands out a fresh id to every `Vm`.
pub(crate) static NEXT_VM_ID: AtomicU64 = AtomicU64::new(1);

pub const MAX_STACK: usize = 1024;
/// The longest string the runtime builds (64 MiB): the natives that can grow a result several times stop at it.
pub const MAX_STRING_BYTES: usize = 64 << 20;
/// The global variable that holds the name of the file that is executing (`$Con::File`).
const CON_FILE: &[u8] = b"$Con::File";
pub const MAX_CALL_DEPTH: usize = 256;
/// Extra call-depth units charged when Rust runs script code inside a run (`invoke`, `run` from a
/// native, `call_method`: the script `onAdd` of a new object): the Rust frames between two script
/// levels (the call, the native and its closure, `invoke`) are about as large as the interpreter's own,
/// so such a level costs two units and recursion through `call` or `onAdd` stops at about 128 levels
/// (see the [`Vm`] documentation).
pub const REENTRY_COST: usize = 1;
/// Work units (see `sim` and `ns`) that cost one extra step. Loops whose length depends on what a
/// script has built count their iterations; each instruction is charged `1 + work / WORK_PER_STEP`
/// steps, so the step limit bounds run time however large the objects get. Work on a stored value is
/// charged by its bytes: a field read copies or formats it (`read_cur_field`), a `: Parent` copy clones the
/// parent's dynamic fields (`copy_parent`), `dump` builds a line for each field; each asks the step budget
/// (`Vm::ensure_budget`) as it goes, a copy before it copies, so what overshoots a limit is one line, one
/// chunk of a list or one refused copy, never a whole object. A method lookup is charged the length of the
/// namespace name it hashes (an object named by a script looks its methods up under its own name), and a
/// validator's warning the part of the object's name it quotes.
pub const WORK_PER_STEP: u64 = 64;

/// The bytes of the preload error buffer that hold text (a 256-byte static array, one for the NUL).
pub const PRELOAD_ERROR_BYTES: usize = 255;

/// The steps one compile costs before anything is scanned: a compiler may start a thread with a large stack for every
/// compile (`exec` makes two, `eval`, `compile()` and `Vm::compile_exec` one), which takes tens of microseconds whatever
/// the text, against a few hundredths of one for a plain step in a release build. Without the charge a loop of
/// `eval("");` runs about 180 times slower per step than a plain loop under the same step limit; with it, at about the
/// same speed. 2,048 is that parity (about 2,300 steps in a release build) rounded down to a power of two.
pub const COMPILE_BASE_STEPS: u64 = 2_048;
/// [`COMPILE_BASE_STEPS`] in work units (`compile_source` adds it to every compile).
pub const COMPILE_BASE_WORK: u64 = COMPILE_BASE_STEPS * WORK_PER_STEP;
/// The longest console line, in bytes: `Con::_printf`'s 4096-byte buffer less its terminator.
pub const MAX_LOG_LINE: usize = 4095;
/// The most the retained log may hold ([`Vm::log`]): the bytes of its lines and [`LOG_LINE_OVERHEAD`] for each. Past
/// it the oldest lines are dropped and counted ([`Vm::dropped_log_lines`]).
///
/// 64 MiB: that is 16,513 lines of the longest kind (4000 bytes of text, 63 steps of the budget each) or about
/// a million one-byte lines, and a thirtieth of what one long server run could keep without it (a run's budget is
/// billions of steps: about 2 GB of 4000-byte `echo`s).
pub const MAX_LOG_BYTES: usize = 64 << 20;
/// The bytes of function names the tally of missing functions keeps for lines that have left the log (the
/// report of a run that printed without end must not grow without end either: a script can call a new name
/// with every line).
pub const MAX_GONE_MISSING_BYTES: usize = 8 << 20;
/// What a retained log line counts besides its text: its `LogLine`, its allocation and the slot a dropped line
/// leaves until the log is compacted.
pub const LOG_LINE_OVERHEAD: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmError {
    /// The executor met an operation (named as the dialect names it) it does not run.
    Unsupported(&'static str),
    InvalidOpcode {
        ip: usize,
        word: u32,
    },
    CodeOutOfRange {
        ip: usize,
    },
    BadIndex {
        ip: usize,
    },
    StackUnderflow,
    StackOverflow,
    DivideByZero,
    CallDepth,
    StepLimit,
    /// ADD_OBJECT ran while no object was being created. Compiled code reaches this when an inner
    /// declaration in a field value fails (`new A { f = new Typo(); }`: the failed inner `new` leaves
    /// nothing for the outer ADD_OBJECT); TGE dereferences NULL there.
    NoNewObject {
        ip: usize,
    },
    /// A tagged string would need a slot past the tag table's limit ([`crate::tags::MAX_SLOTS`]). TGE
    /// has no limit of its own until its address space is gone; this stops the run instead.
    TagTableFull,
    /// `Vm::advance_time` fired as many events as its [`EventLimits`] allow and left the rest pending (a
    /// script that schedules itself again keeps due events coming).
    EventLimit,
    /// A string passed [`MAX_STRING_BYTES`] (TGE has no limit and
    /// runs out of memory).
    StringTooLong,
    /// The `Vm` has no executor to run script with ([`NoExecutor`](crate::exec::NoExecutor)).
    NoExecutor,
    /// A store would have taken what scripts hold past the memory limit ([`crate::memory`]); it was not made.
    MemoryLimit,
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmError::Unsupported(op) => write!(f, "{op} is not supported yet"),
            VmError::InvalidOpcode { ip, word } => {
                write!(f, "invalid opcode {word:#x} at {ip}")
            }
            VmError::CodeOutOfRange { ip } => write!(f, "code index {ip} is outside the code"),
            VmError::BadIndex { ip } => {
                write!(
                    f,
                    "instruction at {ip} refers to a table entry that does not exist"
                )
            }
            VmError::StackUnderflow => write!(f, "evaluation stack underflow"),
            VmError::StackOverflow => write!(f, "evaluation stack overflow"),
            VmError::DivideByZero => write!(f, "modulo by zero"),
            VmError::CallDepth => write!(f, "call depth limit ({MAX_CALL_DEPTH}) exceeded"),
            VmError::StepLimit => write!(f, "step limit exceeded"),
            VmError::NoNewObject { ip } => write!(
                f,
                "instruction at {ip} adds an object but none is being created"
            ),
            VmError::TagTableFull => write!(f, "the tagged string table is full"),
            VmError::EventLimit => write!(f, "too many events fired in one advance of the clock"),
            VmError::StringTooLong => write!(f, "a string grew past {MAX_STRING_BYTES} bytes"),
            VmError::NoExecutor => write!(f, "this Vm has no executor to run script"),
            VmError::MemoryLimit => write!(f, "scripts would hold more than their memory limit"),
        }
    }
}

impl std::error::Error for VmError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Normal,
    Warning,
    Error,
}

/// The retained log (`Vm::log`): `lines[head..]` are the lines held; the ones before are dropped (their text freed)
/// and go when the vector is compacted ([`MAX_LOG_BYTES`]).
#[derive(Default)]
struct LogKeep {
    lines: Vec<LogLine>,
    head: usize,
    /// What the held lines count against [`MAX_LOG_BYTES`].
    held: usize,
    /// The lines dropped for it, for good ([`Vm::dropped_log_lines`]).
    dropped: u64,
    /// The functions the warnings of lines that left the log (drained or dropped) named, with how often each:
    /// `boot::missing_functions` adds the lines still held. Keys and counts are kept up to [`MAX_GONE_MISSING_BYTES`]
    /// of names; past that a name not in the tally yet is not added.
    gone_missing: std::collections::BTreeMap<String, usize>,
    gone_missing_bytes: usize,
    /// The console log (`consoleLog`, what a GuiConsole shows): every printed line, split at newlines, kept
    /// when a host asks for it ([`Vm::keep_console_history`]), at most [`MAX_HISTORY_LINES`] lines and [`MAX_HISTORY_BYTES`]
    /// (the oldest dropped first; TGE keeps them all).
    history: Option<std::collections::VecDeque<LogLine>>,
    history_bytes: usize,
    /// Where every printed line also goes as it is made ([`Vm::set_console_sink`]: the client's `console.log`).
    sink: Option<ConsoleSink>,
    /// Every line printed so far ([`Vm::lines_printed`]).
    printed: u64,
}

/// A host's receiver of every printed line ([`Vm::set_console_sink`]), with the level and the text as the log keeps it.
pub type ConsoleSink = Box<dyn FnMut(Level, &[u8])>;

/// The most lines and bytes the console history keeps.
pub const MAX_HISTORY_LINES: usize = 1 << 16;
pub const MAX_HISTORY_BYTES: usize = 16 << 20;

/// `$Con::File` (TGE's `Con::gCurrentFile`): for each of the `Vm`'s `blocks`, the block whose name the variable shows
/// while it runs (a named block's own, an unnamed one's the one around it, None outside any named block), and the
/// block the variable holds the name of now (None: there is no such variable, which reads "").
#[derive(Default)]
struct ConFileState {
    blocks: Vec<Option<Rc<CodeBlock>>>,
    shown: Option<Rc<CodeBlock>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub level: Level,
    pub text: Vec<u8>,
}

/// What a native function returns; the kind decides how the result reaches the script (see [`crate::exec`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Native {
    Str(Vec<u8>),
    Int(i32),
    Float(f64),
    Bool(bool),
    Void,
    /// Makes the calling instruction return this error (for natives that call back into the VM).
    Fail(VmError),
}

/// Where the function a name finds was declared ([`Vm::global_function_origin`]).
#[derive(Clone)]
pub struct FunctionOrigin {
    /// The file of its code block (a script function's).
    pub file: Option<Rc<[u8]>>,
    /// A package declared it (it is active now, since the name finds it).
    pub packaged: bool,
    pub native: bool,
    /// A script function's block and the index of its declaration's first operand.
    pub code: Option<(Rc<CodeBlock>, usize)>,
}

/// What a native function is told about the call that invoked it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCall {
    /// `argv[0]` is the called name, then the arguments.
    pub argv: Vec<Vec<u8>>,
    /// The object of the method that is running (TGE's `thisObject`): set by a method call and
    /// inherited by everything called from it, `None` outside any method.
    pub this_object: Option<ObjId>,
    /// Whether script bytecode made the call, as opposed to the host (`invoke`, `call()`, an event). Only a call by
    /// bytecode is handed the static argument array, which later calls overwrite: `eval` prints from it
    /// (`Vm::eval_with`).
    pub from_bytecode: bool,
    /// The function name as written at the call site.
    pub name: Vec<u8>,
    /// The namespace the called entry lives in, lower-cased (empty for the global namespace).
    pub namespace: Rc<[u8]>,
    /// The name of the code block that made the call, if it has one.
    pub file: Option<Rc<[u8]>>,
}

pub type NativeFn = Rc<dyn Fn(&mut Vm, &NativeCall) -> Native>;

/// A host's observer of static-field writes (see [`Vm::set_static_write_observer`]): the object, None while it is still
/// being built, and its class.
pub type StaticWriteFn = Rc<dyn Fn(&mut Vm, Option<ObjId>, ClassId)>;

/// Engine state that is just a flag.
#[derive(Debug, Default)]
struct Quirks {
    /// `gSyntaxError`: set by an `eval` whose text has a syntax error, cleared at the start of each `eval` and by
    /// nothing else. The `eval` that ran a bad text reports only if it is still set after the run.
    syntax_error: bool,
    /// How many times text was evaluated (`eval`, the native or the host's), for hosts that must show none ran.
    evals: u64,
    /// `call("eval", ...)` is about to run the native `eval`: it is handed the static argument array one
    /// slot on, so its report shows the array's second slot. Set by `call` and taken by `eval` at its entry.
    eval_via_call: bool,
    /// The host's observer of static-field writes (see [`Vm::set_static_write_observer`]).
    static_observer: statics::StaticObserver,
    /// The executor's report of an `eval` with a syntax error (see [`Vm::set_eval_report`]).
    eval_report: files::EvalReportSlot,
}

/// The lengths of the per-frame stacks when a block started running (see [`Vm::enter_frame`]).
#[derive(Clone, Copy)]
pub struct FrameMark {
    frames: usize,
    creating: usize,
    traces: usize,
}

/// What a finished run must put back after an error (and the executor's current variable even after a success).
struct RunState {
    exec: RunMark,
}

/// The `Class::method` keys still registered as the class table's stubs (a newtype so that the set stays boxed in `Vm`).
#[derive(Default)]
struct StubKeys(std::collections::BTreeSet<String>);

/// The interpreter state: stacks, variables, the namespaces and packages, and the log.
///
/// `run` (and `invoke`) recurse for script calls: call them on a thread with at least 1 MiB of stack.
/// The call-depth budget is 256 nested script calls (method calls included). Everything that runs
/// script code from Rust (a native that calls `invoke` or `run`, `call_method`, and so the script
/// `onAdd` of an object being created) is charged one more depth unit on top of the script call it
/// makes, because the Rust frames in between are as large as a script level's: recursion through the
/// `call` native, or through nested `onAdd`s, therefore stops with `CallDepth` after about 128 levels
/// (class behaviours cost one more unit, `objops::BEHAVIOUR_COST`, so those stop at about 85; a refused
/// path's BackTrace under a host-replaced `backtrace` costs `files::REFUSAL_BACKTRACE_COST`).
///
/// **Stack use.** In a debug build the deepest path (256 plain levels, or 128 `deleteAll` levels, ending in a
/// BackTrace or a compile of source) uses about 800 KiB of a 1 MiB thread, leaving about 178 KiB of
/// headroom (measured with a native that reads the stack at the bottom of each path). A change that raises one of these
/// paths by more than a couple of KiB needs a look. The frames on these paths are kept small on
/// purpose (`call_function`, `call_native`, `add_object`, `class_on_add`, `preload`, `delete_now`,
/// `class_on_remove`, `delete_all`, `open_frame`, `check_write_path`, `exec_file`, `find_file`,
/// `compile_source`): anything with a large local, and any bookkeeping a frame only needs before or after
/// the script runs, lives in an out-of-line helper that has returned before the script runs. A compiler may
/// run on a thread of its own with a large stack ([`Compiler`]), so that a text of any depth
/// compiles at the bottom of any recursion; a compile then costs about 8 KiB on the `Vm`'s thread.
///
/// **Why scripts recurse on the host stack**: ordinary scripts nest about ten functions deep
/// ([`peak_depth`](Self::peak_depth)), well inside the 256-level budget, and every path that runs script code from
/// Rust is charged as a level, so no script can reach the end of the stack before `CallDepth` stops it. TGE has no
/// depth limit of its own and overflows its stack between 1000 and 2000 levels: a script that recurses between 257
/// and about 1000 levels works there and stops here with `CallDepth`. A host that needs
/// such scripts can run the `Vm` on a bigger thread and raise `MAX_CALL_DEPTH` together with it (about
/// a heap frame and rewrites the evaluation loop's re-entry points (`invoke`, `exec`, `eval`, events).
///
/// The string stack holds at most `strstack::MAX_STRING_BYTES` (64 MiB); every byte it writes and every
/// argument byte a call collects costs one work unit (a step per `WORK_PER_STEP`), so a doubling loop
/// ends at the step limit long before the cap, and without a limit at the cap with
/// `VmError::StringTooLong`. The cap is applied where a result is delivered to the string stack (a native
/// result the string stack would cut fails the run), so `Vm::invoke` from Rust sees a native's result
/// uncut unless the native itself stops at the cap: the natives that can grow a result several times
/// (`strreplace`, `setWord` and friends, `expandEscape`, and a host's text encoders) check it inside
/// their loops and never build more than the cap plus one step of growth.
///
/// Objects live in [`Objects`] (see `sim`); work proportional to their size is charged to the step
/// budget (`WORK_PER_STEP`).
pub struct Vm {
    /// The opcode loop and its evaluation state (see [`crate::exec`]).
    executor: Rc<dyn Executor>,
    globals: Dictionary,
    frames: Vec<Dictionary>,
    /// Emptied local-variable tables of finished frames, for the next frames.
    frame_pool: Vec<Dictionary>,
    namespaces: Namespaces,
    objects: Objects,
    /// The object of the method that is running (TGE's `gEvalState.thisObject`).
    this_object: Option<ObjId>,
    /// What each running frame is building with `new` (one entry per frame, like `frames`).
    creating: Vec<Creating>,
    /// The script functions that are running, outermost first: the entry each was called through and
    /// where its layer sat then (`backtrace`).
    traces: Vec<(EntryRef, Holder)>,
    /// The tagged string table (`NetStringTable`).
    tags: TagTable,
    /// Scheduled events and the simulation clock.
    events: EventQueue,
    /// How many events one `advance_time` may fire.
    event_limits: EventLimits,
    /// The files scripts load (`exec`) and the compiler for source text.
    files: Box<dyn FileHost>,
    compiler: Box<dyn Compiler>,
    /// The tick counter, the wall clock and the time zone (`setRandomSeed()`; later `getRealTime`,
    /// the local date and time and file times).
    clock: Box<dyn Clock>,
    /// The state of the one random generator (TGE's `gRandGen`).
    random_seed: i32,
    /// Where `findNextFile` goes on: the index in the host's file list of the last match (TGE's
    /// `firstMatch`); None after the last one.
    file_search: Option<usize>,
    /// The code blocks that are executing, innermost last (TGE's `CodeBlock::smCurrentCodeBlock` is
    /// the last): `./` and `~/` in file names are relative to its name, and natives are told it as the
    /// calling file.
    blocks: Vec<Rc<CodeBlock>>,
    /// Which file `$Con::File` names (boxed, like the log state below: a `Vm` is held by value in the frames of hosts
    /// and tests, so every byte it grows costs stack, and the stack budget of deep recursion counts in KiB).
    con_file: Box<ConFileState>,
    /// The first spelling of every file name `exec` ran (the string table keeps a name's first
    /// spelling).
    file_names: std::collections::HashMap<Vec<u8>, Vec<u8>>,
    /// The bytes `file_names` holds (keys and values; see `files::MAX_FILE_NAME_BYTES`).
    file_name_bytes: usize,
    /// The rules the host installed ([`crate::policy`]). Boxed: a `Vm` is held by value in the frames of hosts and
    /// tests, and every byte of it is stack.
    policies: Box<crate::policy::Policies>,
    /// The runtime's string table: the first spelling of the text `string` and `filename` fields stored, of
    /// every class name, and of every folder and file name a file native looked up or the host's list holds
    /// (unlike TGE's, which also holds every identifier).
    strings: statics::StringTable,
    /// Calls of the class table's method stubs, by `Class::method` (see `classes::install`).
    stub_calls: std::collections::BTreeMap<String, u64>,
    /// The `Class::method` keys whose registered entry is still the class table's stub (boxed).
    stub_keys: Box<StubKeys>,
    /// A script called `quit()` (see [`quit_requested`](Self::quit_requested)).
    quit_requested: bool,
    /// `delete_datablocks` is deleting (a nested call does nothing).
    deleting_datablocks: bool,
    /// Two runtime flags scripts can see (boxed, like every field added since the stack rows were measured).
    quirks: Box<Quirks>,
    /// The host's handler for the native events of the queue (`vm/schedule.rs`; boxed like every field added since the
    /// stack rows were measured).
    native_events: Box<schedule::NativeEvents>,
    /// The preload `errorBuffer` (one static 256-byte array, never cleared): the text of the last failed
    /// preload ([`PreloadError::Failed`]), at most [`PRELOAD_ERROR_BYTES`] bytes. Boxed like every field added since
    /// the stack rows were measured.
    preload_error: Box<[u8]>,

    /// What the `FileObject`s have open (`classes::file_object`; a table of the `Vm`'s own, so that no borrow of
    /// it can be held across anything that runs script code or prints).
    pub(crate) file_objects: crate::classes::file_object::FileObjects,
    /// What scripts wrote in the current window of the write quota ([`WriteQuota`](crate::policy::WriteQuota)).
    pub(crate) writes: file_natives::WriteWindow,
    /// Tells this `Vm` apart from others: tagged-literal conversions are stamped with it.
    id: u64,
    /// The stamp of this `Vm`'s call-site bindings: `id` at first, a fresh one after the host resets the
    /// call-site bindings ([`Vm::reset_call_bindings`]), which makes every binding made before stale.
    binding_epoch: u64,
    /// The retained log and what is kept of the lines that left it ([`LogKeep`]).
    log: Box<LogKeep>,
    depth: usize,
    /// The deepest `depth` reached so far ([`peak_depth`](Self::peak_depth)).
    peak_depth: usize,
    /// Runs and invokes in progress (nested ones included); the step budget restarts at 0.
    running: usize,
    steps: u64,
    step_limit: Option<u64>,
    /// The budget of host work done outside a run ([`Vm::begin_host_budget`]): a tick's.
    host_budget: lifecycle::HostBudget,
    /// The script profile while a host has it on ([`Vm::set_profiling`]; boxed, off by default).
    profile: Option<Box<profile::Profiler>>,
    /// The "Unknown command" warnings of each source's second in progress ([`MAX_UNKNOWN_COMMAND_LINES`]).
    unknown_commands: std::collections::HashMap<CommandSource, UnknownCommands>,
}

/// Who asked [`Vm::invoke_from`] for a function: [`CommandSource::LOCAL`] for the host itself, else a number the host gives each
/// remote party (a server's connection object on the server, the server on a client).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandSource(pub u64);

impl CommandSource {
    /// The host's own calls ([`Vm::invoke`]).
    pub const LOCAL: CommandSource = CommandSource(0);
    /// Where the sources past [`MAX_COMMAND_SOURCES`] are counted together.
    const OVERFLOW: CommandSource = CommandSource(u64::MAX);
}

/// The most `<name>: Unknown command.` warnings one source's calls print in one second of the simulation's time (a server's
/// remote commands name functions; thousands a tick of ones that do not exist filled the console log in a second). Past it they
/// are counted, and once the second is over (at the next [`Vm::advance_time`], or the source's next unknown command) one line
/// says how many were not printed.
pub const MAX_UNKNOWN_COMMAND_LINES: u32 = 32;
/// The most sources counted apart; the ones past it share one count.
pub const MAX_COMMAND_SOURCES: usize = 1024;

/// The second [`MAX_UNKNOWN_COMMAND_LINES`] counts in for one source: when it started (`sim_time`), the lines printed, the ones
/// left out.
#[derive(Debug, Clone, Copy)]
struct UnknownCommands {
    since: u32,
    printed: u32,
    left_out: u64,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    /// A `Vm` that runs Torque3D's TorqueScript: the [`Torque3dExecutor`](crate::exec::torque3d::Torque3dExecutor),
    /// its compiler and natives ([`torque3d::install`](crate::exec::torque3d::install)). A game with a dialect of its
    /// own makes its `Vm` with [`with_executor`](Self::with_executor).
    ///
    /// A new `Vm` has no step limit: a script that never ends hangs its host, and `charge_work` and `ensure_budget` refuse
    /// nothing. A host that runs scripts it does not trust sets one ([`set_step_limit`](Self::set_step_limit); a
    /// server made with `torque_server::server::Server` has 50 million steps). Its memory limit is [`DEFAULT_SCRIPT_BYTES`].
    pub fn new() -> Vm {
        let mut vm = Vm::with_executor(Rc::new(crate::exec::torque3d::Torque3dExecutor::new()));
        crate::exec::torque3d::install(&mut vm);
        vm
    }

    /// A `Vm` that runs scripts with `executor` (one executor per `Vm`: it keeps the evaluation state of
    /// this `Vm`'s runs).
    pub fn with_executor(executor: Rc<dyn Executor>) -> Vm {
        Vm::with_config(executor, crate::sim::SimConfig::default())
    }

    /// [`with_executor`](Self::with_executor) with the object tables started as `sim` says.
    pub fn with_config(executor: Rc<dyn Executor>, sim: crate::sim::SimConfig) -> Vm {
        let objects = Objects::with_config(sim);
        objects.memory().set_limit(DEFAULT_SCRIPT_BYTES);
        let mut vm = Vm {
            executor,
            globals: Dictionary::with_account(objects.memory().clone()),
            frames: Vec::new(),
            frame_pool: Vec::new(),
            namespaces: Namespaces::default(),
            objects,
            this_object: None,
            creating: Vec::new(),
            traces: Vec::new(),
            tags: TagTable::new(),
            events: EventQueue::new(),
            event_limits: EventLimits::default(),
            files: Box::new(NoFiles),
            compiler: Box::new(NoCompiler),
            clock: Box::new(SystemClock::new()),
            random_seed: math::INITIAL_RANDOM_SEED,
            file_search: None,
            blocks: Vec::new(),
            con_file: Box::default(),
            file_names: std::collections::HashMap::new(),
            file_name_bytes: 0,
            policies: Box::default(),
            strings: statics::StringTable::default(),
            stub_calls: std::collections::BTreeMap::new(),
            stub_keys: Box::default(),
            quit_requested: false,
            deleting_datablocks: false,
            quirks: Box::default(),
            native_events: Box::default(),
            preload_error: Box::default(),
            file_objects: Default::default(),
            writes: Default::default(),
            id: 0,
            binding_epoch: 0,
            log: Box::default(),
            depth: 0,
            peak_depth: 0,
            running: 0,
            steps: 0,
            step_limit: None,
            host_budget: Default::default(),
            profile: None,
            unknown_commands: std::collections::HashMap::new(),
        };
        vm.id = NEXT_VM_ID.fetch_add(1, Ordering::Relaxed);
        vm.binding_epoch = vm.id;
        console::register(&mut vm);
        // `call` runs the function it names: a policy checking the call sees only `call`.
        vm.add_native_effects("", "call", crate::effects::Effects::CODE);
        // Switching packages switches the functions every script sees, as switching a mod does. `$instantGroup` says where
        // every new object goes, and the `$Con::` globals are the console's own settings.
        for name in ["activatePackage", "deactivatePackage"] {
            vm.add_native_effects("", name, crate::effects::Effects::ENGINE);
        }
        vm.add_global_effects("$instantGroup", crate::effects::Effects::ENGINE);
        vm.add_global_effects("$Con::*", crate::effects::Effects::ENGINE);
        // The memory limit of every later run.
        vm.add_global_effects(MEMORY_LIMIT_PREF, crate::effects::Effects::ENGINE);
        vm.register(
            "call",
            2,
            0,
            "call(function, args...) - calls a global function by name",
            |vm, c| {
                let args = c.argv.get(2..).unwrap_or_default();
                let name = c.argv.get(1).map_or(&b""[..], Vec::as_slice);
                vm.quirks.eval_via_call = c.from_bytecode && name.eq_ignore_ascii_case(b"eval");
                let called = vm.invoke(None, name, args);
                vm.quirks.eval_via_call = false;
                match called {
                    Ok(v) => Native::Str(v),
                    Err(e) => Native::Fail(e),
                }
            },
        );
        // The call stack (see `print_backtrace`).
        vm.register(
            "backtrace",
            1,
            1,
            "backtrace() - prints the script functions that are running",
            |vm, _| {
                let line = vm.backtrace_text();
                // `print` charges the line (one work unit per byte, see `log_bytes`): a loop of BackTraces
                // meets the step limit.
                vm.print(&line);
                Native::Void
            },
        );
        vm.register(
            "activatePackage",
            2,
            2,
            "activatePackage(name) - puts the package on top of the active ones",
            |vm, c| {
                vm.activate_package(c.argv.get(1).map_or(&b""[..], Vec::as_slice));
                Native::Void
            },
        );
        vm.register(
            "deactivatePackage",
            2,
            2,
            "deactivatePackage(name) - takes the package and the ones above it off the active stack",
            |vm, c| {
                vm.deactivate_package(c.argv.get(1).map_or(&b""[..], Vec::as_slice));
                Native::Void
            },
        );
        vm.register(
            "isPackage",
            2,
            2,
            "isPackage(name) - whether a package of that name exists",
            |vm, c| {
                Native::Bool(
                    vm.namespaces
                        .is_package(c.argv.get(1).map_or(&b""[..], Vec::as_slice)),
                )
            },
        );
        // `isFunction(name)` looks the bare name up in the global namespace chain only
        // (`isFunction("Ns::bar")` is 0); `isFunction(namespace, name)` looks in that namespace.
        vm.register(
            "isFunction",
            2,
            3,
            "isFunction([namespace,] name) - whether a function of that name exists",
            |vm, c| {
                let arg = |i: usize| c.argv.get(i).map_or(&b""[..], Vec::as_slice);
                let found = if c.argv.len() >= 3 {
                    vm.namespaces.lookup(arg(1), arg(2))
                } else {
                    vm.namespaces.lookup(b"", arg(1))
                };
                Native::Bool(found.is_some())
            },
        );
        sim_natives::register_globals(&mut vm);
        vm.link_class_namespaces();
        sim_natives::register_object_methods(&mut vm);
        sim_natives::register_set_methods(&mut vm);
        tag_natives::register(&mut vm);
        files::register(&mut vm);
        schedule::register(&mut vm);
        variables::register(&mut vm);
        words::register(&mut vm);
        strings::register(&mut vm);
        math::register(&mut vm);
        vectors::register(&mut vm);
        file_natives::register(&mut vm);
        lifecycle::register(&mut vm);
        vm
    }

    /// Sets the files scripts load (the default has none).
    ///
    /// The names of the host's list go into the string table at once, in the order the start-up scan
    /// found them (the list is the scan reversed): the resource manager interns every folder and file name before any
    /// script runs, so the first spelling of a name is the scan's, and `findFirstFile` shows it. `exec`'s
    /// messages do not (see `file_name`). A host with a lazy list builds it here.
    pub fn set_file_host(&mut self, files: impl FileHost + 'static) {
        self.files = Box::new(files);
        self.seed_names();
    }

    /// Replaces the files scripts load with what `wrap` makes of them: a layer over the host that is there, which it must
    /// keep serving whatever it does not change. The names of the new host's list go into the string table as for
    /// [`set_file_host`](Self::set_file_host).
    pub fn wrap_file_host(&mut self, wrap: impl FnOnce(Box<dyn FileHost>) -> Box<dyn FileHost>) {
        let inner = std::mem::replace(&mut self.files, Box::new(NoFiles));
        self.files = wrap(inner);
        self.seed_names();
    }

    /// Sets the clocks scripts read (the default is [`SystemClock`]).
    pub fn set_clock(&mut self, clock: impl Clock + 'static) {
        self.clock = Box::new(clock);
    }

    /// Whether `name` is a native of the global namespace (a dialect may refuse a script declaration of it,
    /// `Namespaces::shadows_native`).
    pub fn is_native_function(&self, name: &[u8]) -> bool {
        self.namespaces.shadows_native(b"", None, name)
    }

    /// Sets the compiler for TorqueScript source (the default, [`NoCompiler`], reports that there is none; the
    /// game installs its dialect's compiler with its executor).
    pub fn set_compiler(&mut self, compiler: impl Compiler + 'static) {
        self.compiler = Box::new(compiler);
    }

    /// The deepest call depth reached so far, in the units of `MAX_CALL_DEPTH` (each script function one,
    /// each re-entry from Rust one more): the measure of how much of the Rust stack scripts needed.
    pub fn peak_depth(&self) -> usize {
        self.peak_depth
    }

    /// The call depth now, in the units of `MAX_CALL_DEPTH` (see [`peak_depth`](Self::peak_depth)): a host that runs
    /// script from Rust through frames of its own checks it against `MAX_CALL_DEPTH` first (a native's call into script
    /// that calls the native again is otherwise checked only when a script function is entered).
    pub fn call_depth(&self) -> usize {
        self.depth
    }

    /// Runs `f` (a host's call into script) with `units` more call-depth units charged while it runs: for a host whose
    /// Rust frames between two script levels are larger than the [`REENTRY_COST`] a re-entry is charged (a state
    /// machine that calls script from Rust), so that recursion through it stops at `CallDepth` before the end of the
    /// stack.
    #[inline]
    pub fn with_extra_depth<T>(&mut self, units: usize, f: impl FnOnce(&mut Vm) -> T) -> T {
        self.depth = self.depth.saturating_add(units);
        let r = f(self);
        self.depth = self.depth.saturating_sub(units);
        r
    }

    /// The name of the code block that is executing, if any (and if it has one).
    pub fn current_file(&self) -> Option<&[u8]> {
        self.blocks.last().and_then(|b| b.name())
    }

    /// The tagged string table.
    pub fn tags(&self) -> &TagTable {
        &self.tags
    }

    pub fn tags_mut(&mut self) -> &mut TagTable {
        &mut self.tags
    }

    /// What the preload error buffer holds: the text of the last failed preload that wrote it (empty before one), which a
    /// preload that fails without writing it ([`PreloadError::Stale`](crate::classes::PreloadError::Stale)) shows again
    /// in its `preload failed` line.
    pub fn preload_error_buffer(&self) -> &[u8] {
        &self.preload_error
    }

    /// The objects, the class registry and the members of sets and groups.
    pub fn objects(&self) -> &Objects {
        &self.objects
    }

    pub fn objects_mut(&mut self) -> &mut Objects {
        &mut self.objects
    }

    /// Logs an error line (`Con::errorf`).
    pub fn error(&mut self, text: String) {
        self.log_bytes(Level::Error, text.as_bytes());
    }

    /// `activatePackage`: prints what TGE prints, in its order.
    #[inline(never)]
    fn activate_package(&mut self, package: &[u8]) {
        let shown = String::from_utf8_lossy(&self.namespaces.package_name(package)).into_owned();
        let messages = self.messages();
        if let Some(line) = messages.activating_package(&shown) {
            self.print(line.as_bytes());
        }
        match self.namespaces.activate(package) {
            Activation::Activated => {}
            Activation::AlreadyActive => {
                if let Some(line) = messages.package_already_active(&shown) {
                    self.print(line.as_bytes());
                }
            }
            Activation::NotFound => {
                if let Some(line) = messages.package_not_found(&shown) {
                    self.error(line);
                }
            }
            Activation::LimitReached => self.print(
                format!(
                    "ActivatePackage({shown}) failed - Max package limit reached: {}",
                    self.namespaces.max_active()
                )
                .as_bytes(),
            ),
        }
    }

    /// `deactivatePackage`: TGE announces it even for a package that is not active.
    #[inline(never)]
    fn deactivate_package(&mut self, package: &[u8]) {
        let shown = String::from_utf8_lossy(&self.namespaces.package_name(package)).into_owned();
        if let Some(line) = self.messages().deactivating_package(&shown) {
            self.print(line.as_bytes());
        }
        self.namespaces.deactivate(package);
    }

    /// Registers a native function in the global namespace. `min` and `max` count the function name as
    /// the first argument; `0` means no limit.
    pub fn register(
        &mut self,
        name: &str,
        min: i32,
        max: i32,
        usage: &str,
        f: impl Fn(&mut Vm, &NativeCall) -> Native + 'static,
    ) {
        self.register_in("", name, min, max, usage, f);
    }

    /// Registers a native function in the named namespace (empty for the global one).
    pub fn register_in(
        &mut self,
        namespace: &str,
        name: &str,
        min: i32,
        max: i32,
        usage: &str,
        f: impl Fn(&mut Vm, &NativeCall) -> Native + 'static,
    ) {
        // An existing entry of that name is replaced in place, so call sites bound to it see the new
        // native.
        if !self.stub_keys.0.is_empty() {
            self.stub_keys.0.remove(&format!("{namespace}::{name}"));
        }
        // `Namespace::addCommand` keeps the name as a string table entry, made at static initialisation, before any script:
        // a script that declares or names the function in another case spells it as the native does.
        let spelled = self.strings.intern(name.as_bytes());
        self.namespaces.declare(
            namespace.as_bytes(),
            None,
            &spelled,
            Callable::Native {
                min,
                max,
                usage: Rc::from(usage),
                f: Rc::new(f),
            },
        );
    }

    /// The names of the functions scripts defined (natives left out), sorted: see
    /// `Namespaces::script_function_names`.
    pub fn script_function_names(&self) -> Vec<Vec<u8>> {
        self.namespaces.script_function_names()
    }

    /// The natives registered, as `(namespace, name, min, max)`: see `Namespaces::native_signatures`.
    pub fn native_signatures(&self) -> Vec<(String, String, i32, i32)> {
        self.namespaces
            .native_signatures()
            .into_iter()
            .map(|(ns, name, min, max)| {
                (
                    String::from_utf8_lossy(&ns).into_owned(),
                    String::from_utf8_lossy(&name).into_owned(),
                    min,
                    max,
                )
            })
            .collect()
    }

    /// Every native registered, one line each, sorted, as text for a snapshot test: see `Namespaces::native_listing`.
    /// Each line ends with a line break.
    pub fn native_listing(&self) -> String {
        let mut out = String::new();
        for line in self.namespaces.native_listing() {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    /// Replaces the usage text of the native `namespace::name` (`namespace` empty for a global function), keeping the
    /// function and its argument counts; false when there is no such native.
    pub fn set_native_usage(&mut self, namespace: &str, name: &str, usage: &str) -> bool {
        self.namespaces
            .set_native_usage(namespace.as_bytes(), name.as_bytes(), usage)
    }

    /// The natives' usage texts (the empty ones left out), as `(namespace, name, usage)`: see
    /// `Namespaces::native_usages`.
    pub fn native_usages(&self) -> Vec<(String, String, String)> {
        self.namespaces
            .native_usages()
            .into_iter()
            .map(|(ns, name, usage)| {
                (
                    String::from_utf8_lossy(&ns).into_owned(),
                    String::from_utf8_lossy(&name).into_owned(),
                    usage,
                )
            })
            .collect()
    }

    /// The native `namespace`'s chain finds for `name` (its class parents' too), with its argument counts and usage: a
    /// host that replaces a native for some objects keeps the old one for the rest (`None` when the name is a script
    /// function or missing).
    #[allow(clippy::type_complexity)]
    pub fn native_of(
        &self,
        namespace: &str,
        name: &str,
    ) -> Option<(
        i32,
        i32,
        Rc<str>,
        Rc<dyn Fn(&mut Vm, &NativeCall) -> Native>,
    )> {
        let entry = self
            .namespaces
            .lookup(namespace.as_bytes(), name.as_bytes())?;
        let entry = entry.borrow();
        match &entry.callable {
            Callable::Native { min, max, usage, f } => {
                Some((*min, *max, Rc::clone(usage), Rc::clone(f)))
            }
            Callable::Script { .. } => None,
        }
    }

    /// Whether `namespace` itself (not a parent) has a function `name`.
    pub fn has_own_function(&self, namespace: &str, name: &str) -> bool {
        self.namespaces
            .has_own(namespace.as_bytes(), name.as_bytes())
    }

    /// Counts a call of a stub (see `classes::install`).
    pub fn count_stub(&mut self, key: &str) {
        match self.stub_calls.get_mut(key) {
            Some(count) => *count = count.saturating_add(1),
            None => {
                self.stub_calls.insert(key.to_string(), 1);
            }
        }
    }

    /// Marks `key` (`Class::method`) as a class-table stub (after its registration; any later `register_in` of it
    /// clears the mark).
    pub fn mark_stub(&mut self, key: &str) {
        self.stub_keys.0.insert(key.to_string());
    }

    /// The methods that are still the class table's stubs, sorted.
    pub fn stub_methods(&self) -> Vec<String> {
        self.stub_keys.0.iter().cloned().collect()
    }

    /// How often each stub was called, by `Class::method`.
    pub fn stub_calls(&self) -> &std::collections::BTreeMap<String, u64> {
        &self.stub_calls
    }

    /// Runs a function by name from Rust and returns its result as a string (no numeric peephole).
    /// Script functions run in a new local frame and share the step and call-depth budgets of the run
    /// that is in progress; re-entering the VM from a native costs one extra call-depth unit (see the
    /// [`Vm`] documentation). A missing global function warns `<name>: Unknown command.`, as `Con::execute`
    /// does (silent when [`ConsoleRules::unknown_command`](crate::policy::ConsoleRules::unknown_command) is off),
    /// and a missing one in a namespace is silent; both return the empty string. On error the evaluation stacks
    /// are restored, as for `run`.
    ///
    /// The current variable is put back afterwards, as `run` does. Compiled code always selects its
    /// variable again after evaluating a sub-expression, so this is a precaution for natives;
    /// script-to-script calls do not restore it.
    pub fn invoke(
        &mut self,
        namespace: Option<&[u8]>,
        name: &[u8],
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, VmError> {
        self.invoke_from(CommandSource::LOCAL, namespace, name, args)
    }

    /// [`invoke`](Self::invoke) for `source` (a remote party's command): its "Unknown command" warnings are limited apart from
    /// every other source's ([`MAX_UNKNOWN_COMMAND_LINES`]).
    pub fn invoke_from(
        &mut self,
        source: CommandSource,
        namespace: Option<&[u8]>,
        name: &[u8],
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, VmError> {
        let Some(entry) = self.namespaces.lookup(namespace.unwrap_or(b""), name) else {
            if namespace.is_none_or(<[u8]>::is_empty) && self.console_rules().unknown_command {
                self.warn_unknown_command(source, name);
            }
            return Ok(Vec::new());
        };
        let mut argv = Vec::with_capacity(args.len().saturating_add(1));
        argv.push(name.to_vec());
        argv.extend(args.iter().cloned());
        let saved = self.enter_run();
        let result = self.invoke_callable(entry, argv, true);
        self.leave_run(saved, result)
    }

    /// [`invoke`](Self::invoke) with the argument list made by the caller, which gives it up (`argv[0]` is the
    /// function's name, then the arguments): nothing is copied. `keep` is whether the result is wanted; a caller that
    /// drops it (a scheduled event) skips the copy, and gets the empty string.
    pub fn invoke_owned(
        &mut self,
        namespace: Option<&[u8]>,
        argv: Vec<Vec<u8>>,
        keep: bool,
    ) -> Result<Vec<u8>, VmError> {
        let name = argv.first().map(Vec::as_slice).unwrap_or_default();
        let Some(entry) = self.namespaces.lookup(namespace.unwrap_or(b""), name) else {
            if namespace.is_none_or(<[u8]>::is_empty) && self.console_rules().unknown_command {
                let name = argv.into_iter().next().unwrap_or_default();
                self.warn_unknown_command(CommandSource::LOCAL, &name);
            }
            return Ok(Vec::new());
        };
        let saved = self.enter_run();
        let result = self.invoke_callable(entry, argv, keep);
        self.leave_run(saved, result)
    }

    /// Whether `name` is a function of `namespace` (`None`: the global one), looked up as [`invoke`](Self::invoke) and
    /// `isFunction` look it up: a host that charges a remote party for a name with no function asks first.
    #[must_use]
    pub fn is_function(&self, namespace: Option<&[u8]>, name: &[u8]) -> bool {
        self.namespaces
            .lookup(namespace.unwrap_or(b""), name)
            .is_some()
    }

    /// `Con::execute`'s line for a global function that does not exist.
    #[cold]
    #[inline(never)]
    fn warn_unknown_command(&mut self, source: CommandSource, name: &[u8]) {
        let now = self.sim_time();
        self.flush_unknown_commands(now);
        let source = if self.unknown_commands.contains_key(&source)
            || self.unknown_commands.len() < MAX_COMMAND_SOURCES
        {
            source
        } else {
            CommandSource::OVERFLOW
        };
        let u = self
            .unknown_commands
            .entry(source)
            .or_insert(UnknownCommands {
                since: now,
                printed: 0,
                left_out: 0,
            });
        if u.printed >= MAX_UNKNOWN_COMMAND_LINES {
            u.left_out = u.left_out.saturating_add(1);
            return;
        }
        u.printed += 1;
        let mut line = until_nul(name).to_vec();
        line.extend_from_slice(b": Unknown command.");
        self.log_bytes(Level::Warning, &line);
    }

    /// Ends the seconds of the sources whose second is over at `now`: each that left lines out prints how many.
    pub(crate) fn flush_unknown_commands(&mut self, now: u32) {
        if self.unknown_commands.is_empty() {
            return;
        }
        let mut lines = Vec::new();
        self.unknown_commands.retain(|source, u| {
            if now.wrapping_sub(u.since) < 1000 {
                return true;
            }
            if u.left_out > 0 {
                lines.push((*source, u.left_out));
            }
            false
        });
        lines.sort_unstable_by_key(|(source, _)| source.0);
        for (source, n) in lines {
            let line = match source {
                CommandSource::LOCAL => format!("{n} more unknown commands were not printed."),
                CommandSource::OVERFLOW => {
                    format!("{n} more unknown commands from other sources were not printed.")
                }
                CommandSource(s) => {
                    format!("{n} more unknown commands from source {s} were not printed.")
                }
            };
            self.log_bytes(Level::Warning, line.as_bytes());
        }
    }

    /// Runs the method `name` of an object from Rust, as `Con::executef(object, ...)`:
    /// `%this` is the id text, then `args`. A missing object or method is silent and gives the empty
    /// string. Like [`invoke`](Self::invoke) it shares the budgets of the run in progress and, inside
    /// one, costs `REENTRY_COST` extra call-depth units.
    pub fn call_method(
        &mut self,
        object: ObjId,
        name: &[u8],
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, VmError> {
        let Some(entry) = self.method_entry(object, name) else {
            return Ok(Vec::new());
        };
        let argv = method_argv(object, name, args);
        let saved = self.enter_run();
        let outer = self.this_object.replace(object);
        let result = self.invoke_callable(entry, argv, true);
        self.this_object = outer;
        self.leave_run(saved, result)
    }

    /// [`call_method`](Self::call_method) with the argument list made by the caller, which gives it up (`argv[0]` is
    /// the method's name, then the arguments, without the object's id: it is put in here). `keep` as for
    /// [`invoke_owned`](Self::invoke_owned).
    pub fn call_method_owned(
        &mut self,
        object: ObjId,
        mut argv: Vec<Vec<u8>>,
        keep: bool,
    ) -> Result<Vec<u8>, VmError> {
        let name = argv.first().map(Vec::as_slice).unwrap_or_default();
        let Some(entry) = self.method_entry(object, name) else {
            return Ok(Vec::new());
        };
        argv.insert(
            1.min(argv.len()),
            crate::text::format_int(object).into_bytes(),
        );
        let saved = self.enter_run();
        let outer = self.this_object.replace(object);
        let result = self.invoke_callable(entry, argv, keep);
        self.this_object = outer;
        self.leave_run(saved, result)
    }

    /// A method call as bytecode makes it (`<target>.<name>(<args>)`, call type 1), from Rust: the object `target` names
    /// (`Sim::findObject`), its method `name`, `%this` the target's text as written. A missing object warns as the
    /// interpreter does, naming `label` (a script call site's `file (line)`, which a host doing that script's work gives), and
    /// so does a missing method; both answer "". Shares the budgets of the run in progress, as
    /// [`call_method`](Self::call_method).
    pub fn call_method_text(
        &mut self,
        label: &str,
        target: &[u8],
        name: &[u8],
        args: &[Vec<u8>],
    ) -> Result<Vec<u8>, VmError> {
        let Some(object) = self.objects.find(target) else {
            self.warn_missing_object(label.to_string(), target, name)?;
            return Ok(Vec::new());
        };
        let Some(entry) = self.method_entry(object, name) else {
            let namespace = self
                .objects
                .get(object)
                .map(|o| o.namespace().to_vec())
                .unwrap_or_default();
            self.warn_unknown_method(label.to_string(), name, object, &namespace);
            return Ok(Vec::new());
        };
        let mut argv = Vec::with_capacity(args.len().saturating_add(2));
        argv.push(name.to_vec());
        argv.push(target.to_vec());
        argv.extend(args.iter().cloned());
        let saved = self.enter_run();
        let outer = self.this_object.replace(object);
        let result = self.invoke_callable(entry, argv, true);
        self.this_object = outer;
        self.leave_run(saved, result)
    }

    /// A field of an object as `LOADFIELD_STR` reads it (`<object>.<name>[<array>]`): "" for no object or an engine-made one;
    /// charged as the interpreter charges the copy.
    pub fn script_field(&self, object: ObjId, name: &[u8], array: &[u8]) -> Vec<u8> {
        let Some(o) = self.objects.get(object) else {
            return Vec::new();
        };
        if o.engine_made {
            return Vec::new();
        }
        let objects = &self.objects;
        let names = |id: ObjId| objects.get(id).and_then(|o| o.name().map(<[u8]>::to_vec));
        let mut charge = |units: u64| self.charge_work(units);
        o.data_field_charged(&objects.classes, name, array, &names, &mut charge)
    }

    /// A field write as `SAVEFIELD_STR` makes it (`<object>.<name>[<array>] = <value>`): through the field's type and
    /// validator, the static-write observer told; nothing for no object or an engine-made one.
    pub fn script_set_field(&mut self, object: ObjId, name: &[u8], array: &[u8], value: &[u8]) {
        if self.objects.get(object).is_none_or(|o| o.engine_made) {
            return;
        }
        self.write_field(Some(object), name, array, value);
    }

    /// `$name = <float>` as `SAVEVAR_FLT` stores it (a float variable: the text is `%g` of the 32-bit value).
    pub fn set_global_float(&mut self, name: &[u8], value: f64) {
        if let Some(entry) = self.globals.add(name) {
            entry.set_float(value);
        }
    }

    /// `$name = <uint>` as `SAVEVAR_UINT` stores it.
    pub fn set_global_uint(&mut self, name: &[u8], value: u32) {
        if let Some(entry) = self.globals.add(name) {
            entry.set_int(value);
        }
    }

    /// `$name = <text>` as `SAVEVAR_STR` stores it, by a name of any bytes.
    pub fn set_global_text(&mut self, name: &[u8], value: &[u8]) {
        if let Some(entry) = self.globals.add(name) {
            entry.set_string(value);
        }
    }

    /// How many times text has been evaluated here (`eval`, the native or the host's `Vm::eval`).
    pub fn eval_count(&self) -> u64 {
        self.quirks.evals
    }

    /// The method `name` in the namespace chain of `object`, if both exist.
    #[inline(never)]
    fn method_entry(&self, object: ObjId, name: &[u8]) -> Option<EntryRef> {
        let namespace = self.objects.get(object)?.namespace();
        self.namespaces.lookup(namespace, name)
    }

    /// Registers a class below `parent` with its static fields and links its namespace to the parent's,
    /// as TGE does for every class at start-up. None when the name is taken or the parent is
    /// unknown. A namespace conflict with a link a script made earlier (`class = Foo` on a ScriptObject)
    /// is reported as `Error: cannot change namespace parent linkage ...` and the
    /// class is still registered. Hooks go on the returned class (`objects_mut().classes.get_mut(id)`);
    /// natives with `register_in(name, ...)`.
    pub fn register_class(&mut self, name: &str, parent: &str, fields: &[&str]) -> Option<ClassId> {
        let class = self.objects.classes.register(name, parent, fields)?;
        self.strings.intern(name.as_bytes());
        let parent = self.objects.classes.get(class)?.parent?;
        let parent_name = self.objects.classes.name(parent).to_vec();
        // A name that scripts already linked under another parent keeps that parent: the error is printed,
        // error and carries on, and so does this (the class is registered, its objects walk the old chain).
        if let Err(conflict) = self.namespaces.link_class(name.as_bytes(), &parent_name) {
            self.error(conflict.message());
        }
        Some(class)
    }

    /// Like [`register_class`](Self::register_class) with typed static fields.
    pub fn register_class_typed(
        &mut self,
        name: &str,
        parent: &str,
        fields: Vec<crate::sim::StaticField>,
        inherits_fields: bool,
    ) -> Option<ClassId> {
        let class = self
            .objects
            .classes
            .register_typed(name, parent, fields, inherits_fields)?;
        let parent = self.objects.classes.get(class)?.parent?;
        let parent_name = self.objects.classes.name(parent).to_vec();
        self.strings.intern(name.as_bytes());
        if let Err(conflict) = self.namespaces.link_class(name.as_bytes(), &parent_name) {
            self.error(conflict.message());
        }
        Some(class)
    }

    /// Installs the host's observer of static-field writes (replacing one): after the validator of every `setDataField` whose
    /// name matched a registered, non-deprecated static field, the observer is called with the object (None while `new`
    /// or `datablock` is still building it, as the fields of an object are written before it registers; a datablock a
    /// script declares again is registered) and its class. It is the `onStaticModified` hook point (called after every
    /// such write, also one whose index was out of range and one that stored the value it already held). With no
    /// observer set a write does exactly what it did.
    pub fn set_static_write_observer(&mut self, observer: Option<StaticWriteFn>) {
        self.quirks.static_observer = statics::StaticObserver(observer);
    }

    /// Gives a class a Rust behaviour (replacing one it had); false when there is no class of that name. Its
    /// objects and those of its subclasses run it, after their ancestors' behaviours for `on_add` and `preload`,
    /// before them for `on_remove` (see [`EngineClass`](crate::classes::EngineClass)).
    pub fn set_class_behaviour(
        &mut self,
        class: &str,
        behaviour: Rc<dyn crate::classes::EngineClass>,
    ) -> bool {
        let Some(id) = self.objects.classes.find(class.as_bytes()) else {
            return false;
        };
        match self.objects.classes.get_mut(id) {
            Some(def) => {
                def.behaviour = Some(behaviour);
                true
            }
            None => false,
        }
    }

    /// Links every class's namespace to its parent's (`SimGroup -> SimSet -> SimObject`, ...) and puts every
    /// class name in the string table.
    fn link_class_namespaces(&mut self) {
        for index in 0..self.objects.classes.len() {
            let Ok(class) = ClassId::try_from(index) else {
                break;
            };
            self.strings.intern(self.objects.classes.name(class));
            let Some(parent) = self.objects.classes.get(class).and_then(|c| c.parent) else {
                continue;
            };
            let child = self.objects.classes.name(class).to_vec();
            let parent = self.objects.classes.name(parent).to_vec();
            let _ = self.namespaces.link_class(&child, &parent);
        }
    }

    /// Starts a run: restarts the step budget when no run is in progress, charges the call-depth
    /// budget when one is (a re-entry from a native costs [`REENTRY_COST`] extra depth units, for the
    /// Rust frames in between), and records what an error must restore.
    fn enter_run(&mut self) -> RunState {
        if self.running == 0 {
            self.steps = 0;
            // Work done outside a run (host calls into the objects, natives called from a top-level
            // `invoke`) belongs to no script step: it must not be charged to this run.
            self.objects.take_work();
            self.namespaces.take_work();
            self.executor.take_work();
            self.executor.take_overflow();
            // Objects the host unregistered since the last run lose their events here; the work of
            // dropping them is this run's.
            self.drop_events_of_unregistered();
            // A host budget with run steps (a tick's callbacks) lowers this run's limit to what it has left.
            self.begin_charged_run();
            self.begin_memory_run();
        } else {
            self.depth = self.depth.saturating_add(REENTRY_COST);
        }
        self.running = self.running.saturating_add(1);
        RunState {
            exec: self.executor.save(),
        }
    }

    /// Ends a run started with `enter_run` and returns its result. After an error the evaluation stacks go
    /// back to what the run found, so a native that swallows the error leaves the surrounding expression
    /// intact.
    ///
    /// A string cut short at the size cap during the last instruction of a run, or by a native called from
    /// Rust, has not been seen by `fetch_op`: the outermost run turns it into `StringTooLong` here.
    fn leave_run<T>(&mut self, saved: RunState, result: Result<T, VmError>) -> Result<T, VmError> {
        self.running = self.running.saturating_sub(1);
        if self.running > 0 {
            self.depth = self.depth.saturating_sub(REENTRY_COST);
        } else {
            self.bill_unbilled_work();
            self.end_charged_run();
        }
        let cut = self.running == 0 && self.executor.take_overflow();
        self.executor.restore(saved.exec, result.is_err() || cut);
        if cut && result.is_ok() {
            return Err(VmError::StringTooLong);
        }
        if self.running == 0 && result.is_ok() && self.objects.memory().take_refused() {
            return Err(VmError::MemoryLimit);
        }
        result
    }

    /// The work an outermost run ended with that no instruction charged joins the run's steps: the interpreter
    /// charges work (`fetch_op`) at the start of the next instruction, so what a native did last (an event
    /// whose target is `export` or `strreplace` runs no instruction after it) would sit in the counters until
    /// the next run dropped it, and a host that counts a run's steps (`Vm::advance_time_counted`) would see an
    /// event that cost seconds as one that cost nothing. Never an error: the run is over (and with no step
    /// limit steps are not counted). Out of line: only the outermost `leave_run` calls it.
    #[inline(never)]
    fn bill_unbilled_work(&mut self) {
        let work = self
            .objects
            .take_work()
            .saturating_add(self.namespaces.take_work())
            .saturating_add(self.executor.take_work());
        if self.step_limit.is_some() {
            self.steps = self.steps.saturating_add(work / WORK_PER_STEP);
        }
    }

    /// Runs an entry from Rust (`invoke`, `call_method`) and returns its result as a string. On the
    /// recursion path of `call` and of a script `onAdd`, so natives (which add nothing to the depth
    /// chain of scripts) are handled out of line.
    #[inline(never)]
    fn invoke_callable(
        &mut self,
        entry: EntryRef,
        argv: Vec<Vec<u8>>,
        keep: bool,
    ) -> Result<Vec<u8>, VmError> {
        let Some((block, offset)) = script_target(&entry) else {
            return self.invoke_native(&entry, argv);
        };
        if offset == 0 {
            return Ok(Vec::new());
        }
        let call = self.new_call(argv, entry);
        self.run_block(&block, offset, Some(call), Mode::PLAIN)?;
        Ok(if keep {
            self.executor.result()
        } else {
            Vec::new()
        })
    }

    /// A native called from Rust: the file is the code block that is executing (the one that called
    /// `call`, say), the current object the one of the method that is running.
    #[inline(never)]
    fn invoke_native(&mut self, entry: &EntryRef, argv: Vec<Vec<u8>>) -> Result<Vec<u8>, VmError> {
        let Callable::Native { min, max, usage, f } = entry.borrow().callable.clone() else {
            return Ok(Vec::new());
        };
        let name = argv.first().cloned().unwrap_or_default();
        if !argc_ok(argv.len(), min, max) {
            self.warn_wrong_argc(entry, &name, &usage)?;
            return Ok(Vec::new());
        }
        let call = NativeCall {
            argv,
            this_object: self.this_object,
            from_bytecode: false,
            name,
            namespace: Rc::clone(&entry.borrow().namespace),
            file: self.blocks.last().and_then(|b| b.name_shared()),
        };
        native_to_string(f(self, &call))
    }

    /// The argument-count warning of a native called from Rust (`invoke`, `call`, `call_method`:
    /// `Namespace::Entry::execute`): no file prefix, the namespace the entry lives in and
    /// `(null)` for the global one (printf of a NULL name), between blank lines, with a
    /// BackTrace.
    #[cold]
    #[inline(never)]
    fn warn_wrong_argc(
        &mut self,
        entry: &EntryRef,
        name: &[u8],
        usage: &str,
    ) -> Result<(), VmError> {
        let namespace = entry.borrow().namespace.clone();
        let scope = if namespace.is_empty() {
            "(null)".to_string()
        } else {
            lossy(&self.namespaces.display(&namespace))
        };
        let loud = self.console_rules().backtraces;
        if loud {
            self.print(b"");
        }
        self.warn(format!(
            "{scope}::{} - wrong number of arguments.",
            lossy(name)
        ));
        self.warn(format!("usage: {usage}"));
        if loud {
            self.print_backtrace()?;
            self.print(b"");
        }
        Ok(())
    }

    /// When the console rules ask for it ([`ConsoleRules::backtraces`](crate::policy::ConsoleRules::backtraces)), what
    /// `Con::evaluate("backtrace();")` does after a message: the global function `backtrace` runs, normally the native,
    /// which prints its line at the normal level. A host can replace it with a native that runs a script. Without
    /// any, an evaluated call would report the missing function and ask for a BackTrace again until its stack
    /// overflows; this
    /// reports it once (unreachable today: `Vm::new` registers the native and scripts cannot remove it, so
    /// this is a guard for a host that does). An error of a replacement (`CallDepth`, the step limit)
    /// is returned: callers propagate it.
    #[cold]
    #[inline(never)]
    pub fn backtrace_after_message(&mut self) -> Result<(), VmError> {
        if self.console_rules().backtraces {
            self.print_backtrace()
        } else {
            Ok(())
        }
    }

    /// The BackTrace itself (see [`backtrace_after_message`](Self::backtrace_after_message)).
    #[cold]
    #[inline(never)]
    pub fn print_backtrace(&mut self) -> Result<(), VmError> {
        if self.namespaces.lookup(b"", b"backtrace").is_none() {
            self.warn("<input> (0): Unable to find function backtrace".to_string());
            return Ok(());
        }
        self.invoke(None, b"backtrace", &[]).map(|_| ())
    }

    /// The line the `backtrace` native prints: `BackTrace: ` then, outermost first, one
    /// `->` per running script function: `[Package]` when the layer the function's namespace object
    /// holds belongs to a package (see [`Namespaces::frame_package`]), the namespace and `::` for a
    /// namespaced function, and its declared name.
    pub fn backtrace_text(&self) -> Vec<u8> {
        // This is one console line, which ends at `MAX_LOG_LINE` bytes: nothing is
        // built beyond that however deep the recursion or long the names, and the loop stops there.
        // Each frame's layer scan is charged by `frame_package`; the `backtrace` native charges the line.
        let mut out = b"BackTrace: ".to_vec();
        for (entry, holder) in &self.traces {
            if out.len() >= MAX_LOG_LINE {
                break;
            }
            push_capped(&mut out, b"->");
            let entry = entry.borrow();
            let namespace = &entry.namespace;
            if let Some(package) = self.namespaces.frame_package(namespace, *holder) {
                push_capped(&mut out, b"[");
                push_capped(&mut out, package);
                push_capped(&mut out, b"]");
            }
            if !namespace.is_empty() {
                let shown = self.namespaces.display_ref(namespace);
                push_capped(&mut out, shown.unwrap_or(namespace));
                push_capped(&mut out, b"::");
            }
            push_capped(
                &mut out,
                script_name_ref(&entry.callable).unwrap_or_default(),
            );
        }
        out
    }

    pub fn set_step_limit(&mut self, limit: Option<u64>) {
        self.step_limit = limit;
        self.steps = 0;
    }

    /// The steps the run in progress has taken, or, outside a run, the last outermost run's (the next run starts
    /// again at 0). Counted only while a step limit is set ([`set_step_limit`](Self::set_step_limit)); 0 otherwise.
    /// A host that sizes a limit from a measured run reads it after the run (the work of a native that ended
    /// the run is already in it).
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// The lines the `Vm` has printed and a host has not taken ([`drain_log`](Self::drain_log)), oldest first. At
    /// most [`MAX_LOG_BYTES`] of them are held: once a run prints more, the oldest are dropped
    /// ([`dropped_log_lines`](Self::dropped_log_lines) counts them), so a host that wants every line takes them
    /// between runs, and one that keeps an index into this slice must not (an index shifts by what was dropped).
    pub fn log(&self) -> &[LogLine] {
        &self.log.lines[self.log.head..]
    }

    /// Takes the held log lines out of the `Vm`, oldest first, and frees what held them: the next lines start a
    /// new log. A host that prints the lines as they come calls this after each print, so that a script that
    /// prints for ever never keeps more than what one run printed. The count of dropped lines is not reset.
    pub fn drain_log(&mut self) -> Vec<LogLine> {
        let mut lines = std::mem::take(&mut self.log.lines);
        lines.drain(..self.log.head);
        self.log.head = 0;
        self.log.held = 0;
        for line in &lines {
            self.tally_gone(line.level, &line.text);
        }
        lines
    }

    /// A line leaves the log: the function its warning names (if it is one) stays in the tally of missing ones.
    fn tally_gone(&mut self, level: Level, text: &[u8]) {
        if level != Level::Warning {
            return;
        }
        let mut one = std::collections::BTreeMap::new();
        count_called(&mut one, text);
        for (name, count) in one {
            match self.log.gone_missing.get_mut(&name) {
                Some(seen) => *seen = seen.saturating_add(count),
                None => {
                    let size = name.len().saturating_add(32);
                    if self.log.gone_missing_bytes.saturating_add(size) <= MAX_GONE_MISSING_BYTES {
                        self.log.gone_missing_bytes += size;
                        self.log.gone_missing.insert(name, count);
                    }
                }
            }
        }
    }

    /// The tally of missing functions for lines that left the log
    /// ([`boot::missing_functions`](crate::boot::missing_functions)).
    pub(crate) fn gone_missing(&self) -> &std::collections::BTreeMap<String, usize> {
        &self.log.gone_missing
    }

    /// How many lines were dropped from the log for passing [`MAX_LOG_BYTES`] (all runs together; never reset).
    pub fn dropped_log_lines(&self) -> u64 {
        self.log.dropped
    }

    pub fn log_texts(&self) -> Vec<String> {
        self.log()
            .iter()
            .map(|l| String::from_utf8_lossy(&l.text).into_owned())
            .collect()
    }

    pub fn print(&mut self, text: &[u8]) {
        self.log_bytes(Level::Normal, until_nul(text));
    }

    pub fn warn(&mut self, text: String) {
        self.log_bytes(Level::Warning, text.as_bytes());
    }

    /// The one place a log line is made. `Con::_printf` formats into a 4096-byte buffer, so
    /// a longer line is cut at 4095 bytes (a 5016-byte `echo` is logged as 4095). Retained log memory is
    /// therefore bounded per line, and every line is charged one work unit per byte it keeps (the unit of
    /// the string stack's writes: a step per [`WORK_PER_STEP`] bytes), so a loop that logs the same long
    /// message again and again meets the step limit after about `limit * 64 / 4095` lines of the longest
    /// kind, retaining at most that much log (a 4000-byte line costs 63 steps beyond the instruction's own;
    /// a missing function with a 4000-byte name in a 2M-step loop used to keep 1.9 GiB). The BackTrace line
    /// is charged here too.
    ///
    /// Every tab becomes `^`, as TGE's log shows it (`Con::_printf` rewrites tabs before it writes
    /// the line to the log, after the cut, at every level). A newline stays in the line: a log
    /// file shows it as a line break, and a host that wants lines splits them.
    pub fn log_bytes(&mut self, level: Level, text: &[u8]) {
        let kept = text.get(..MAX_LOG_LINE).unwrap_or(text);
        self.objects.add_work(kept.len() as u64);
        let mut text = kept.to_vec();
        for b in &mut text {
            if *b == b'\t' {
                *b = b'^';
            }
        }
        self.log.held = self
            .log
            .held
            .saturating_add(text.len().saturating_add(LOG_LINE_OVERHEAD));
        self.log.printed = self.log.printed.wrapping_add(1);
        if self.log.history.is_some() {
            self.keep_history(level, &text);
        }
        if let Some(sink) = self.log.sink.as_mut() {
            sink(level, &text);
        }
        self.log.lines.push(LogLine { level, text });
        if self.log.held > MAX_LOG_BYTES {
            self.drop_oldest_log_lines();
        }
    }

    /// The console history's part of a printed line ([`Vm::console_history`]).
    #[inline(never)]
    fn keep_history(&mut self, level: Level, text: &[u8]) {
        let log = &mut *self.log;
        let Some(h) = log.history.as_mut() else {
            return;
        };
        for part in text.split(|b| *b == b'\n') {
            log.history_bytes = log
                .history_bytes
                .saturating_add(part.len() + LOG_LINE_OVERHEAD);
            h.push_back(LogLine {
                level,
                text: part.to_vec(),
            });
            while (h.len() > MAX_HISTORY_LINES || log.history_bytes > MAX_HISTORY_BYTES)
                && h.len() > 1
            {
                if let Some(old) = h.pop_front() {
                    log.history_bytes = log
                        .history_bytes
                        .saturating_sub(old.text.len() + LOG_LINE_OVERHEAD);
                }
            }
        }
    }

    /// Keeps the console history from now on (a client's GuiConsole reads it), or stops and forgets it.
    pub fn keep_console_history(&mut self, on: bool) {
        if on {
            if self.log.history.is_none() {
                self.log.history = Some(std::collections::VecDeque::new());
            }
        } else {
            self.log.history = None;
            self.log.history_bytes = 0;
        }
    }

    /// Every line printed since the VM was made (the window's frame breakdown counts them a second).
    pub fn lines_printed(&self) -> u64 {
        self.log.printed
    }

    /// Hands every line printed from now on to `sink` as well (a `console.log`, written as the lines are made), or stops.
    pub fn set_console_sink(&mut self, sink: Option<ConsoleSink>) {
        self.log.sink = sink;
    }

    /// The console history (empty unless kept).
    pub fn console_history(&self) -> Option<&std::collections::VecDeque<LogLine>> {
        self.log.history.as_ref()
    }

    /// Clears the console history (`cls`).
    pub fn clear_console_history(&mut self) {
        if let Some(h) = self.log.history.as_mut() {
            h.clear();
            self.log.history_bytes = 0;
        }
    }

    /// Drops the oldest log lines until the held ones fit [`MAX_LOG_BYTES`] again (the newest is never dropped).
    /// The vector is compacted when the dropped slots are as many as the held lines, so the cost per line stays
    /// constant.
    #[cold]
    #[inline(never)]
    fn drop_oldest_log_lines(&mut self) {
        while self.log.held > MAX_LOG_BYTES && self.log.head + 1 < self.log.lines.len() {
            let level = self.log.lines[self.log.head].level;
            let dropped = std::mem::take(&mut self.log.lines[self.log.head].text);
            self.log.held = self
                .log
                .held
                .saturating_sub(dropped.len().saturating_add(LOG_LINE_OVERHEAD));
            self.tally_gone(level, &dropped);
            self.log.head += 1;
            self.log.dropped = self.log.dropped.saturating_add(1);
        }
        if self.log.head >= 4096 && self.log.head >= self.log.lines.len() - self.log.head {
            self.log.lines.drain(..self.log.head);
            self.log.head = 0;
        }
    }

    pub fn global(&self, name: &str) -> Option<Vec<u8>> {
        self.globals.get(name.as_bytes()).map(Entry::get_string)
    }

    /// A global by a name of any bytes (an array element: `$table` and the index text), as `LOADVAR_STR` reads it: None
    /// when there is no such variable.
    pub fn global_of(&self, name: &[u8]) -> Option<Vec<u8>> {
        self.globals.get(name).map(Entry::get_string)
    }

    /// A global's text as `LOADVAR_STR` reads it ("" when there is no such variable), handed to `f` without a copy.
    pub fn with_global<R>(&self, name: &[u8], f: impl FnOnce(&[u8]) -> R) -> R {
        match self.globals.get(name) {
            Some(entry) => entry.with_string(f),
            None => f(b""),
        }
    }

    /// A global's integer and float views by a name of any bytes, as `LOADVAR_UINT` and `LOADVAR_FLT` read them (0 when there
    /// is no such variable).
    pub fn global_views(&self, name: &[u8]) -> (u32, f64) {
        self.globals
            .get(name)
            .map_or((0, 0.0), |e| (e.get_int(), e.get_float()))
    }

    /// Every global, in the order of folded names: (the name as first written, its text). For a host's dump of the
    /// server state; it copies every value.
    pub fn globals_sorted(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut out: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> = self
            .globals
            .iter()
            .map(|e| {
                (
                    crate::text::fold(e.name()),
                    e.name().to_vec(),
                    e.get_string(),
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.into_iter()
            .map(|(_, name, value)| (name, value))
            .collect()
    }

    /// A number that changes whenever the namespace tables do (a function declared, a package activated or deactivated, a
    /// class linked): a host that checked which functions a name finds can tell its answer still holds.
    pub fn namespace_generation(&self) -> u64 {
        self.namespaces.generation()
    }

    /// Where the function a call of the global function `name` finds now (what an event or `invoke` runs) was declared.
    pub fn global_function_origin(&self, name: &[u8]) -> Option<FunctionOrigin> {
        Some(origin_of(&self.namespaces.lookup(b"", name)?))
    }

    /// Where the function `name` the namespace `namespace` (a class's, a datablock's) finds now was declared.
    pub fn namespace_function_origin(
        &self,
        namespace: &[u8],
        name: &[u8],
    ) -> Option<FunctionOrigin> {
        Some(origin_of(&self.namespaces.lookup(namespace, name)?))
    }

    /// Where the method `name` a call on `object` finds now was declared (None for no such object or method).
    pub fn method_origin(&self, object: ObjId, name: &[u8]) -> Option<FunctionOrigin> {
        Some(origin_of(&self.method_entry(object, name)?))
    }
}

/// Where an entry was declared.
fn origin_of(entry: &EntryRef) -> FunctionOrigin {
    let entry = entry.borrow();
    match &entry.callable {
        crate::ns::Callable::Script { block, offset } => FunctionOrigin {
            file: block.name_shared(),
            packaged: entry.package.is_some(),
            native: false,
            code: Some((Rc::clone(block), *offset)),
        },
        crate::ns::Callable::Native { .. } => FunctionOrigin {
            file: None,
            packaged: entry.package.is_some(),
            native: true,
            code: None,
        },
    }
}

impl Vm {
    /// A global's 32-bit float view (an engine variable of type F32 reads as the C global it is bound to: the value its last
    /// text was parsed to).
    pub fn global_f32(&self, name: &str) -> Option<f32> {
        self.globals
            .get(name.as_bytes())
            .map(|e| e.get_float() as f32)
    }

    /// A global's integer view.
    pub fn global_int(&self, name: &str) -> Option<u32> {
        self.globals.get(name.as_bytes()).map(Entry::get_int)
    }

    /// The runtime-wide string table's spelling of `text` (`StringTable::insert` without case sensitivity): the
    /// first spelling any `string` field, class name or host native stored, whatever the case. A host's
    /// natives that keep a string as TGE does store what this
    /// returns. The table is capped (16 MiB of text: past it a text is returned as it is, not remembered);
    /// the copy costs `text.len()`, which the caller charges.
    pub fn intern_string(&mut self, text: &[u8]) -> Rc<[u8]> {
        self.strings.intern(text)
    }

    /// `Con::addVariable`: `name` bound to an engine global of type `t` holding `initial` (a host registers its own
    /// variables before any script runs; see [`crate::vars::EngineType`]).
    pub fn add_engine_variable(&mut self, name: &str, t: crate::vars::EngineType, initial: &str) {
        self.globals
            .add_engine(name.as_bytes(), t, initial.as_bytes());
    }

    pub fn set_global(&mut self, name: &str, value: &str) {
        self.set_global_text(name.as_bytes(), value.as_bytes());
    }

    /// [`set_global`](Self::set_global) with a value of any bytes (a host's text from the network: not necessarily UTF-8).
    pub fn set_global_bytes(&mut self, name: &str, value: &[u8]) {
        self.set_global_text(name.as_bytes(), value);
    }

    /// Runs a block from its start in a fresh local frame and returns the string left on the string
    /// stack. (TGE runs the top level of a file in the frame of whoever ran it, which at the very
    /// top is none: see `exec_file` for that; a host's own run gets a frame so that its locals work.) On error the
    /// evaluation stacks are restored, so the VM stays usable.
    ///
    /// The step budget restarts with every outermost run or `invoke`; one started from inside a native
    /// function shares the budget of the run that called it.
    pub fn run(&mut self, block: &Rc<CodeBlock>) -> Result<Vec<u8>, VmError> {
        // `exec` already unwinds `frames` and `depth` on every exit, error or not. On error the rest is
        // undone to exactly what this run found: restoring (rather than replacing) the string stack
        // keeps the bookkeeping of an outer run intact when this run was started by a native function
        // in the middle of an expression.
        let saved = self.enter_run();
        let result = self.run_block(block, 0, None, Mode::own_frame());
        self.leave_run(saved, result)?;
        Ok(self.executor.result())
    }

    /// Runs `block` from `start` with this `Vm`'s executor (see [`Executor::run`]): the runtime's way into script.
    #[inline(never)]
    pub(crate) fn run_block(
        &mut self,
        block: &Rc<CodeBlock>,
        start: usize,
        call: Option<ScriptCall>,
        mode: Mode,
    ) -> Result<(), VmError> {
        let executor = Rc::clone(&self.executor);
        executor.run(self, block, start, call, mode)
    }

    /// Everything a running block owns besides the evaluation loop: its local frame (functions, and runs
    /// with `own_frame`), the object it is building, its entry for `backtrace` (functions), and its place
    /// as the executing block. Out of line, with [`leave_frame`](Self::leave_frame): the executor's frame is
    /// multiplied by the call depth, so it holds only the returned mark.
    #[inline(never)]
    pub fn enter_frame(
        &mut self,
        block: &Rc<CodeBlock>,
        call: Option<&ScriptCall>,
        mode: Mode,
    ) -> FrameMark {
        self.depth += 1;
        self.peak_depth = self.peak_depth.max(self.depth);
        let mark = FrameMark {
            frames: self.frames.len(),
            creating: self.creating.len(),
            traces: self.traces.len(),
        };
        if call.is_some() || mode.has_own_frame() {
            let frame = self
                .frame_pool
                .pop()
                .unwrap_or_else(|| Dictionary::with_account(self.objects.memory().clone()));
            self.frames.push(frame);
            self.alloc_frame_entered();
        }
        self.push_creating();
        if let Some(call) = call {
            if self.profile.is_some() {
                self.profile_enter(&call.entry);
            }
            self.traces.push((Rc::clone(&call.entry), call.holder));
        }
        self.blocks.push(Rc::clone(block));
        self.enter_file(block);
        mark
    }

    /// The block that [`enter_frame`](Self::enter_frame) gave `mark` ends.
    #[inline(never)]
    pub fn leave_frame(&mut self, mark: FrameMark) {
        if self.frames.len() > mark.frames {
            self.alloc_frame_left();
        }
        while self.frames.len() > mark.frames {
            let Some(mut frame) = self.frames.pop() else {
                break;
            };
            // The table goes back to the pool empty (a frame of a few locals; a big one is let go).
            if self.frame_pool.len() < 64 && frame.capacity() <= 64 {
                frame.clear();
                self.frame_pool.push(frame);
            }
        }
        self.pop_creating(mark.creating);
        if self.profile.is_some() && self.traces.len() > mark.traces {
            self.profile_leave();
        }
        self.traces.truncate(mark.traces);
        // `enter_frame` pushed exactly one block (no mark: the mark is multiplied by the call depth too).
        self.blocks.pop();
        self.leave_file();
        self.depth -= 1;
    }

    /// A block starts: `$Con::File` is its name when it has one (TGE's `CodeBlock::exec` sets `gCurrentFile` from a
    /// named block), else it stays what it was. Out of line, with [`leave_file`](Self::leave_file): `open_frame`
    /// is on the call path of every function.
    #[inline(never)]
    fn enter_file(&mut self, block: &Rc<CodeBlock>) {
        let shown = if block.name().is_some() {
            Some(Rc::clone(block))
        } else {
            self.con_file.blocks.last().cloned().flatten()
        };
        self.con_file.blocks.push(shown);
        self.show_con_file();
    }

    /// The block ends: `$Con::File` is the caller's again, "" (no variable) when no named block is left.
    #[inline(never)]
    fn leave_file(&mut self) {
        self.con_file.blocks.pop();
        self.show_con_file();
    }

    /// Brings the variable `$Con::File` up to the file that is running: a write only when the name changes.
    fn show_con_file(&mut self) {
        let want = self.con_file.blocks.last().and_then(Option::as_ref);
        let same = match (&self.con_file.shown, want) {
            (None, None) => true,
            (Some(shown), Some(want)) => Rc::ptr_eq(shown, want) || shown.name() == want.name(),
            _ => false,
        };
        if same {
            return;
        }
        let want = want.cloned();
        match want.as_ref().and_then(|block| block.name()) {
            Some(name) => self.set_global_text(CON_FILE, name),
            None => {
                self.globals.remove(CON_FILE);
            }
        }
        self.con_file.shown = want;
    }

    /// The warning between two empty warning lines, with a BackTrace.
    #[cold]
    #[inline(never)]
    pub fn warn_missing_object(
        &mut self,
        label: String,
        target: &[u8],
        name: &[u8],
    ) -> Result<(), VmError> {
        let loud = self.console_rules().backtraces;
        if loud {
            self.warn(String::new());
        }
        self.warn(format!(
            "{label}: Unable to find object: '{}' attempting to call function '{}'",
            lossy(target),
            lossy(name)
        ));
        if loud {
            self.print_backtrace()?;
            self.warn(String::new());
        }
        self.executor.set_result(b"");
        Ok(())
    }

    #[cold]
    #[inline(never)]
    pub fn warn_unknown_method(
        &mut self,
        label: String,
        name: &[u8],
        object: ObjId,
        namespace: &[u8],
    ) {
        let shown = self
            .objects
            .get(object)
            .and_then(SimObject::name)
            .map(lossy)
            .unwrap_or_default();
        let chain = lossy(&self.namespaces.chain_names(namespace));
        self.warn(format!("{label}: Unknown command {}.", lossy(name)));
        self.warn(format!("  Object {shown}({}) {chain}", object as i32));
        self.executor.set_result(b"");
    }

    /// The record of a script function about to run: records where its layer sits in the chain now.
    #[inline(never)]
    pub fn new_call(&self, argv: Vec<Vec<u8>>, entry: EntryRef) -> ScriptCall {
        let holder = self.namespaces.holder(&entry.borrow());
        ScriptCall {
            argv,
            entry,
            holder,
        }
    }
}

/// The arguments of a method call from Rust: the name, the object's id text, then `args`.
#[inline(never)]
fn method_argv(object: ObjId, name: &[u8], args: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut argv = Vec::with_capacity(args.len().saturating_add(2));
    argv.push(name.to_vec());
    argv.push(crate::text::format_int(object).into_bytes());
    argv.extend(args.iter().cloned());
    argv
}

/// A native's result as the string a call from Rust returns.
#[inline(never)]
fn native_to_string(result: Native) -> Result<Vec<u8>, VmError> {
    Ok(match result {
        Native::Str(s) => s,
        Native::Int(i) => crate::text::format_int(i as u32).into_bytes(),
        Native::Bool(b) => crate::text::format_int(u32::from(b)).into_bytes(),
        Native::Float(x) => crate::text::format_g(x).into_bytes(),
        Native::Void => Vec::new(),
        Native::Fail(e) => return Err(e),
    })
}

/// Appends as much of `text` as fits below [`MAX_LOG_LINE`] bytes.
pub(crate) fn push_capped(out: &mut Vec<u8>, text: &[u8]) {
    let room = MAX_LOG_LINE.saturating_sub(out.len());
    out.extend_from_slice(text.get(..room).unwrap_or(text));
}

pub fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
