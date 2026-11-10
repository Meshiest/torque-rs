//! The Torque3D executor: the opcode loop of Torque3D's instruction set (the DSO version 50 files
//! torque-ts-compiler writes), on the runtime's public executor interface ([`crate::exec`]).
//!
//! The semantics follow Torque3D's MIT-licensed interpreter: `compiledEval.cpp` (Copyright (c) 2013 GarageGames, LLC;
//! Copyright (c) 2015 Faust Logic, Inc.; Copyright (c) 2021 TGEMIT Authors & Contributors), `codeBlock.cpp`
//! (Copyright (c) 2012 GarageGames, LLC) and `evalState.cpp`; the code here is this crate's own.
//!
//! **Values.** Every operation works on one stack of typed values (an integer, a float or a string),
//! converting an operand to the type it needs. Function locals live in registers (one array per running
//! block, sized by the function's declaration; a top level and a default-value expression grow theirs as they
//! use them); globals and indexed locals (`%a[1]`) are the runtime's variables. A call collects its arguments on
//! a call stack of typed values; a script function receives them typed in its registers and returns a typed
//! value (`RETURN_UINT`, `RETURN_FLT`), a native receives their text and its typed result is pushed as it is
//! (or dropped when the next operation is `POP_STK`).
//!
//! **Frames.** Every block runs in a frame of its own: a function's, a file's top level, evaluated text and a
//! default-value expression each get fresh registers and a fresh local-variable table, so evaluated text does
//! not see the locals of the function that evaluated it. What the loop keeps per frame (the object and field
//! the field operations act on, the object a method call made current, the objects being declared, the `foreach`
//! loops) lives in the executor's state, never in the Rust frames on the recursion path.
//!
//! **Where this differs from Torque3D on purpose.** Hostile or broken code is an error (`VmError`), where Torque3D
//! reads or writes past its arrays: a register outside the frame, a stack underflow, an object operation with no
//! declaration open. An integer operation on a float value converts it (Torque3D's fast path reads the integer
//! field a float value does not set). Registers are bounded (4096 a frame, 65536 in all frames) and charged to the
//! step budget as they are made; a field component has at most four letters, and its reads and writes stay within
//! Torque3D's buffers and are charged. A failed `assert` logs its error line and the run goes on (Torque3D then asks
//! the user whether to stop). Globals and indexed locals keep the runtime's variable storage, whose float text has
//! six significant digits; values on the stack and in registers print with nine, as Torque3D's.

mod compiler;
mod natives;
mod value;

pub use compiler::Torque3dCompiler;
pub use natives::install;

use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use torque_ts_bytecode::opcodes::Op;

use self::value::{Value, is_space};
use crate::code::CodeBlock;
use crate::exec::{
    Executor, RunMark, RunMode, ScriptCall, argc_ok, fetch, native_call, script_name, script_target,
};
use crate::memory::MemoryCharge;
use crate::ns::{Callable, EntryRef};
use crate::sim::ObjId;
use crate::tags::tag_text;
use crate::text::{atof, atoi, to_f32, to_f64, until_nul};
use crate::vm::words::{WORDS, set_unit, unit, unit_count};
use crate::vm::{
    AddedObject, Declaration, FieldTarget, FrameMark, MAX_STACK, MAX_STRING_BYTES, Native,
    SuspendedObject, Vm, VmError, lossy,
};

/// The most values the call stack holds (all frames' arguments together).
const MAX_CALL_VALUES: usize = 4096;
/// The most registers one frame has, and all frames together. Each register a frame makes is charged as one work
/// unit.
const MAX_REGISTERS: usize = 4096;
const MAX_TOTAL_REGISTERS: usize = 1 << 16;
/// The most letters a field component has (`.x` to `.xyzw`); a longer field name is no component.
const MAX_COMPONENT_LETTERS: usize = 4;
/// The most `foreach` loops and nested declarations open at once (all frames together).
const MAX_NESTING: usize = MAX_STACK;
/// The longest array index a field operation keeps (Torque3D's buffer of 256 bytes).
const MAX_FIELD_ARRAY: usize = 255;
/// The text a field component read can give and a component write can build (Torque3D's buffers of 128 and 1024
/// bytes, less their NUL).
const COMPONENT_NUMBER: usize = 127;
const COMPONENT_WRITE: usize = 1023;
/// The first byte of a tagged string's text.
const TAG_PREFIX: u8 = 1;
/// The digits of a tag id written over a tagged literal (Torque3D's buffer of 7 bytes, less its NUL).
const TAG_DIGITS: usize = 6;

/// The `callType` operand of `OP_CALLFUNC`.
const CALL_FUNCTION: u32 = 0;
const CALL_STATIC: u32 = 1;
const CALL_METHOD: u32 = 2;
const CALL_PARENT: u32 = 3;

/// Where the current variable lives; its folded name is `State::cur_key`, its slot `cur_slot` while its table's stamp
/// is `cur_stamp` (see [`Dictionary`](crate::vars::Dictionary)).
#[derive(Clone, Copy)]
enum VarRef {
    Global,
    Local(usize),
}

/// What a block running in a frame is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A script function's body: its arguments are bound first.
    Function,
    /// A file's top level or evaluated text.
    TopLevel,
    /// The expression of a parameter's default value, ended by `OP_DEFAULT_END`.
    DefaultValue,
}

/// The variable a `foreach` loop sets.
enum IterVar {
    Global(Vec<u8>),
    Register(u32),
}

/// What a `foreach` loop walks: the words of a text (`foreach$`) or the members of a set.
enum IterOver {
    /// The text, which counts in the memory account while the loop runs.
    Words {
        text: Vec<u8>,
        at: usize,
        _held: MemoryCharge,
    },
    Set {
        set: ObjId,
        index: usize,
    },
}

struct Iter {
    var: IterVar,
    over: IterOver,
}

/// The loop's state of one running block (Torque3D keeps it in the locals of its interpreter function).
struct Frame {
    /// Where the frame's registers start in `State::registers` (they run to its end: only the innermost frame
    /// uses its registers).
    regs: usize,
    /// The frame makes registers as its code uses them (its count is in no declaration).
    growable: bool,
    /// The lengths of `State::iters` and `State::creation` when the frame started.
    iters: usize,
    creation: usize,
    /// The object of the last method call this frame made (Torque3D's `thisObject`), handed to the natives it calls.
    this_object: Option<ObjId>,
    /// The object and field the field operations act on, and the ones before (for a field component).
    cur_object: FieldTarget,
    prev_object: FieldTarget,
    cur_field: Option<Vec<u8>>,
    prev_field: Option<Vec<u8>>,
    /// The register a local-variable operation last named (a field component reads and writes it).
    current_register: Option<u32>,
    /// The namespace a wrong-argument-count warning names: the one the last lookup of this frame went through.
    ns_label: Vec<u8>,
}

/// The executor's state (see the module documentation).
#[derive(Default)]
struct State {
    stack: Vec<Value>,
    /// The call stack: argument values, and where each open argument list starts.
    args: Vec<Value>,
    arg_frames: Vec<usize>,
    registers: Vec<Value>,
    frames: Vec<Frame>,
    iters: Vec<Iter>,
    /// Declarations set aside while a nested one runs (see `Vm::suspend_new_object`).
    creation: Vec<SuspendedObject>,
    cur_var: Option<VarRef>,
    cur_key: Vec<u8>,
    cur_slot: usize,
    cur_stamp: u64,
    /// The array index of the current field and of the one before (shared by all frames, as Torque3D's).
    cur_field_array: Vec<u8>,
    prev_field_array: Vec<u8>,
    /// A buffer for the value a field write copies, kept between writes.
    field_value: Vec<u8>,
    /// The value whose text the last call or object lookup left (Torque3D's return variable), for field components:
    /// kept as the value and converted only when a component reads it (the conversion gives the same text later).
    ret: Value,
    /// What the last run started from Rust returned.
    result: Value,
    /// Bytes of string values made since the last instruction (the step budget's work).
    work: u64,
}

impl State {
    fn push(&mut self, v: Value) -> Result<(), VmError> {
        if self.stack.len() >= MAX_STACK {
            return Err(VmError::StackOverflow);
        }
        self.work = self.work.saturating_add(v.text_len() as u64);
        self.stack.push(v);
        Ok(())
    }

    fn pop(&mut self) -> Result<Value, VmError> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
    }

    /// Counts `bytes` of text copied or scanned against the step budget.
    fn charge(&mut self, bytes: usize) {
        self.work = self.work.saturating_add(bytes as u64);
    }

    /// The top value, its text's bytes charged: for an operation that copies or scans the value it leaves on the stack (a
    /// save, an `_NP` jump), so that one long string read again and again costs its bytes each time.
    fn top_charged(&mut self) -> Result<&mut Value, VmError> {
        let len = self.stack.last().ok_or(VmError::StackUnderflow)?.text_len();
        self.charge(len);
        self.top()
    }

    fn top(&mut self) -> Result<&mut Value, VmError> {
        self.stack.last_mut().ok_or(VmError::StackUnderflow)
    }

    fn top_two(&mut self) -> Result<(Value, &mut Value), VmError> {
        let top = self.pop()?;
        let second = self.top()?;
        Ok((top, second))
    }

    fn frame(&mut self) -> &mut Frame {
        // A frame is pushed before any instruction runs.
        self.frames
            .last_mut()
            .expect("an instruction runs inside a frame")
    }

    /// Register `reg` of the innermost frame.
    fn register(&mut self, reg: u32, ip: usize) -> Result<&mut Value, VmError> {
        let frame = self.frames.last().ok_or(VmError::BadIndex { ip })?;
        let (base, growable) = (frame.regs, frame.growable);
        let at = base.saturating_add(reg as usize);
        if at >= self.registers.len() {
            if !growable || reg as usize >= MAX_REGISTERS {
                return Err(VmError::BadIndex { ip });
            }
            if at >= MAX_TOTAL_REGISTERS {
                return Err(VmError::StackOverflow);
            }
            self.work = self
                .work
                .saturating_add((at + 1 - self.registers.len()) as u64);
            self.registers.resize(at + 1, Value::default());
        }
        Ok(&mut self.registers[at])
    }

    /// Marks a local-variable operation on `reg`: no field component, no object (Torque3D clears them so the
    /// next field operation reads the register).
    fn select_register(&mut self, reg: Option<u32>) {
        self.clear_ret();
        let frame = self.frame();
        frame.current_register = reg;
        frame.prev_field = None;
        frame.prev_object = FieldTarget::None;
        frame.cur_object = FieldTarget::None;
    }

    /// Empties the return variable.
    fn clear_ret(&mut self) {
        match &mut self.ret {
            Value::Str(text) => text.clear(),
            other => *other = Value::default(),
        }
    }

    /// The arguments of the innermost open argument list, `name` first.
    fn take_args(&mut self, name: &[u8]) -> Result<Vec<Value>, VmError> {
        let start = self.arg_frames.pop().ok_or(VmError::StackUnderflow)?;
        let start = start.min(self.args.len());
        let mut argv = Vec::with_capacity(self.args.len() - start + 1);
        argv.push(Value::text(name));
        argv.extend(self.args.drain(start..));
        Ok(argv)
    }

    /// The text of `a` then `b`, charged; `StringTooLong` past the size cap.
    fn concat(&mut self, a: &Value, b: &[u8]) -> Result<Value, VmError> {
        let mut text = a.string().into_owned();
        if text.len().saturating_add(b.len()) > MAX_STRING_BYTES {
            return Err(VmError::StringTooLong);
        }
        text.extend_from_slice(b);
        Ok(Value::Str(text))
    }
}

/// The Torque3D executor (see the module documentation). One per [`Vm`].
#[derive(Default)]
pub struct Torque3dExecutor {
    state: RefCell<State>,
}

impl Torque3dExecutor {
    pub fn new() -> Torque3dExecutor {
        Torque3dExecutor::default()
    }

    fn st(&self) -> RefMut<'_, State> {
        self.state.borrow_mut()
    }
}

/// What a run started from Rust puts back after an error (see [`Executor::save`]).
#[derive(Clone, Copy, Debug)]
pub struct Torque3dMark {
    stack: usize,
    args: usize,
    arg_frames: usize,
}

impl Executor for Torque3dExecutor {
    fn run(
        &self,
        vm: &mut Vm,
        block: &Rc<CodeBlock>,
        start: usize,
        call: Option<ScriptCall>,
        mode: RunMode,
    ) -> Result<(), VmError> {
        let (args, kind) = match &call {
            Some(call) => (
                call.argv.iter().map(|a| Value::text(a)).collect(),
                Kind::Function,
            ),
            None => (Vec::new(), Kind::TopLevel),
        };
        let result = Cx { ex: self, vm }.exec(block, start, call, args, (mode, kind))?;
        self.st().result = result;
        Ok(())
    }

    fn result(&self) -> Vec<u8> {
        self.st().result.string().into_owned()
    }

    fn set_result(&self, text: &[u8]) {
        self.st().result = Value::text(text);
    }

    fn save(&self) -> RunMark {
        let st = self.st();
        RunMark::Torque3d(Torque3dMark {
            stack: st.stack.len(),
            args: st.args.len(),
            arg_frames: st.arg_frames.len(),
        })
    }

    fn restore(&self, saved: RunMark, unwind: bool) {
        let RunMark::Torque3d(mark) = saved else {
            return;
        };
        if unwind {
            let mut st = self.st();
            st.stack.truncate(mark.stack);
            st.args.truncate(mark.args);
            st.arg_frames.truncate(mark.arg_frames);
        }
    }

    fn take_work(&self) -> u64 {
        std::mem::take(&mut self.st().work)
    }

    fn take_overflow(&self) -> bool {
        // Values past the size cap are refused as they are made (`StringTooLong`), never cut.
        false
    }
}

/// A run of the loop: the executor and the `Vm` it runs on. Script-to-script calls recurse inside one `Cx`; a
/// native that runs script again goes through the `Vm` and gets a `Cx` of its own.
struct Cx<'a> {
    ex: &'a Torque3dExecutor,
    vm: &'a mut Vm,
}

impl<'a> Cx<'a> {
    fn st(&self) -> RefMut<'a, State> {
        self.ex.state.borrow_mut()
    }

    // ---- frames ----

    /// Runs `block` from `ip` in a frame of its own and returns its result. `args` are a function's arguments
    /// (its name first).
    ///
    /// **Stack.** This frame, `exec_loop`'s and `call_function`'s are on the recursion path of every script call
    /// (multiplied by the call depth): each keeps only what it must and leaves the rest to out-of-line helpers.
    fn exec(
        &mut self,
        block: &Rc<CodeBlock>,
        ip: usize,
        call: Option<ScriptCall>,
        args: Vec<Value>,
        (mode, kind): (RunMode, Kind),
    ) -> Result<Value, VmError> {
        let mark = self.open_frame(block, ip, call.as_ref(), (mode, kind))?;
        let result = self.exec_loop(block, ip, call.as_ref(), args, (mode, kind));
        self.close_frame(mark);
        result
    }

    /// A block starts: the call-depth check, the runtime's frame (see [`Vm::enter_frame`]) and the executor's,
    /// with a function's registers.
    #[inline(never)]
    fn open_frame(
        &mut self,
        block: &Rc<CodeBlock>,
        ip: usize,
        call: Option<&ScriptCall>,
        (mode, kind): (RunMode, Kind),
    ) -> Result<FrameMark, VmError> {
        self.vm.check_call_depth()?;
        let registers = match kind {
            Kind::Function => Some(fetch(block.words(), ip + 9)? as usize),
            Kind::TopLevel | Kind::DefaultValue => None,
        };
        let count = registers.unwrap_or(0);
        if count > MAX_REGISTERS {
            return Err(VmError::BadIndex { ip: ip + 9 });
        }
        if self.st().registers.len().saturating_add(count) > MAX_TOTAL_REGISTERS {
            return Err(VmError::StackOverflow);
        }
        let mark = self.vm.enter_frame(block, call, mode.with_own_frame());
        let mut st = self.st();
        let regs = st.registers.len();
        st.charge(count);
        st.registers.resize(regs + count, Value::default());
        let (iters, creation) = (st.iters.len(), st.creation.len());
        st.frames.push(Frame {
            regs,
            growable: registers.is_none(),
            iters,
            creation,
            this_object: None,
            cur_object: FieldTarget::None,
            prev_object: FieldTarget::None,
            cur_field: None,
            prev_field: None,
            current_register: None,
            ns_label: Vec::new(),
        });
        st.cur_var = None;
        Ok(mark)
    }

    /// The block that [`open_frame`](Self::open_frame) started ends.
    #[inline(never)]
    fn close_frame(&mut self, mark: FrameMark) {
        {
            let mut st = self.st();
            if let Some(frame) = st.frames.pop() {
                st.registers.truncate(frame.regs);
                st.iters.truncate(frame.iters);
                st.creation.truncate(frame.creation);
            }
            st.cur_var = None;
        }
        self.vm.leave_frame(mark);
    }

    /// Puts a function's arguments in its registers, runs the default value of each declared parameter the call
    /// did not pass (one with no default stays empty), and returns where the body starts. `ip` is the
    /// declaration's first operand: name, namespace, package (two words each), flags, end, argc, register count,
    /// then per parameter its register, its flags (bit 0: it has a default value) and its default's code index.
    #[inline(never)]
    fn bind_params(
        &mut self,
        block: &Rc<CodeBlock>,
        ip: usize,
        args: Vec<Value>,
    ) -> Result<usize, VmError> {
        let code = block.words();
        let declared = fetch(code, ip + 8)? as usize;
        // Each parameter is a register, so a count past the register cap is a broken file; the rounds are charged.
        if declared > MAX_REGISTERS {
            return Err(VmError::BadIndex { ip: ip + 8 });
        }
        self.st().charge(declared);
        let passed = args.len().saturating_sub(1).min(declared);
        for (i, value) in args.into_iter().skip(1).take(passed).enumerate() {
            let reg = fetch(code, ip + 10 + i)?;
            *self.st().register(reg, ip)? = value;
        }
        for i in passed..declared {
            let reg = fetch(code, ip + 10 + i)?;
            let flags = fetch(code, ip + 10 + declared + i)?;
            let start = fetch(code, ip + 10 + 2 * declared + i)? as usize;
            if flags & 1 == 0 || start == 0 {
                continue;
            }
            let value = self.exec(
                block,
                start,
                None,
                Vec::new(),
                (RunMode::PLAIN, Kind::DefaultValue),
            )?;
            *self.st().register(reg, ip)? = value;
        }
        declared
            .checked_mul(3)
            .and_then(|n| n.checked_add(ip + 10))
            .ok_or(VmError::CodeOutOfRange { ip })
    }

    // ---- the loop ----

    /// The evaluation loop: one arm per group of opcodes, each a call that returns the next instruction.
    fn exec_loop(
        &mut self,
        block: &Rc<CodeBlock>,
        mut ip: usize,
        call: Option<&ScriptCall>,
        args: Vec<Value>,
        (mode, kind): (RunMode, Kind),
    ) -> Result<Value, VmError> {
        if kind == Kind::Function {
            ip = self.bind_params(block, ip, args)?;
        }
        let function = kind != Kind::TopLevel;
        loop {
            let op = self.fetch_op(block, ip)?;
            ip = match op {
                Op::CallFunc => self.call_function(block, ip + 1, call, (mode, kind))?,
                Op::AddObject => self.add_object(block, ip + 1)?,
                Op::DefaultEnd | Op::Return | Op::ReturnVoid | Op::ReturnFlt | Op::ReturnUint => {
                    return self.finish(op);
                }
                Op::FuncDecl => self.declare_function(block, ip + 1, mode)?,
                Op::CreateObject
                | Op::EndObject
                | Op::FinishObject
                | Op::SetCurObject
                | Op::SetCurObjectNew
                | Op::SetCurObjectInternal
                | Op::SetCurField
                | Op::SetCurFieldArray
                | Op::SetCurFieldType
                | Op::LoadFieldUint
                | Op::LoadFieldFlt
                | Op::LoadFieldStr
                | Op::SaveFieldUint
                | Op::SaveFieldFlt
                | Op::SaveFieldStr => self.object_op(block, op, ip + 1, mode)?,
                Op::IterBegin | Op::IterBeginStr | Op::Iter | Op::IterEnd => {
                    self.iterate(block, op, ip + 1)?
                }
                _ => self.plain_op(block, op, ip + 1, function)?,
            };
        }
    }

    /// Counts the step (and the work of the last one), reads the opcode word at `ip` and decodes it.
    #[inline(never)]
    fn fetch_op(&mut self, block: &CodeBlock, ip: usize) -> Result<Op, VmError> {
        let work = std::mem::take(&mut self.st().work);
        self.vm.charge_instruction(work)?;
        let word = fetch(block.words(), ip)?;
        Op::ALL
            .get(word as usize)
            .copied()
            .ok_or(VmError::InvalidOpcode { ip, word })
    }

    /// The returns: the value, after the open `foreach` loops of the frame are closed (each leaves its value
    /// on the stack).
    #[inline(never)]
    fn finish(&mut self, op: Op) -> Result<Value, VmError> {
        let mut st = self.st();
        let value = match op {
            Op::ReturnVoid => Value::default(),
            Op::ReturnFlt => Value::Float(st.pop()?.float()),
            Op::ReturnUint => Value::Int(st.pop()?.int()),
            _ => st.pop()?,
        };
        let base = st.frame().iters;
        while st.iters.len() > base {
            st.iters.pop();
            st.pop()?;
        }
        Ok(value)
    }

    /// Everything that only touches the stacks, the registers and the variables; returns the next `ip`.
    #[inline(never)]
    fn plain_op(
        &mut self,
        block: &CodeBlock,
        op: Op,
        ip: usize,
        function: bool,
    ) -> Result<usize, VmError> {
        let code = block.words();
        match op {
            Op::Jmp
            | Op::JmpIf
            | Op::JmpIfNot
            | Op::JmpIfF
            | Op::JmpIfFNot
            | Op::JmpNotString
            | Op::JmpIfNp
            | Op::JmpIfNotNp => {
                let target = fetch(code, ip)? as usize;
                Ok(if self.jump_taken(op)? { target } else { ip + 1 })
            }
            Op::CmpEq
            | Op::CmpGr
            | Op::CmpGe
            | Op::CmpLt
            | Op::CmpLe
            | Op::CmpNe
            | Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div => {
                self.float_binary(op)?;
                Ok(ip)
            }
            Op::Xor | Op::BitAnd | Op::BitOr | Op::Shl | Op::Shr | Op::And | Op::Or | Op::Mod => {
                self.int_binary(op)?;
                Ok(ip)
            }
            Op::Not | Op::NotF | Op::OnesComplement | Op::Neg => {
                let mut st = self.st();
                let top = st.top()?;
                *top = match op {
                    Op::Not => Value::from_bool(top.int() == 0),
                    Op::NotF => Value::from_bool(top.float() == 0.0),
                    Op::OnesComplement => Value::Int(!top.int()),
                    _ => Value::Float(-top.float()),
                };
                Ok(ip)
            }
            Op::Inc => {
                let reg = fetch(code, ip)?;
                let mut st = self.st();
                st.frame().current_register = Some(reg);
                let slot = st.register(reg, ip)?;
                *slot = Value::Float(slot.float() + 1.0);
                Ok(ip + 1)
            }
            Op::SetCurVar | Op::SetCurVarCreate => {
                let name = block.ident(fetch(code, ip)?).unwrap_or(b"");
                self.set_cur_var(name, op == Op::SetCurVarCreate);
                Ok(ip + 2)
            }
            Op::SetCurVarArray | Op::SetCurVarArrayCreate => {
                let name = self.st().top_charged()?.string().into_owned();
                self.set_cur_var(&name, op == Op::SetCurVarArrayCreate);
                Ok(ip)
            }
            Op::LoadVarUint | Op::LoadVarFlt | Op::LoadVarStr => {
                self.load_var(op)?;
                Ok(ip)
            }
            Op::SaveVarUint | Op::SaveVarFlt | Op::SaveVarStr => {
                self.save_var(op)?;
                Ok(ip)
            }
            Op::LoadLocalVarUint
            | Op::LoadLocalVarFlt
            | Op::LoadLocalVarStr
            | Op::SaveLocalVarUint
            | Op::SaveLocalVarFlt
            | Op::SaveLocalVarStr => {
                self.local_var(op, fetch(code, ip)?, ip)?;
                Ok(ip + 1)
            }
            Op::PopStk => {
                self.st().pop()?;
                Ok(ip)
            }
            Op::LoadImmedUint => {
                let v = fetch(code, ip)?;
                self.st().push(Value::Int(i64::from(v)))?;
                Ok(ip + 1)
            }
            Op::LoadImmedFlt => {
                let index = fetch(code, ip)? as usize;
                let v = *block
                    .floats(function)
                    .get(index)
                    .ok_or(VmError::BadIndex { ip })?;
                self.st().push(Value::Float(v))?;
                Ok(ip + 1)
            }
            Op::LoadImmedStr => {
                let text = string_at(block, function, fetch(code, ip)?, ip)?;
                self.st().push(Value::text(text))?;
                Ok(ip + 1)
            }
            Op::TagToStr => {
                self.tag_to_str(block, function, fetch(code, ip)?, ip)?;
                Ok(ip + 1)
            }
            Op::DocblockStr => {
                // Documentation for the console's help: nothing runs.
                fetch(code, ip)?;
                Ok(ip + 1)
            }
            Op::LoadImmedIdent => {
                let name = block.ident(fetch(code, ip)?).unwrap_or(b"");
                self.st().push(Value::text(name))?;
                Ok(ip + 2)
            }
            Op::AdvanceStrAppendChar => {
                let ch = fetch(code, ip)? as u8;
                let mut st = self.st();
                let top = st.pop()?;
                let joined = st.concat(&top, &[ch])?;
                st.push(joined)?;
                Ok(ip + 1)
            }
            Op::RewindStr | Op::TerminateRewindStr => {
                let mut st = self.st();
                let top = st.pop()?;
                let second = st.pop()?;
                let joined = top.with_string(|b| st.concat(&second, b))?;
                st.push(joined)?;
                Ok(ip)
            }
            Op::CompareStr => {
                let mut st = self.st();
                let (top, second) = st.top_two()?;
                let equal = top.with_string(|a| second.with_string(|b| a.eq_ignore_ascii_case(b)));
                *second = Value::from_bool(equal);
                Ok(ip)
            }
            Op::Push => {
                let mut st = self.st();
                if st.args.len() >= MAX_CALL_VALUES {
                    return Err(VmError::StackOverflow);
                }
                let v = st.pop()?;
                st.args.push(v);
                Ok(ip)
            }
            Op::PushFrame => {
                fetch(code, ip)?;
                let mut st = self.st();
                if st.arg_frames.len() >= MAX_CALL_VALUES {
                    return Err(VmError::StackOverflow);
                }
                let start = st.args.len();
                st.arg_frames.push(start);
                Ok(ip + 1)
            }
            Op::Assert => {
                let message = fetch(code, ip)?;
                if !self.st().pop()?.truth() {
                    self.assert_failed(block, function, message, ip - 1)?;
                }
                Ok(ip + 1)
            }
            _ => Err(VmError::Unsupported(op.name())),
        }
    }

    /// The jumps' conditions. The `_NP` forms keep the value when they jump and pop it when they go on.
    fn jump_taken(&mut self, op: Op) -> Result<bool, VmError> {
        let mut st = self.st();
        Ok(match op {
            Op::Jmp => true,
            Op::JmpIfFNot => st.pop()?.float() == 0.0,
            Op::JmpIfNot => st.pop()?.int() == 0,
            Op::JmpNotString => !st.pop()?.truth(),
            // Both read the value as a float (`JMPIF` too, as Torque3D's does).
            Op::JmpIfF | Op::JmpIf => st.pop()?.float() != 0.0,
            Op::JmpIfNotNp => {
                let taken = st.top_charged()?.int() == 0;
                if !taken {
                    st.pop()?;
                }
                taken
            }
            _ => {
                let taken = st.top_charged()?.int() != 0;
                if !taken {
                    st.pop()?;
                }
                taken
            }
        })
    }

    /// The float operations and comparisons: the top value is the left operand (the compiler pushes the right
    /// one first), the result replaces the second.
    fn float_binary(&mut self, op: Op) -> Result<(), VmError> {
        let mut st = self.st();
        let (top, second) = st.top_two()?;
        let (a, b) = (top.float(), second.float());
        *second = match op {
            Op::Add => Value::Float(a + b),
            Op::Sub => Value::Float(a - b),
            Op::Mul => Value::Float(a * b),
            Op::Div => Value::Float(a / b),
            Op::CmpEq => Value::from_bool(a == b),
            Op::CmpNe => Value::from_bool(a != b),
            Op::CmpLt => Value::from_bool(a < b),
            Op::CmpLe => Value::from_bool(a <= b),
            Op::CmpGr => Value::from_bool(a > b),
            _ => Value::from_bool(a >= b),
        };
        Ok(())
    }

    /// The integer operations (64-bit), the top value the left operand. A remainder by 0 is 0; a shift count
    /// is taken modulo 64.
    fn int_binary(&mut self, op: Op) -> Result<(), VmError> {
        let mut st = self.st();
        let (top, second) = st.top_two()?;
        let (a, b) = (top.int(), second.int());
        *second = match op {
            Op::Xor => Value::Int(a ^ b),
            Op::BitAnd => Value::Int(a & b),
            Op::BitOr => Value::Int(a | b),
            Op::Shl => Value::Int(a.wrapping_shl(b as u32)),
            Op::Shr => Value::Int(a.wrapping_shr(b as u32)),
            Op::And => Value::from_bool(a != 0 && b != 0),
            Op::Or => Value::from_bool(a != 0 || b != 0),
            _ if b == 0 => Value::Int(0),
            _ => Value::Int(a.wrapping_rem(b)),
        };
        Ok(())
    }

    /// `OP_TAG_TO_STR`: the first time a block's tagged literal runs in this `Vm`, its text enters the tag table
    /// and the literal becomes `"\x01<id>"` for good (a literal that already starts with that byte stays as it
    /// is); then it loads like `OP_LOADIMMED_STR`. The copy into the table is charged by its length.
    #[inline(never)]
    fn tag_to_str(
        &mut self,
        block: &CodeBlock,
        function: bool,
        offset: u32,
        ip: usize,
    ) -> Result<(), VmError> {
        let vm_id = self.vm.vm_id();
        if let Some(text) = block.tag_conversion(function, offset, vm_id) {
            return self.st().push(Value::Str(text));
        }
        let literal = until_nul(string_at(block, function, offset, ip)?);
        self.vm.objects_mut().add_work(literal.len() as u64);
        let text = if literal.first() == Some(&TAG_PREFIX) {
            literal.to_vec()
        } else {
            let id = self
                .vm
                .tags_mut()
                .add_string(literal)
                .ok_or(VmError::TagTableFull)?;
            tag_text(id, TAG_DIGITS)
        };
        block.set_tag_conversion(function, offset, vm_id, text.clone());
        self.st().push(Value::Str(text))
    }

    /// A failed `assert`: Torque3D's error line (the file, `eval` for evaluated text, and the line), then the run
    /// goes on.
    #[cold]
    #[inline(never)]
    fn assert_failed(
        &mut self,
        block: &CodeBlock,
        function: bool,
        message: u32,
        op_ip: usize,
    ) -> Result<(), VmError> {
        let text = until_nul(string_at(block, function, message, op_ip)?).to_vec();
        let file = block.name().map_or_else(|| "eval".to_string(), lossy);
        self.vm.error(format!(
            "{file}({},0): {{Fatal}} - {}",
            block.line_at(op_ip),
            lossy(&text)
        ));
        Ok(())
    }

    // ---- variables ----

    /// `OP_SETCURVAR` and friends: selects (and with `create` makes) the variable `name`: a global, or a local of
    /// the frame's table. The field state is cleared (the next field operation is not a component of it).
    #[inline(never)]
    fn set_cur_var(&mut self, name: &[u8], create: bool) {
        let mut st = self.st();
        let st = &mut *st;
        st.select_register(None);
        crate::vars::refill(&mut st.cur_key, until_nul(name));
        st.cur_key.make_ascii_lowercase();
        let key = &st.cur_key;
        let (var, table) = if name.first() == Some(&b'$') {
            (VarRef::Global, Some(self.vm.global_table_mut()))
        } else {
            match self.vm.local_frames().len().checked_sub(1) {
                Some(frame) => (
                    VarRef::Local(frame),
                    self.vm.local_frames_mut().get_mut(frame),
                ),
                None => (VarRef::Global, None),
            }
        };
        st.cur_var = table.and_then(|table| {
            let slot = if create {
                table.add_slot_folded(key, name)
            } else {
                table.slot_folded(key)
            }?;
            st.cur_slot = slot;
            st.cur_stamp = table.stamp();
            Some(var)
        });
    }

    /// Runs `f` on the current variable, if there is one.
    fn with_var<R>(&mut self, f: impl FnOnce(&mut crate::vars::Entry) -> R) -> Option<R> {
        let st = self.st();
        let table = match st.cur_var? {
            VarRef::Global => self.vm.global_table_mut(),
            VarRef::Local(frame) => self.vm.local_frames_mut().get_mut(frame)?,
        };
        let entry = if table.stamp() == st.cur_stamp {
            table.at_mut(st.cur_slot)
        } else {
            table.get_mut_folded(&st.cur_key)
        }?;
        Some(f(entry))
    }

    /// `LOADVAR_*`: the current variable as an integer (its 32-bit view, signed), a float or text; 0, 0 or ""
    /// without one.
    #[inline(never)]
    fn load_var(&mut self, op: Op) -> Result<(), VmError> {
        self.st().frame().current_register = None;
        let value = match op {
            Op::LoadVarUint => {
                Value::Int(i64::from(self.with_var(|e| e.get_int()).unwrap_or(0) as i32))
            }
            Op::LoadVarFlt => Value::Float(self.with_var(|e| e.get_float()).unwrap_or(0.0)),
            _ => Value::Str(self.with_var(|e| e.get_string()).unwrap_or_default()),
        };
        self.st().push(value)
    }

    /// `SAVEVAR_*`: the top value (left there) into the current variable, as an unsigned 32-bit integer, a
    /// float or text. Nothing without a variable.
    #[inline(never)]
    fn save_var(&mut self, op: Op) -> Result<(), VmError> {
        let value = self.st().top_charged()?.clone();
        self.with_var(|e| match op {
            Op::SaveVarUint => e.set_int(value.int() as u32),
            Op::SaveVarFlt => e.set_float(value.float()),
            _ => value.with_string(|text| e.set_string(text)),
        });
        Ok(())
    }

    /// The register operations: a load pushes the register as a 32-bit integer, a float or text; a save puts
    /// the top value (left there) in it as a 64-bit integer, a float or text.
    #[inline(never)]
    fn local_var(&mut self, op: Op, reg: u32, ip: usize) -> Result<(), VmError> {
        let mut st = self.st();
        st.select_register(Some(reg));
        match op {
            Op::LoadLocalVarUint | Op::LoadLocalVarFlt | Op::LoadLocalVarStr => {
                let slot = st.register(reg, ip)?;
                let value = match op {
                    Op::LoadLocalVarUint => Value::Int(i64::from(slot.int() as i32)),
                    Op::LoadLocalVarFlt => Value::Float(slot.float()),
                    _ => Value::Str(slot.string().into_owned()),
                };
                st.push(value)
            }
            _ => {
                let top = st.top()?;
                let value = match op {
                    Op::SaveLocalVarUint => Value::Int(top.int()),
                    Op::SaveLocalVarFlt => Value::Float(top.float()),
                    _ => Value::Str(top.string().into_owned()),
                };
                st.charge(value.text_len());
                *st.register(reg, ip)? = value;
                Ok(())
            }
        }
    }

    // ---- functions ----

    /// `OP_FUNC_DECL`: declares the function (unless the run makes no calls) and returns the index past its
    /// body and default values. A declaration replaces whatever the layer held under the name, a native included.
    #[inline(never)]
    fn declare_function(
        &mut self,
        block: &Rc<CodeBlock>,
        ip: usize,
        mode: RunMode,
    ) -> Result<usize, VmError> {
        let code = block.words();
        let end = fetch(code, ip + 7)? as usize;
        if mode.is_no_calls() {
            return Ok(end);
        }
        let ident = |word: u32| match block.ident(word) {
            None if word != 0 => Err(VmError::BadIndex { ip }),
            found => Ok(found.map(<[u8]>::to_vec)),
        };
        let name = ident(fetch(code, ip)?)?.unwrap_or_default();
        let namespace = ident(fetch(code, ip + 2)?)?.unwrap_or_default();
        let package = ident(fetch(code, ip + 4)?)?.or_else(|| {
            self.vm
                .namespaces()
                .declare_package_for(block.name())
                .map(<[u8]>::to_vec)
        });
        let has_body = fetch(code, ip + 6)? & 1 != 0;
        let label = if namespace.is_empty() {
            b"(null)".to_vec()
        } else {
            namespace.clone()
        };
        self.st().frame().ns_label = label;
        self.vm
            .namespaces_mut()
            .touch(&namespace, package.as_deref());
        if self.vm.may_declare(&namespace, &name, mode.is_trusted()) {
            self.vm.namespaces_mut().declare(
                &namespace,
                package.as_deref(),
                &name,
                Callable::Script {
                    block: Rc::clone(block),
                    offset: if has_body { ip } else { 0 },
                },
            );
        }
        Ok(end)
    }

    /// `OP_CALLFUNC`: the name, the namespace (two words each) and the call type; the arguments are the open
    /// argument list. The lookup and its warnings come first (a call that finds nothing pushes ""), then a
    /// script function runs in a frame of its own, or a native gets the arguments' text. `ip` is the first
    /// operand; returns the next instruction.
    #[inline(never)]
    fn call_function(
        &mut self,
        block: &Rc<CodeBlock>,
        ip: usize,
        running: Option<&ScriptCall>,
        (mode, kind): (RunMode, Kind),
    ) -> Result<usize, VmError> {
        let (next, pending) = self.prepare_call(block, ip, running, (mode, kind))?;
        if let Some(pending) = pending {
            let Pending {
                target,
                offset,
                call,
                argv,
            } = *pending;
            let value = self.exec(
                &target,
                offset,
                Some(call),
                argv,
                (RunMode::PLAIN, Kind::Function),
            )?;
            self.pushed_result(value)?;
            self.st().cur_var = None;
        }
        Ok(next)
    }

    /// Everything of a call but running a script function: the lookup, a native's call, the result of a call that
    /// runs nothing. Returns the next instruction and the script function to run, if any.
    #[inline(never)]
    fn prepare_call(
        &mut self,
        block: &Rc<CodeBlock>,
        ip: usize,
        running: Option<&ScriptCall>,
        (mode, kind): (RunMode, Kind),
    ) -> Result<(usize, Option<Box<Pending>>), VmError> {
        let op_ip = ip - 1;
        let code = block.words();
        let name = block.ident(fetch(code, ip)?).unwrap_or(b"");
        let namespace = block.ident(fetch(code, ip + 2)?);
        let call_type = fetch(code, ip + 4)?;
        let mut next = ip + 5;
        let argv = self.st().take_args(name)?;
        let found = self.resolve(block, op_ip, (name, namespace, call_type), &argv, running)?;
        let entry = match found {
            Resolved::Entry(entry) if !mode.is_no_calls() => entry,
            Resolved::Entry(_) | Resolved::Warned => {
                self.pushed_missing()?;
                return Ok((next, None));
            }
            Resolved::Missing => {
                self.call_missing(block, op_ip, (name, namespace), call_type, mode)?;
                return Ok((next, None));
            }
        };
        match script_target(&entry) {
            Some((target, offset)) if offset != 0 => {
                let call = self.vm.new_call(Vec::new(), entry);
                return Ok((
                    next,
                    Some(Box::new(Pending {
                        target,
                        offset,
                        call,
                        argv,
                    })),
                ));
            }
            // A declaration without a body.
            Some(_) => self.pushed_result(Value::default())?,
            None => {
                self.call_native(
                    block,
                    (op_ip, &mut next),
                    name,
                    argv,
                    &entry,
                    (running, kind),
                )?;
            }
        }
        self.st().cur_var = None;
        Ok((next, None))
    }

    /// A call's result goes on the stack and becomes the return variable's text.
    fn pushed_result(&mut self, value: Value) -> Result<(), VmError> {
        let mut st = self.st();
        st.ret = value.clone();
        st.push(value)
    }

    /// Finds the entry a call opcode calls. A plain call looks in the global namespace, a static one in the
    /// namespace it names, a method call in the namespace of the object `argv[1]` names (which becomes the frame's
    /// current object), `Parent::` below the layer of the running function. A missing object warns and gives
    /// None, as a missing function does (see `call_missing`).
    #[inline(never)]
    fn resolve(
        &mut self,
        block: &CodeBlock,
        op_ip: usize,
        (name, namespace, call_type): (&[u8], Option<&[u8]>, u32),
        argv: &[Value],
        running: Option<&ScriptCall>,
    ) -> Result<Resolved, VmError> {
        let target = || argv.get(1).map(Value::string).unwrap_or_default();
        let found = match call_type {
            CALL_FUNCTION => self.vm.namespaces().lookup(b"", name),
            CALL_STATIC => {
                let namespace = namespace.unwrap_or(b"");
                crate::vars::refill(&mut self.st().frame().ns_label, namespace);
                self.vm.namespaces().lookup(namespace, name)
            }
            CALL_METHOD => {
                let target = target();
                let Some(object) = self.vm.objects().find(&target) else {
                    self.vm.warn(format!(
                        "{}: Unable to find object: '{}' attempting to call function '{}'",
                        block.file_line(op_ip),
                        lossy(&target),
                        lossy(name)
                    ));
                    return Ok(Resolved::Warned);
                };
                let vm = &*self.vm;
                let namespace = vm.objects().get(object).map_or(&b""[..], |o| o.namespace());
                {
                    let mut st = self.st();
                    let frame = st.frame();
                    frame.this_object = Some(object);
                    vm.namespaces().display_into(namespace, &mut frame.ns_label);
                }
                vm.namespaces().lookup(namespace, name)
            }
            CALL_PARENT => {
                if argv.len() > 1 {
                    let object = self.vm.objects().find(&target());
                    self.st().frame().this_object = object;
                }
                match running {
                    Some(running) => {
                        let entry = running.entry.borrow();
                        let namespaces = self.vm.namespaces();
                        let label = namespaces
                            .parent_scope(&entry.namespace, running.holder)
                            .unwrap_or_default();
                        let found =
                            namespaces.lookup_parent(&entry.namespace, running.holder, name);
                        drop(entry);
                        self.st().frame().ns_label = label;
                        found
                    }
                    None => {
                        self.st().frame().ns_label.clear();
                        None
                    }
                }
            }
            _ => return Err(VmError::Unsupported(Op::CallFunc.name())),
        };
        Ok(found.map_or(Resolved::Missing, Resolved::Entry))
    }

    /// A call that found no function: a plain or static call warns "Unable to find function" (even in a run
    /// that makes no calls: the lookup comes first), a method or `Parent::` call "Unknown command" (a method call
    /// also names the object and its namespaces; nothing in a run that makes no calls). "" goes on the stack.
    #[cold]
    #[inline(never)]
    fn call_missing(
        &mut self,
        block: &CodeBlock,
        op_ip: usize,
        (name, namespace): (&[u8], Option<&[u8]>),
        call_type: u32,
        mode: RunMode,
    ) -> Result<(), VmError> {
        let label = block.file_line(op_ip + 2);
        match call_type {
            CALL_FUNCTION => {
                self.vm
                    .warn(format!("{label}: Unable to find function {}", lossy(name)));
            }
            CALL_STATIC => {
                let shown = namespace.map(|ns| format!("{}::", lossy(ns)));
                self.vm.warn(format!(
                    "{label}: Unable to find function {}{}",
                    shown.unwrap_or_default(),
                    lossy(name)
                ));
            }
            _ if mode.is_no_calls() => {}
            CALL_METHOD => {
                self.vm
                    .warn(format!("{label}: Unknown command {}.", lossy(name)));
                let object = self.st().frame().this_object;
                if let Some(object) = object {
                    let (shown, namespace) = self
                        .vm
                        .objects()
                        .get(object)
                        .map(|o| (o.name().map(<[u8]>::to_vec), o.namespace().to_vec()))
                        .unwrap_or_default();
                    let chain = self.vm.namespaces().chain_names(&namespace);
                    self.vm.warn(format!(
                        "  Object {}({}) {}",
                        lossy(&shown.unwrap_or_default()),
                        object as i32,
                        lossy(&chain)
                    ));
                }
            }
            _ => {
                self.vm
                    .warn(format!("{label}: Unknown command {}.", lossy(name)));
            }
        }
        self.pushed_missing()
    }

    /// The "" a call that ran nothing leaves.
    fn pushed_missing(&mut self) -> Result<(), VmError> {
        self.st().push(Value::default())
    }

    /// Calls a native after checking its argument count, with the frame's current object as the object it acts
    /// on. Its typed result is pushed as it is, or dropped together with the `POP_STK` after the call; a void
    /// result whose value is used warns (unless `$Con::warnVoidAssignment` is false) and pushes "".
    #[inline(never)]
    fn call_native(
        &mut self,
        block: &CodeBlock,
        (op_ip, ip): (usize, &mut usize),
        name: &[u8],
        argv: Vec<Value>,
        entry: &EntryRef,
        (running, kind): (Option<&ScriptCall>, Kind),
    ) -> Result<(), VmError> {
        let Callable::Native { min, max, usage, f } = entry.borrow().callable.clone() else {
            return Ok(());
        };
        if !argc_ok(argv.len(), min, max) {
            return self.warn_wrong_argc(block, op_ip, name, (argv.len(), min, max), &usage);
        }
        let texts: Vec<Vec<u8>> = argv.into_iter().map(Value::into_string).collect();
        let this_object = self.st().frame().this_object;
        let outer = self.vm.set_this_object(this_object);
        let call = native_call(block, name, texts, entry, this_object);
        let profiled = self.vm.profiling();
        if profiled {
            self.vm.profile_enter(entry);
        }
        let result = f(self.vm, &call);
        if profiled {
            self.vm.profile_leave();
        }
        self.vm.set_this_object(outer);
        let drops = block.words().get(*ip).copied() == Some(Op::PopStk as u32);
        let value = match result {
            Native::Fail(e) => return Err(e),
            Native::Str(s) => {
                if s.len() > MAX_STRING_BYTES {
                    return Err(VmError::StringTooLong);
                }
                return self.pushed_result(Value::from_text(s));
            }
            Native::Int(i) => Value::Int(i64::from(i)),
            Native::Bool(b) => Value::from_bool(b),
            Native::Float(x) => Value::Float(to_f64(to_f32(x))),
            Native::Void => {
                self.st().clear_ret();
                if drops {
                    *ip += 1;
                    return Ok(());
                }
                if self.warns_void_use() {
                    let function = match (kind, running) {
                        (Kind::Function, Some(call)) => script_name(&call.entry),
                        (Kind::TopLevel, _) => block.name().map(<[u8]>::to_vec),
                        _ => None,
                    };
                    self.vm.warn(format!(
                        "{}: Call to {} in {} uses result of void function call.",
                        block.file_line(op_ip + 2),
                        lossy(name),
                        function.map_or_else(|| "(null)".to_string(), |f| lossy(&f))
                    ));
                }
                return self.pushed_missing();
            }
        };
        if drops {
            *ip += 1;
            self.st().ret = value;
            return Ok(());
        }
        self.pushed_result(value)
    }

    /// `$Con::warnVoidAssignment`, true when it is not set.
    fn warns_void_use(&self) -> bool {
        self.vm
            .global_table()
            .get(b"$Con::warnVoidAssignment")
            .map(|e| e.get_string())
            .is_none_or(|text| text.is_empty() || Value::Str(text).truth())
    }

    /// A native called with too few or too many arguments: two warnings (the namespace the frame's last lookup
    /// went through, the counts, the usage) and "" on the stack.
    #[cold]
    #[inline(never)]
    fn warn_wrong_argc(
        &mut self,
        block: &CodeBlock,
        op_ip: usize,
        name: &[u8],
        (argc, min, max): (usize, i32, i32),
        usage: &str,
    ) -> Result<(), VmError> {
        let label = block.file_line(op_ip + 2);
        let scope = self.st().frame().ns_label.clone();
        self.vm.warn(format!(
            "{label}: {}::{} - wrong number of arguments. got {argc}, expected {min} to {max}",
            lossy(&scope),
            lossy(name)
        ));
        self.vm.warn(format!("{label}: usage: {usage}"));
        self.pushed_missing()
    }

    // ---- objects and fields ----

    /// The object declaration and field opcodes; `ip` is the first operand and moves past the operands.
    #[inline(never)]
    fn object_op(
        &mut self,
        block: &Rc<CodeBlock>,
        op: Op,
        at: usize,
        mode: RunMode,
    ) -> Result<usize, VmError> {
        let code = block.words();
        let mut next = at;
        let ip = &mut next;
        match op {
            Op::CreateObject => *ip = self.create_object(block, at, mode)?,
            Op::EndObject => {
                if fetch(code, at)? == 0 {
                    self.st().pop()?;
                }
                *ip = at + 1;
            }
            Op::FinishObject => {
                let mut st = self.st();
                let base = st.frame().creation;
                let suspended = (st.creation.len() > base)
                    .then(|| st.creation.pop())
                    .flatten();
                drop(st);
                if let Some(suspended) = suspended {
                    self.vm.resume_new_object(suspended);
                }
            }
            Op::SetCurObject => self.set_cur_object()?,
            Op::SetCurObjectNew => {
                let target = if self.vm.is_making_object() {
                    FieldTarget::New
                } else {
                    FieldTarget::None
                };
                let mut st = self.st();
                st.frame().cur_object = target;
                if target != FieldTarget::None {
                    st.clear_ret();
                }
            }
            Op::SetCurObjectInternal => {
                let recurse = fetch(code, at)? != 0;
                self.set_cur_object_internal(block, at - 1, recurse)?;
                *ip = at + 1;
            }
            Op::SetCurField => {
                let name = block.ident(fetch(code, at)?).map(<[u8]>::to_vec);
                let mut st = self.st();
                let st = &mut *st;
                std::mem::swap(&mut st.prev_field_array, &mut st.cur_field_array);
                st.cur_field_array.clear();
                let frame = st.frame();
                frame.prev_field = std::mem::replace(&mut frame.cur_field, name);
                *ip = at + 2;
            }
            Op::SetCurFieldArray => {
                let mut st = self.st();
                let index = st.top_charged()?.string().into_owned();
                st.cur_field_array = index.get(..MAX_FIELD_ARRAY).unwrap_or(&index).to_vec();
            }
            Op::SetCurFieldType => {
                let type_id = fetch(code, at)?;
                self.set_field_type(type_id);
                *ip = at + 1;
            }
            Op::LoadFieldUint | Op::LoadFieldFlt | Op::LoadFieldStr => self.load_field(op)?,
            _ => self.save_field()?,
        }
        Ok(next)
    }

    /// `OP_CREATE_OBJECT`: operands the parent's name (two words), the datablock, internal-name and singleton
    /// flags, the declaration's line and the fail jump; the class, the name and the constructor arguments are
    /// the open argument list. The object being built so far is set aside until `OP_FINISH_OBJECT`. A
    /// non-internal name may not be a class's; a singleton whose name is taken fills in the existing object
    /// (of the same class). Returns the next instruction: the body, or the fail jump.
    #[inline(never)]
    fn create_object(
        &mut self,
        block: &CodeBlock,
        ip: usize,
        mode: RunMode,
    ) -> Result<usize, VmError> {
        let code = block.words();
        let parent_word = fetch(code, ip)?;
        let datablock = fetch(code, ip + 2)? != 0;
        let internal_name = fetch(code, ip + 3)? != 0;
        let singleton = fetch(code, ip + 4)? != 0;
        let fail = fetch(code, ip + 6)? as usize;
        let argv: Vec<Vec<u8>> = self
            .st()
            .take_args(b"")?
            .into_iter()
            .map(Value::into_string)
            .collect();
        if mode.is_no_calls() {
            return Ok(fail);
        }
        let parent = match block.ident(parent_word) {
            None if parent_word != 0 => return Err(VmError::BadIndex { ip }),
            found => found.map(<[u8]>::to_vec),
        };
        if self.st().creation.len() >= MAX_NESTING {
            return Err(VmError::StackOverflow);
        }
        let suspended = self.vm.suspend_new_object();
        self.st().creation.push(suspended);
        let class = argv.get(1).cloned().unwrap_or_default();
        let name = argv.get(2).cloned().unwrap_or_default();
        let mut reuse = None;
        if !datablock && !internal_name {
            if self.vm.objects().classes.find(&name).is_some() {
                self.vm.error(format!(
                    "{}: Cannot name object [{}] the same name as a script class.",
                    block.file_line(ip),
                    lossy(&name)
                ));
                return Ok(fail);
            }
            if singleton && let Some(found) = self.vm.objects().find(&name) {
                let was = self
                    .vm
                    .objects()
                    .get(found)
                    .map(|o| self.vm.objects().classes.name(o.class()).to_vec())
                    .unwrap_or_default();
                if !was.eq_ignore_ascii_case(&class) {
                    self.vm.error(format!(
                        "{}: Cannot re-declare object [{}] with a different class [{}] - was [{}].",
                        block.file_line(ip),
                        lossy(&name),
                        lossy(&class),
                        lossy(&was)
                    ));
                    return Ok(fail);
                }
                self.vm.warn(format!(
                    "{}: Singleton Object was already created with name {}. Using existing object.",
                    block.file_line(ip),
                    lossy(&name)
                ));
                reuse = Some(found);
            }
        }
        let declaration = Declaration {
            datablock,
            internal_name,
            reuse,
        };
        self.vm.begin_new_object_as(
            (block, ip - 1),
            (parent, declaration),
            (ip + 7, fail),
            argv,
            false,
        )
    }

    /// `OP_ADD_OBJECT`: registers the object being built (see [`Vm::add_new_object`]); its id replaces the 0 the
    /// declaration pushed (`place at root`) or is pushed for its children, which join the group or set below it
    /// on the stack. Nothing when no object is being built.
    ///
    /// On the recursion path of a script `onAdd`: dispatched from the loop itself.
    #[inline(never)]
    fn add_object(&mut self, block: &CodeBlock, at: usize) -> Result<usize, VmError> {
        let place_at_root = fetch(block.words(), at)? != 0;
        if !self.vm.is_making_object() {
            return Ok(at + 1);
        }
        let ex = self.ex;
        let target = move || ex.state.borrow_mut().top().map(|v| v.int() as u32);
        match self
            .vm
            .add_new_object((block, at.saturating_sub(1)), place_at_root, &target)?
        {
            AddedObject::Failed(fail) => Ok(fail),
            AddedObject::Added(id) => {
                let mut st = self.st();
                let id = Value::Int(i64::from(id));
                if place_at_root {
                    *st.top()? = id;
                } else {
                    st.push(id)?;
                }
                Ok(at + 1)
            }
        }
    }

    /// `OP_SETCUROBJECT`: the object the top value names (a text with a space names none); the field operations
    /// act on it. When none is found the text becomes the return variable (a field component reads it).
    #[inline(never)]
    fn set_cur_object(&mut self) -> Result<(), VmError> {
        let text = self.st().top_charged()?.string().into_owned();
        let found = if text.contains(&b' ') {
            None
        } else {
            self.vm.objects().find(&text)
        };
        let mut st = self.st();
        let frame = st.frame();
        frame.prev_object = frame.cur_object;
        frame.cur_object = found.map_or(FieldTarget::None, FieldTarget::Live);
        st.ret = Value::Str(if found.is_some() { Vec::new() } else { text });
        Ok(())
    }

    /// `OP_SETCUROBJECT_INTERNAL`: the member of the current object (a set) whose internal name is the top value
    /// replaces that value (0 when there is none, with an error when the current object is not a set or there
    /// is none).
    #[inline(never)]
    fn set_cur_object_internal(
        &mut self,
        block: &CodeBlock,
        op_ip: usize,
        recurse: bool,
    ) -> Result<(), VmError> {
        let target = self.st().frame().cur_object;
        let object = match target {
            FieldTarget::Live(id) => Some(id).filter(|id| self.vm.objects().contains(*id)),
            _ => None,
        };
        let name = self.st().top_charged()?.string().into_owned();
        let found = match object {
            Some(set) if self.vm.objects().is_set(set) => {
                self.vm.objects().find_internal(set, &name, recurse)
            }
            Some(other) => {
                let (shown, class) = self
                    .vm
                    .objects()
                    .get(other)
                    .map(|o| {
                        (
                            o.name().map(<[u8]>::to_vec).unwrap_or_default(),
                            self.vm.objects().classes.name(o.class()).to_vec(),
                        )
                    })
                    .unwrap_or_default();
                self.vm.error(format!(
                    "{}: Attempt to use -> on non-set {} of class {}.",
                    block.file_line(op_ip),
                    lossy(&shown),
                    lossy(&class)
                ));
                None
            }
            None => {
                self.vm.error(format!(
                    "{}: Attempt to use ->, but the group object wasn't found.",
                    block.file_line(op_ip)
                ));
                None
            }
        };
        *self.st().top()? = Value::Int(found.map_or(0, i64::from));
        Ok(())
    }

    /// `OP_SETCURFIELD_TYPE`: tags the current dynamic field of the current object with a console type.
    #[inline(never)]
    fn set_field_type(&mut self, type_id: u32) {
        let (target, mut field) = {
            let mut st = self.st();
            let array = st.cur_field_array.clone();
            let frame = st.frame();
            let Some(field) = frame.cur_field.clone() else {
                return;
            };
            let mut field = field;
            field.extend_from_slice(&array);
            (frame.cur_object, field)
        };
        let FieldTarget::Live(id) = target else {
            return;
        };
        field = until_nul(&field).to_vec();
        if let Some(object) = self.vm.objects_mut().get_mut(id) {
            object.set_dynamic_field_type(&field, type_id);
        }
    }

    /// `OP_LOADFIELD_*`: the current field of the current object, read as an integer (`atoi`), a float (`atof`,
    /// 64-bit) or text; without an object, a component of the value before (see `component`).
    #[inline(never)]
    fn load_field(&mut self, op: Op) -> Result<(), VmError> {
        let target = self.st().frame().cur_object;
        let text = if target == FieldTarget::None {
            let field = self.st().frame().cur_field.clone();
            self.component(field.as_deref().unwrap_or(b""))
        } else {
            let mut text = Vec::new();
            {
                // Reading runs no script: the name and the index are read in place.
                let st = self.st();
                let field = st.frames.last().and_then(|f| f.cur_field.as_deref());
                self.vm.read_field_into(
                    target,
                    field.unwrap_or(b""),
                    &st.cur_field_array,
                    &mut text,
                );
            }
            if op == Op::LoadFieldStr {
                self.st().clear_ret();
            }
            text
        };
        let value = match op {
            Op::LoadFieldUint => Value::Int(i64::from(atoi(&text))),
            Op::LoadFieldFlt => Value::Float(atof(&text)),
            _ => Value::Str(text),
        };
        self.st().push(value)
    }

    /// `OP_SAVEFIELD_*`: the top value's text (left there) into the current field of the current object; without
    /// an object, into a component of the value before (see `set_component`).
    #[inline(never)]
    fn save_field(&mut self) -> Result<(), VmError> {
        // The name, index and value are moved out while the write runs (a static-write observer may run script) and put
        // back after, so a write copies nothing but the value's text.
        let (target, field, array, value) = {
            let mut st = self.st();
            let st = &mut *st;
            let mut value = std::mem::take(&mut st.field_value);
            value.clear();
            st.top_charged()?
                .with_string(|text| value.extend_from_slice(text));
            let array = std::mem::take(&mut st.cur_field_array);
            let frame = st.frame();
            (frame.cur_object, frame.cur_field.take(), array, value)
        };
        if target == FieldTarget::None {
            let done = self.set_component(field.as_deref().unwrap_or(b""), &value);
            if done.is_ok() {
                self.st().frame().prev_object = FieldTarget::None;
            }
            self.put_back_field(field, array, value);
            return done;
        }
        self.vm
            .write_field_of(target, field.as_deref().unwrap_or(b""), &array, &value);
        self.put_back_field(field, array, value);
        Ok(())
    }

    /// What `save_field` moved out of the state, back in.
    fn put_back_field(&mut self, field: Option<Vec<u8>>, array: Vec<u8>, value: Vec<u8>) {
        let mut st = self.st();
        st.cur_field_array = array;
        st.field_value = value;
        if let Some(frame) = st.frames.last_mut() {
            frame.cur_field = field;
        }
    }

    /// The value a field component reads or writes (Torque3D's `.x`, `.y`, `.z`, `.w` and their colour and
    /// texture-coordinate spellings on a value that is not an object): the previous object's previous field, else
    /// the register last named, else the current variable, else (for a read) the return variable.
    fn component_source(&mut self, read: bool) -> Option<ComponentSource> {
        let (prev_object, prev_field, register, has_var, prev_array, ret) = {
            let mut st = self.st();
            let has_var = st.cur_var.is_some();
            let prev_array = st.prev_field_array.clone();
            let ret = st.ret.clone();
            let frame = st.frame();
            (
                frame.prev_object,
                frame.prev_field.clone(),
                frame.current_register,
                has_var,
                prev_array,
                ret,
            )
        };
        match (prev_object, prev_field) {
            (FieldTarget::Live(_) | FieldTarget::New, Some(field)) => {
                Some(ComponentSource::Field {
                    target: prev_object,
                    field,
                    array: prev_array,
                })
            }
            _ => {
                if let Some(reg) = register {
                    Some(ComponentSource::Register(reg))
                } else if has_var {
                    Some(ComponentSource::Variable)
                } else if read && !matches!(&ret, Value::Str(text) if text.is_empty()) {
                    Some(ComponentSource::Text(ret.into_string()))
                } else {
                    None
                }
            }
        }
    }

    /// The text of a component source.
    fn source_text(&mut self, source: &ComponentSource) -> Vec<u8> {
        match source {
            ComponentSource::Field {
                target,
                field,
                array,
            } => self.vm.read_field_of(*target, field, array),
            ComponentSource::Register(reg) => {
                let mut st = self.st();
                let text = st
                    .register(*reg, 0)
                    .map(|v| v.string().into_owned())
                    .unwrap_or_default();
                st.charge(text.len());
                text
            }
            ComponentSource::Variable => {
                let text = self.with_var(|e| e.get_string()).unwrap_or_default();
                let mut st = self.st();
                st.charge(text.len());
                text
            }
            ComponentSource::Text(text) => text.clone(),
        }
    }

    /// A field component read: the words of the source value the suffix letters pick, joined by spaces, at most
    /// [`COMPONENT_NUMBER`] bytes ("" when a letter is not a component's, or for more than four letters).
    fn component(&mut self, suffix: &[u8]) -> Vec<u8> {
        if suffix.len() > MAX_COMPONENT_LETTERS
            || !suffix.iter().all(|&c| suffix_index(c).is_some())
        {
            return Vec::new();
        }
        let Some(source) = self.component_source(true) else {
            return Vec::new();
        };
        let value = self.source_text(&source);
        if value.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (i, index) in suffix.iter().filter_map(|&c| suffix_index(c)).enumerate() {
            if i > 0 {
                out.push(b' ');
            }
            let room = COMPONENT_NUMBER.saturating_sub(out.len());
            let word = unit(&value, index, WORDS);
            out.extend_from_slice(word.get(..room).unwrap_or(&word));
            if out.len() >= COMPONENT_NUMBER {
                break;
            }
        }
        out.truncate(COMPONENT_NUMBER);
        out
    }

    /// A field component write: the source value with the words the suffix letters pick replaced by the
    /// words of `text` (a component count that does not match the suffix warns); written back when the result
    /// is not empty. The value, the text and the result are cut at [`COMPONENT_WRITE`] bytes; the work is charged.
    fn set_component(&mut self, suffix: &[u8], text: &[u8]) -> Result<(), VmError> {
        if suffix.len() > MAX_COMPONENT_LETTERS
            || !suffix.iter().all(|&c| suffix_index(c).is_some())
        {
            return Ok(());
        }
        let Some(source) = self.component_source(false) else {
            return Ok(());
        };
        let text = text.get(..COMPONENT_WRITE).unwrap_or(text);
        let mut out = self.source_text(&source);
        out.truncate(COMPONENT_WRITE);
        for (i, index) in suffix.iter().filter_map(|&c| suffix_index(c)).enumerate() {
            let word = if suffix.len() == 1 {
                text.to_vec()
            } else {
                unit(text, i32::try_from(i).unwrap_or(i32::MAX), WORDS)
            };
            if word.is_empty() && suffix.len() > 1 {
                continue;
            }
            let vm = &*self.vm;
            let charge = |pad: usize| {
                if vm.charge_work(pad as u64) {
                    Ok(())
                } else {
                    Err(VmError::StepLimit)
                }
            };
            out = set_unit(&out, index, &word, WORDS, charge)?;
            if !self.vm.charge_work(out.len() as u64) {
                return Err(VmError::StepLimit);
            }
        }
        out.truncate(COMPONENT_WRITE);
        let count = unit_count(text, WORDS);
        if suffix.len() > 1 && usize::try_from(count).ok() != Some(suffix.len()) {
            self.vm.warn(format!(
                "setFieldComponent: component count mismatch - suffix '{}' expects {} value(s), got {} ('{}')!",
                lossy(suffix),
                suffix.len(),
                count,
                lossy(text)
            ));
        }
        if out.is_empty() {
            return Ok(());
        }
        match source {
            ComponentSource::Field { target, field, .. } => {
                self.vm.write_field_of(target, &field, b"", &out);
            }
            ComponentSource::Register(reg) => {
                if let Ok(slot) = self.st().register(reg, 0) {
                    *slot = Value::Str(out);
                }
            }
            ComponentSource::Variable => {
                self.with_var(|e| e.set_string(&out));
            }
            ComponentSource::Text(_) => {}
        }
        Ok(())
    }

    // ---- foreach ----

    /// `OP_ITER_BEGIN` (`foreach` over a set's members), `OP_ITER_BEGIN_STR` (`foreach$` over a text's words),
    /// `OP_ITER` (the next one into the loop variable, or out past the loop) and `OP_ITER_END`. The container
    /// stays on the stack while its loop runs.
    #[inline(never)]
    fn iterate(&mut self, block: &CodeBlock, op: Op, at: usize) -> Result<usize, VmError> {
        let code = block.words();
        let mut next = at;
        let ip = &mut next;
        match op {
            Op::IterBegin | Op::IterBeginStr => {
                let global = fetch(code, at)? != 0;
                let var = if global {
                    let name = block.ident(fetch(code, at + 1)?).unwrap_or(b"").to_vec();
                    self.vm.global_table_mut().add(&name);
                    IterVar::Global(name)
                } else {
                    IterVar::Register(fetch(code, at + 1)?)
                };
                let fail = fetch(code, at + if global { 3 } else { 2 })? as usize;
                *ip = at + if global { 4 } else { 3 };
                let text = self.st().top_charged()?.string().into_owned();
                let over = if op == Op::IterBeginStr {
                    let mut held = MemoryCharge::new(self.vm.objects().memory());
                    if !held.resize(text.len()) {
                        return Err(VmError::MemoryLimit);
                    }
                    IterOver::Words {
                        text,
                        at: 0,
                        _held: held,
                    }
                } else {
                    match self.vm.objects().find(&text) {
                        Some(set) if self.vm.objects().is_set(set) => {
                            IterOver::Set { set, index: 0 }
                        }
                        _ => {
                            self.vm
                                .error(format!("No SimSet object '{}'", lossy(&text)));
                            self.vm.error(
                                "Did you mean to use 'foreach$' instead of 'foreach'?".to_string(),
                            );
                            self.st().pop()?;
                            return Ok(fail);
                        }
                    }
                };
                let mut st = self.st();
                if st.iters.len() >= MAX_NESTING {
                    return Err(VmError::StackOverflow);
                }
                st.iters.push(Iter { var, over });
            }
            Op::Iter => {
                let done = fetch(code, at)? as usize;
                *ip = if self.step_iter(at)? { at + 1 } else { done };
            }
            _ => {
                let mut st = self.st();
                let base = st.frame().iters;
                if st.iters.len() <= base {
                    return Err(VmError::StackUnderflow);
                }
                st.iters.pop();
                st.pop()?;
            }
        }
        Ok(next)
    }

    /// One step of the innermost loop: the next word or member into its variable; false when there is none.
    #[inline(never)]
    fn step_iter(&mut self, ip: usize) -> Result<bool, VmError> {
        let next = {
            let mut st = self.st();
            let base = st.frame().iters;
            if st.iters.len() <= base {
                return Err(VmError::StackUnderflow);
            }
            let Some(iter) = st.iters.last_mut() else {
                return Err(VmError::StackUnderflow);
            };
            match &mut iter.over {
                IterOver::Words { text, at, .. } => {
                    let Some(rest) = text.get(*at..).filter(|r| !r.is_empty()) else {
                        return Ok(false);
                    };
                    let len = rest.iter().position(|&b| is_space(b)).unwrap_or(rest.len());
                    let word = rest[..len].to_vec();
                    *at += len + usize::from(len < rest.len());
                    Value::Str(word)
                }
                IterOver::Set { set, index } => {
                    let member = self.vm.objects().members(*set).and_then(|m| m.get(*index));
                    let Some(member) = member else {
                        return Ok(false);
                    };
                    *index += 1;
                    Value::Int(i64::from(member))
                }
            }
        };
        let mut st = self.st();
        let var = match st.iters.last().map(|i| &i.var) {
            Some(IterVar::Global(name)) => Err(name.clone()),
            Some(IterVar::Register(reg)) => Ok(*reg),
            None => return Err(VmError::StackUnderflow),
        };
        match var {
            Ok(reg) => {
                st.charge(next.text_len());
                *st.register(reg, ip)? = next;
            }
            Err(name) => {
                drop(st);
                if let Some(entry) = self.vm.global_table_mut().add(&name) {
                    match next {
                        Value::Int(id) => entry.set_int(id as u32),
                        other => other.with_string(|text| entry.set_string(text)),
                    }
                }
            }
        }
        Ok(true)
    }
}

/// A script function a call opcode is about to run (see `Cx::prepare_call`).
struct Pending {
    target: Rc<CodeBlock>,
    offset: usize,
    call: ScriptCall,
    argv: Vec<Value>,
}

/// What a call opcode's lookup found (see `Cx::resolve`).
enum Resolved {
    Entry(EntryRef),
    /// No function: `call_missing` warns.
    Missing,
    /// The object of a method call does not exist: the warning is out.
    Warned,
}

/// Where a field component's value comes from (see `Cx::component_source`).
enum ComponentSource {
    Field {
        target: FieldTarget,
        field: Vec<u8>,
        array: Vec<u8>,
    },
    Register(u32),
    Variable,
    Text(Vec<u8>),
}

/// The word a component letter picks: `x`, `r`, `u` the first, `y`, `g`, `v` the second, `z`, `b` the third, `w`, `a`
/// the fourth (any case).
fn suffix_index(c: u8) -> Option<i32> {
    match c.to_ascii_lowercase() {
        b'x' | b'r' | b'u' => Some(0),
        b'y' | b'g' | b'v' => Some(1),
        b'z' | b'b' => Some(2),
        b'w' | b'a' => Some(3),
        _ => None,
    }
}

/// The string at `offset` of the function (`function`) or global string table, to the end of the table (a
/// reader stops at its NUL).
fn string_at(block: &CodeBlock, function: bool, offset: u32, ip: usize) -> Result<&[u8], VmError> {
    block
        .strings(function)
        .get(offset as usize..)
        .ok_or(VmError::BadIndex { ip })
}
