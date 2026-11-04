//! The classic executor: the evaluation loop of TGE 1.4's instruction set
//! ([`torque_ts_bytecode::classic`]), with its integer and float stacks, the string stack (a reused byte buffer
//! whose leftovers scripts can observe), its current variable, object and field, and the delivery of a native's
//! typed result by peeking at the next opcode.
//!
//! The loop runs on the [`Vm`]'s public executor interface (see [`crate::exec`]); its state lives in the
//! executor, borrowed per instruction and never across a call into the `Vm` that can run script or print.
//!
//! **Dialect.** [`ClassicExecutor::new`] runs TGE 1.4's files; [`ClassicExecutor::with_dialect`] another build's:
//! its file format ([`DsoFormat`]), its opcode numbering and its [`ClassicRules`]. The console rules' loud report
//! ([`ConsoleRules::backtraces`](crate::policy::ConsoleRules::backtraces)) also changes how a call reports a missing
//! function, a wrong argument count and a void result (see those methods).
//!
//! Follows TGE 1.4's `console/compiledEval.cc` (`CodeBlock::exec`) and `console/stringStack.h` (each Copyright (C)
//! GarageGames.com, Inc.), whose MIT-licensed successor is Torque3D's `console/torquescript/compiledEval.cpp`
//! (Copyright (c) 2013 GarageGames, LLC; Copyright (c) 2015 Faust Logic, Inc.; Copyright (c) 2021 TGEMIT Authors &
//! Contributors). The code here is this crate's own.
//!
//! **Stack.** Every frame on the recursion path of a script call (`exec`, `exec_loop`, `call_function`,
//! `call_native`, `add_object`) is multiplied by the call depth: a borrow of the state is a local of the frame
//! that takes it, so those frames take none and leave every state access to out-of-line helpers.

use std::any::Any;
use std::cell::{RefCell, RefMut};
use std::rc::Rc;

pub mod compiler;
pub mod strstack;

pub use compiler::ClassicCompiler;

use torque_ts_bytecode::classic::{ClassicDialect, Op, OpcodeTable};

use crate::code::{CodeBlock, DsoFormat};
use crate::exec::{
    Executor, RunMode as Mode, ScriptCall, argc_ok, fetch, native_call, script_name, script_target,
};
use crate::ns::{Callable, EntryRef};
use crate::sim::{MAX_FIELD_NAME, ObjId};
use crate::tags::tag_text;
use crate::text::{atof, atoi, f64_to_u32, to_f32, to_f64, until_nul};
use crate::vars::Entry;
use crate::vm::{AddedObject, FieldTarget, MAX_STACK, Native, Vm, VmError, lossy};

use strstack::{ArgText, Mark, StringStack};

/// What a build of the classic VM does its own way (see the module documentation). [`ClassicRules::TGE14`] is TGE
/// 1.4's.
#[derive(Clone, Copy, Debug)]
pub struct ClassicRules {
    /// The digits of a tag id `OP_TAG_TO_STR` writes over its literal (TGE 1.4's `dSprintf` into 7 bytes: 6).
    pub tag_digits: usize,
    /// The result of `+ - * /` from its value and its two operands (top, second): TGE 1.4's is the value.
    pub float_result: fn(f64, f64, f64) -> f64,
    /// A declaration that would replace a native (see [`Namespaces::shadows_native`](crate::ns::Namespaces)) is
    /// skipped silently (TGE 1.4 replaces it).
    pub keep_natives: bool,
    /// The error line of a declaration the declare policy refuses, from its namespace and name; None: silent.
    pub refused_declaration: Option<RefusalLine>,
    /// Under `exec(file, true)` a method call on an object that does not exist is skipped silently (TGE 1.4 warns
    /// `Unable to find object` first).
    pub quiet_missing_object_without_calls: bool,
}

/// The line of a refused declaration, from its namespace and name.
pub type RefusalLine = fn(&[u8], &[u8]) -> String;

fn plain_float(value: f64, _top: f64, _second: f64) -> f64 {
    value
}

impl ClassicRules {
    /// TGE 1.4's.
    pub const TGE14: ClassicRules = ClassicRules {
        tag_digits: 6,
        float_result: plain_float,
        keep_natives: false,
        refused_declaration: None,
        quiet_missing_object_without_calls: false,
    };
}

impl Default for ClassicRules {
    fn default() -> ClassicRules {
        ClassicRules::TGE14
    }
}

/// TGE 1.4's file format, as the runtime reads it.
pub static TGE14_DIALECT: ClassicDialect = ClassicDialect::TGE14;

/// Makes `vm` compile with TGE 1.4's classic compiler ([`ClassicCompiler`]), for a `Vm` made with a
/// [`ClassicExecutor`].
pub fn install(vm: &mut Vm) {
    vm.set_compiler(ClassicCompiler::default());
}

/// Where the current variable lives; its folded name is `ClassicState::cur_key`, its slot `cur_slot` while its table's
/// stamp is `cur_stamp` (see [`Dictionary`](crate::vars::Dictionary)).
#[derive(Clone, Copy)]
enum VarRef {
    Global,
    Local(usize),
}

/// What a call opcode calls into, for the wrong-argument-count warning: it names the namespace of the object a
/// method was called on, the namespace a `Parent::` lookup starts in, and nothing for a plain call.
#[derive(Clone, Copy)]
enum Callee {
    Plain,
    Method(ObjId),
    /// The running function's namespace says where the lookup started.
    Parent,
}

struct Decoder {
    fast: [Option<Op>; 256],
    table: OpcodeTable,
}

impl Decoder {
    fn new(table: OpcodeTable) -> Decoder {
        let mut fast = [None; 256];
        for op in Op::ALL {
            let code = table.code(*op);
            if (code as usize) < fast.len() {
                fast[code as usize] = Some(*op);
            }
        }
        Decoder { fast, table }
    }

    fn decode(&self, word: u32) -> Option<Op> {
        if (word as usize) < self.fast.len() {
            self.fast[word as usize]
        } else {
            self.table.op(word)
        }
    }
}

/// The classic loop's evaluation state (see the module documentation).
struct ClassicState {
    strs: StringStack,
    ints: Vec<u32>,
    floats: Vec<f64>,
    cur_var: Option<VarRef>,
    /// The folded name of the current variable (kept between selections, so a selection copies no name).
    cur_key: Vec<u8>,
    /// The current variable's slot in its table, valid while the table's stamp is `cur_stamp` (0: never; then the
    /// variable is found again by its name, as a table that lost an entry since the selection needs).
    cur_slot: usize,
    cur_stamp: u64,
    /// A buffer for a variable name taken from the string stack.
    name_scratch: Vec<u8>,
    /// Argument lists of finished calls, their buffers kept for the next calls (`Cx::recycle_argv`).
    argv_pool: Vec<Vec<Vec<u8>>>,
    /// The object the field opcodes act on, and the field and array index they name.
    cur_object: FieldTarget,
    cur_field: Vec<u8>,
    cur_field_array: Vec<u8>,
}

impl ClassicState {
    fn push_u(&mut self, v: u32) -> Result<(), VmError> {
        if self.ints.len() >= MAX_STACK {
            return Err(VmError::StackOverflow);
        }
        self.ints.push(v);
        Ok(())
    }

    fn pop_u(&mut self) -> Result<u32, VmError> {
        self.ints.pop().ok_or(VmError::StackUnderflow)
    }

    fn top_u(&mut self) -> Result<&mut u32, VmError> {
        self.ints.last_mut().ok_or(VmError::StackUnderflow)
    }

    fn push_f(&mut self, v: f64) -> Result<(), VmError> {
        if self.floats.len() >= MAX_STACK {
            return Err(VmError::StackOverflow);
        }
        self.floats.push(v);
        Ok(())
    }

    fn pop_f(&mut self) -> Result<f64, VmError> {
        self.floats.pop().ok_or(VmError::StackUnderflow)
    }

    /// The string-stack opcodes without an operand.
    fn string_op(&mut self, op: Op) -> Result<(), VmError> {
        let strs = &mut self.strs;
        match op {
            Op::AdvanceStr => strs.advance()?,
            Op::AdvanceStrComma => strs.advance_char(b'_')?,
            Op::AdvanceStrNul => strs.advance_char(0)?,
            Op::RewindStr => strs.rewind()?,
            Op::TerminateRewindStr => strs.rewind_terminate()?,
            Op::Push => strs.push()?,
            Op::PushFrame => strs.push_frame()?,
            _ => {
                let equal = strs.compare()?;
                self.push_u(u32::from(equal))?;
            }
        }
        Ok(())
    }

    /// The jump opcodes' condition.
    fn jump_taken(&mut self, op: Op) -> Result<bool, VmError> {
        Ok(match op {
            Op::JmpIfNot => self.pop_u()? == 0,
            Op::JmpIf => self.pop_u()? != 0,
            Op::JmpIfFNot => self.pop_f()? == 0.0,
            Op::JmpIfF => self.pop_f()? != 0.0,
            // The `_NP` forms keep the value when they jump and pop it when they fall through.
            Op::JmpIfNotNp => {
                let zero = *self.top_u()? == 0;
                if !zero {
                    self.ints.pop();
                }
                zero
            }
            Op::JmpIfNp => {
                let nonzero = *self.top_u()? != 0;
                if !nonzero {
                    self.ints.pop();
                }
                nonzero
            }
            Op::Jmp => true,
            _ => return Err(VmError::Unsupported(op.name())),
        })
    }

    fn unary(&mut self, op: Op) -> Result<(), VmError> {
        match op {
            Op::Not => {
                let t = self.top_u()?;
                *t = u32::from(*t == 0);
            }
            Op::NotF => {
                let f = self.pop_f()?;
                self.push_u(u32::from(f == 0.0))?;
            }
            // In place on the integer stack's top, as TGE's `intStack[UINT] = ~intStack[UINT]`. When the compiler
            // put the operand on the FLOAT stack (a string constant or a call's result: `~"3"`, `~strlen(%s)`)
            // the integer stack can be empty: TGE then works on the slot below the first, which holds 0, and goes
            // on with `~0`, which prints as -1. No `StackUnderflow` here: the result is pushed for the `SAVEVAR`
            // or `UINT_TO_NONE` after it to take.
            Op::OnesComplement => match self.ints.last_mut() {
                Some(t) => *t = !*t,
                None => self.push_u(!0)?,
            },
            _ => {
                let f = self.pop_f()?;
                self.push_f(-f)?;
            }
        }
        Ok(())
    }

    fn convert(&mut self, op: Op) -> Result<(), VmError> {
        match op {
            Op::StrToUint => {
                let v = self.strs.int_value();
                self.push_u(v)?;
            }
            Op::StrToFlt => {
                let v = self.strs.float_value();
                self.push_f(v)?;
            }
            Op::FltToUint => {
                let f = self.pop_f()?;
                self.push_u(f64_to_u32(f))?;
            }
            Op::FltToStr => {
                let f = self.pop_f()?;
                self.strs.set_float(f);
            }
            Op::FltToNone => {
                self.pop_f()?;
            }
            Op::UintToFlt => {
                let u = self.pop_u()?;
                self.push_f(f64::from(u))?;
            }
            Op::UintToStr => {
                let u = self.pop_u()?;
                self.strs.set_int(u);
            }
            Op::UintToNone => {
                self.pop_u()?;
            }
            Op::StrToNone => {}
            _ => return Err(VmError::Unsupported(op.name())),
        }
        Ok(())
    }

    fn compare(&mut self, op: Op) -> Result<(), VmError> {
        let top = self.pop_f()?;
        let second = self.pop_f()?;
        let r = match op {
            Op::CmpEq => top == second,
            Op::CmpGr => top > second,
            Op::CmpGe => top >= second,
            Op::CmpLt => top < second,
            Op::CmpLe => top <= second,
            _ => top != second,
        };
        self.push_u(u32::from(r))
    }

    fn int_binary(&mut self, op: Op) -> Result<(), VmError> {
        let top = self.pop_u()?;
        let second = self.pop_u()?;
        let r = match op {
            Op::Xor => top ^ second,
            Op::Mod => {
                if second == 0 {
                    return Err(VmError::DivideByZero);
                }
                top % second
            }
            Op::BitAnd => top & second,
            Op::BitOr => top | second,
            Op::Shr => top.wrapping_shr(second),
            Op::Shl => top.wrapping_shl(second),
            Op::And => u32::from(top != 0 && second != 0),
            _ => u32::from(top != 0 || second != 0),
        };
        self.push_u(r)
    }

    fn float_binary(&mut self, op: Op, result: fn(f64, f64, f64) -> f64) -> Result<(), VmError> {
        let top = self.pop_f()?;
        let second = self.pop_f()?;
        let r = match op {
            Op::Add => top + second,
            Op::Sub => top - second,
            Op::Mul => top * second,
            _ => top / second,
        };
        self.push_f(result(r, top, second))
    }

    /// Delivers a native function's typed result as TGE does, peeking at the next opcode: true when
    /// that opcode was consumed and must be skipped. `Void` and `Fail` are the caller's.
    fn deliver_typed(&mut self, result: &Native, next: Option<Op>) -> Result<bool, VmError> {
        match *result {
            Native::Int(i) => match next {
                Some(Op::StrToUint) => {
                    self.push_u(i as u32)?;
                    return Ok(true);
                }
                Some(Op::StrToFlt) => {
                    self.push_f(f64::from(i))?;
                    return Ok(true);
                }
                Some(Op::StrToNone) => return Ok(true),
                _ => self.strs.set_int(i as u32),
            },
            Native::Bool(b) => match next {
                Some(Op::StrToUint) => {
                    self.push_u(u32::from(b))?;
                    return Ok(true);
                }
                Some(Op::StrToFlt) => {
                    self.push_f(f64::from(u8::from(b)))?;
                    return Ok(true);
                }
                Some(Op::StrToNone) => return Ok(true),
                _ => self.strs.set_int(u32::from(b)),
            },
            Native::Float(x) => match next {
                Some(Op::StrToUint) => {
                    self.push_u(f64_to_u32(x))?;
                    return Ok(true);
                }
                Some(Op::StrToFlt) => {
                    self.push_f(x)?;
                    return Ok(true);
                }
                Some(Op::StrToNone) => return Ok(true),
                _ => self.strs.set_float(x),
            },
            Native::Str(ref s) => self.strs.set_string(s),
            Native::Void | Native::Fail(_) => {}
        }
        Ok(false)
    }
}

/// The classic executor (see the module documentation). One per [`Vm`].
pub struct ClassicExecutor {
    decoder: Decoder,
    format: &'static dyn DsoFormat,
    rules: ClassicRules,
    state: RefCell<ClassicState>,
}

impl Default for ClassicExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ClassicExecutor {
    /// TGE 1.4's VM.
    pub fn new() -> ClassicExecutor {
        ClassicExecutor::with_dialect(&TGE14_DIALECT, OpcodeTable::TGE14, ClassicRules::TGE14)
    }

    /// The VM of another build: what `exec` loads (`format`), the numbering its code uses and its rules.
    pub fn with_dialect(
        format: &'static dyn DsoFormat,
        opcodes: OpcodeTable,
        rules: ClassicRules,
    ) -> ClassicExecutor {
        ClassicExecutor {
            decoder: Decoder::new(opcodes),
            format,
            rules,
            state: RefCell::new(ClassicState {
                strs: StringStack::new(),
                ints: Vec::new(),
                floats: Vec::new(),
                cur_var: None,
                cur_key: Vec::new(),
                cur_slot: 0,
                cur_stamp: 0,
                name_scratch: Vec::new(),
                argv_pool: Vec::new(),
                cur_object: FieldTarget::None,
                cur_field: Vec::new(),
                cur_field_array: Vec::new(),
            }),
        }
    }

    fn st(&self) -> RefMut<'_, ClassicState> {
        self.state.borrow_mut()
    }

    /// What entry `slot + 1` of the argument array of the last call by bytecode points at now, looked at for at most
    /// `limit` bytes (see [`StringStack::arg_text`]): `Some(None)` for the null pointer, None when no terminator
    /// came within the limit.
    pub fn arg_text(&self, slot: usize, limit: usize) -> Option<Option<Vec<u8>>> {
        match self.st().strs.arg_text(slot, limit) {
            ArgText::Null => Some(None),
            ArgText::Text(text) => Some(Some(text.to_vec())),
            ArgText::TooLong => None,
        }
    }

    /// A call with no arguments was made: the argument array's entries are null.
    pub fn clear_arg_slots(&self) {
        self.st().strs.clear_arg_slots();
    }
}

/// What a run started from Rust puts back (see [`Executor::save`]).
struct Saved {
    ints: usize,
    floats: usize,
    strs: Mark,
    cur_var: Option<(VarRef, Vec<u8>)>,
}

impl Executor for ClassicExecutor {
    fn run(
        &self,
        vm: &mut Vm,
        block: &Rc<CodeBlock>,
        start: usize,
        call: Option<ScriptCall>,
        mode: Mode,
    ) -> Result<(), VmError> {
        Cx { ex: self, vm }.exec(block, start, call, mode)
    }

    fn result(&self) -> Vec<u8> {
        self.st().strs.string().to_vec()
    }

    fn set_result(&self, text: &[u8]) {
        self.st().strs.set_string(text);
    }

    fn save(&self) -> Box<dyn Any> {
        let st = self.st();
        Box::new(Saved {
            ints: st.ints.len(),
            floats: st.floats.len(),
            strs: st.strs.mark(),
            cur_var: st.cur_var.map(|v| (v, st.cur_key.clone())),
        })
    }

    fn restore(&self, saved: Box<dyn Any>, unwind: bool) {
        let Ok(saved) = saved.downcast::<Saved>() else {
            return;
        };
        let mut st = self.st();
        if unwind {
            st.ints.truncate(saved.ints);
            st.floats.truncate(saved.floats);
            st.strs.restore(saved.strs);
        }
        match saved.cur_var {
            Some((var, key)) => {
                st.cur_var = Some(var);
                st.cur_key = key;
                st.cur_stamp = 0;
            }
            None => st.cur_var = None,
        }
    }

    fn take_work(&self) -> u64 {
        self.st().strs.take_written()
    }

    fn take_overflow(&self) -> bool {
        self.st().strs.take_overflow()
    }

    fn format(&self) -> &dyn DsoFormat {
        self.format
    }
}

/// A run of the classic loop: the executor and the `Vm` it runs on. Script-to-script calls recurse inside one
/// `Cx`; a native that runs script again goes through the `Vm` and gets a `Cx` of its own.
struct Cx<'a> {
    ex: &'a ClassicExecutor,
    vm: &'a mut Vm,
}

impl<'a> Cx<'a> {
    /// The executor's state, borrowed until the guard is dropped (see the module documentation).
    fn st(&self) -> RefMut<'a, ClassicState> {
        self.ex.state.borrow_mut()
    }

    #[inline(never)]
    fn set_string(&self, text: &[u8]) {
        self.st().strs.set_string(text);
    }

    // ---- variables ----

    #[inline(never)]
    fn set_cur_var(&mut self, name: &[u8], create: bool) {
        let is_global = name.first() == Some(&b'$');
        let mut st = self.st();
        let st = &mut *st;
        crate::vars::refill(&mut st.cur_key, name);
        st.cur_key.make_ascii_lowercase();
        let key = &st.cur_key;
        let mut no_frame = false;
        let (var, table) = if is_global {
            (VarRef::Global, Some(self.vm.global_table_mut()))
        } else if let Some(frame) = self.vm.local_frames().len().checked_sub(1) {
            (
                VarRef::Local(frame),
                self.vm.local_frames_mut().get_mut(frame),
            )
        } else {
            no_frame = true;
            (VarRef::Global, None)
        };
        st.cur_var = table.and_then(|table| {
            let slot = if create {
                Some(table.add_slot_folded(key, name))
            } else {
                table.slot_folded(key)
            }?;
            st.cur_slot = slot;
            st.cur_stamp = table.stamp();
            Some(var)
        });
        if no_frame && create {
            self.vm.warn(format!(
                "Accessing local variable in global scope... failed: {}",
                String::from_utf8_lossy(name)
            ));
        }
    }

    /// Runs `f` on the current variable, if there is one.
    fn with_entry<R>(&mut self, f: impl FnOnce(&mut Entry, &mut ClassicState) -> R) -> Option<R> {
        let mut st = self.st();
        let st = &mut *st;
        let table = match st.cur_var? {
            VarRef::Global => self.vm.global_table_mut(),
            VarRef::Local(frame) => self.vm.local_frames_mut().get_mut(frame)?,
        };
        let entry = if table.stamp() == st.cur_stamp {
            table.at_mut(st.cur_slot)
        } else {
            table.get_mut_folded(&st.cur_key)
        }?;
        Some(f(entry, st))
    }

    // ---- the interpreter ----

    fn exec(
        &mut self,
        block: &Rc<CodeBlock>,
        ip: usize,
        call: Option<ScriptCall>,
        mode: Mode,
    ) -> Result<(), VmError> {
        self.vm.check_call_depth()?;
        let mark = self.vm.enter_frame(block, call.as_ref(), mode);
        let result = self.exec_loop(block, ip, call.as_ref(), mode);
        self.vm.leave_frame(mark);
        if let Some(call) = call {
            self.recycle_argv(call.argv);
        }
        result
    }

    /// Keeps a finished call's argument list for the next call (bounded: a few dozen lists of short arguments).
    #[inline(never)]
    fn recycle_argv(&mut self, argv: Vec<Vec<u8>>) {
        const LISTS: usize = 64;
        const ARGUMENT_BYTES: usize = 1024;
        let mut st = self.st();
        if st.argv_pool.len() < LISTS
            && argv.capacity() <= strstack::MAX_ARGS + 1
            && argv.iter().all(|a| a.capacity() <= ARGUMENT_BYTES)
        {
            st.argv_pool.push(argv);
        }
    }

    /// The evaluation loop. Everything with more than a line of logic lives in a small method, so that
    /// this frame, which is multiplied by the call depth, stays small and 256 nested calls fit in a
    /// default thread stack even in a debug build.
    fn exec_loop(
        &mut self,
        block: &Rc<CodeBlock>,
        mut ip: usize,
        call: Option<&ScriptCall>,
        mode: Mode,
    ) -> Result<(), VmError> {
        let in_function = call.is_some();
        if let Some(call) = call {
            ip = self.bind_params(block, ip, call)?;
        }

        loop {
            let op_ip = ip;
            let op = self.fetch_op(block, op_ip)?;
            ip += 1;

            match op {
                Op::FuncDecl => ip = self.declare_function(block, ip, mode)?,

                Op::Return => return Ok(()),
                Op::Jmp
                | Op::JmpIfNot
                | Op::JmpIf
                | Op::JmpIfFNot
                | Op::JmpIfF
                | Op::JmpIfNotNp
                | Op::JmpIfNp => ip = self.jump(op, block.words(), ip)?,

                // Comparisons: top OP second, result on the integer stack; integer and float binary operations,
                // unary operations and conversions.
                Op::CmpEq
                | Op::CmpGr
                | Op::CmpGe
                | Op::CmpLt
                | Op::CmpLe
                | Op::CmpNe
                | Op::Xor
                | Op::Mod
                | Op::BitAnd
                | Op::BitOr
                | Op::Shr
                | Op::Shl
                | Op::And
                | Op::Or
                | Op::Not
                | Op::NotF
                | Op::OnesComplement
                | Op::Neg
                | Op::Add
                | Op::Sub
                | Op::Mul
                | Op::Div
                | Op::StrToUint
                | Op::StrToFlt
                | Op::StrToNone
                | Op::FltToUint
                | Op::FltToStr
                | Op::FltToNone
                | Op::UintToFlt
                | Op::UintToStr
                | Op::UintToNone => self.arithmetic(op)?,

                // Variables.
                Op::SetCurVar | Op::SetCurVarCreate => {
                    self.set_cur_var_operand(block, ip, op == Op::SetCurVarCreate)?;
                    ip += 1;
                }
                Op::SetCurVarArray | Op::SetCurVarArrayCreate => {
                    self.set_cur_var_from_string(op == Op::SetCurVarArrayCreate);
                }
                Op::LoadVarUint | Op::LoadVarFlt | Op::LoadVarStr => self.load_var(op)?,
                Op::SaveVarUint | Op::SaveVarFlt | Op::SaveVarStr => self.save_var(op)?,

                // Immediates: each takes one operand word.
                Op::LoadImmedUint
                | Op::LoadImmedFlt
                | Op::LoadImmedStr
                | Op::TagToStr
                | Op::LoadImmedIdent
                | Op::AdvanceStrAppendChar => {
                    self.immediate(block, op, op_ip, in_function)?;
                    ip += 1;
                }

                // String stack.
                Op::AdvanceStr
                | Op::AdvanceStrComma
                | Op::AdvanceStrNul
                | Op::RewindStr
                | Op::TerminateRewindStr
                | Op::CompareStr
                | Op::Push
                | Op::PushFrame => self.string_op(op)?,

                // The object opcodes share this arm: a separate arm adds a few hundred bytes to this
                // frame, which is multiplied by the call depth.
                Op::CallFunc
                | Op::CallFuncResolve
                | Op::CreateObject
                | Op::AddObject
                | Op::EndObject
                | Op::SetCurObject
                | Op::SetCurObjectNew
                | Op::SetCurField
                | Op::SetCurFieldArray
                | Op::LoadFieldUint
                | Op::LoadFieldFlt
                | Op::LoadFieldStr
                | Op::SaveFieldUint
                | Op::SaveFieldFlt
                | Op::SaveFieldStr => self.call_function(block, op, &mut ip, call, mode)?,

                Op::Invalid | Op::Break => {
                    return Err(VmError::Unsupported(op.name()));
                }
            }
        }
    }

    /// Counts the step, reads the opcode word at `ip` and decodes it.
    #[inline]
    fn fetch_op(&mut self, block: &CodeBlock, ip: usize) -> Result<Op, VmError> {
        // Every byte the string stack wrote (and every argument byte a call collected) since the last
        // instruction costs one work unit.
        let written = {
            let mut st = self.st();
            if st.strs.take_overflow() {
                return Err(VmError::StringTooLong);
            }
            st.strs.take_written()
        };
        self.vm.charge_instruction(written)?;
        let word = fetch(block.words(), ip)?;
        self.ex
            .decoder
            .decode(word)
            .ok_or(VmError::InvalidOpcode { ip, word })
    }

    /// The jump opcodes. `ip` points at the jump target operand; returns the next instruction pointer.
    #[inline]
    fn jump(&mut self, op: Op, code: &[u32], ip: usize) -> Result<usize, VmError> {
        if self.st().jump_taken(op)? {
            Ok(fetch(code, ip)? as usize)
        } else {
            Ok(ip + 1)
        }
    }

    #[inline(never)]
    fn arithmetic(&mut self, op: Op) -> Result<(), VmError> {
        let float_result = self.ex.rules.float_result;
        let mut st = self.st();
        match op {
            Op::CmpEq | Op::CmpGr | Op::CmpGe | Op::CmpLt | Op::CmpLe | Op::CmpNe => st.compare(op),
            Op::Not | Op::NotF | Op::OnesComplement | Op::Neg => st.unary(op),
            Op::Add | Op::Sub | Op::Mul | Op::Div => st.float_binary(op, float_result),
            Op::Xor | Op::Mod | Op::BitAnd | Op::BitOr | Op::Shr | Op::Shl | Op::And | Op::Or => {
                st.int_binary(op)
            }
            _ => st.convert(op),
        }
    }

    #[inline(never)]
    fn string_op(&mut self, op: Op) -> Result<(), VmError> {
        self.st().string_op(op)
    }

    /// The opcodes that carry one operand word, which sits right after the opcode at `op_ip`.
    #[inline(never)]
    fn immediate(
        &mut self,
        block: &CodeBlock,
        op: Op,
        op_ip: usize,
        in_function: bool,
    ) -> Result<(), VmError> {
        let operand = fetch(block.words(), op_ip + 1)?;
        if op == Op::TagToStr {
            return self.tag_to_str(block, operand, op_ip, in_function);
        }
        let mut st = self.st();
        match op {
            Op::LoadImmedUint => st.push_u(operand)?,
            Op::LoadImmedFlt => {
                let v = *block
                    .floats(in_function)
                    .get(operand as usize)
                    .ok_or(VmError::BadIndex { ip: op_ip })?;
                st.push_f(v)?;
            }
            Op::LoadImmedStr => {
                let s = block
                    .strings(in_function)
                    .get(operand as usize..)
                    .ok_or(VmError::BadIndex { ip: op_ip })?;
                st.strs.set_string(s);
            }
            Op::LoadImmedIdent => {
                let name = block.ident(operand).unwrap_or(b"");
                st.strs.set_string(name);
            }
            _ => st.strs.advance_char(operand as u8)?,
        }
        Ok(())
    }

    /// OP_TAG_TO_STR: the first time a block's tagged literal runs in this `Vm`, its text enters the tag table (one
    /// engine reference) and the literal becomes `"\x01<id>"` for good (TGE writes the id over the string in place,
    /// at most [`ClassicRules::tag_digits`] digits, and rewrites the opcode into OP_LOADIMMED_STR); a literal that
    /// already starts with 0x01 is left as it is. Then the text is loaded like OP_LOADIMMED_STR.
    ///
    /// The operand can point anywhere in a hostile block's string table, so the copy into the tag table
    /// is charged to the step budget by its length (one work unit per byte, as the string stack's
    /// writes are); a full table fails the run (see `tags`).
    #[inline(never)]
    fn tag_to_str(
        &mut self,
        block: &CodeBlock,
        operand: u32,
        op_ip: usize,
        in_function: bool,
    ) -> Result<(), VmError> {
        let vm_id = self.vm.vm_id();
        if let Some(text) = block.tag_conversion(in_function, operand, vm_id) {
            self.set_string(&text);
            return Ok(());
        }
        let literal = until_nul(
            block
                .strings(in_function)
                .get(operand as usize..)
                .ok_or(VmError::BadIndex { ip: op_ip })?,
        );
        self.vm.objects_mut().add_work(literal.len() as u64);
        let text = if literal.first() == Some(&1) {
            literal.to_vec()
        } else {
            let id = self
                .vm
                .tags_mut()
                .add_string(literal)
                .ok_or(VmError::TagTableFull)?;
            tag_text(id, self.ex.rules.tag_digits)
        };
        self.set_string(&text);
        block.set_tag_conversion(in_function, operand, vm_id, text);
        Ok(())
    }

    // ---- variable opcodes, kept out of `exec_loop` to keep its stack frame small ----

    #[inline(never)]
    fn set_cur_var_operand(
        &mut self,
        block: &CodeBlock,
        ip: usize,
        create: bool,
    ) -> Result<(), VmError> {
        let name = block.ident(fetch(block.words(), ip)?).unwrap_or(b"");
        self.set_cur_var(name, create);
        Ok(())
    }

    #[inline(never)]
    fn set_cur_var_from_string(&mut self, create: bool) {
        let name = {
            let mut st = self.st();
            let st = &mut *st;
            let mut name = std::mem::take(&mut st.name_scratch);
            crate::vars::refill(&mut name, st.strs.string());
            name
        };
        self.set_cur_var(&name, create);
        self.st().name_scratch = name;
    }

    /// LOADVAR_UINT / _FLT / _STR: the current variable (0 / 0 / "" without one).
    #[inline(never)]
    fn load_var(&mut self, op: Op) -> Result<(), VmError> {
        match op {
            Op::LoadVarUint => {
                let v = self.with_entry(|e, _| e.get_int()).unwrap_or(0);
                self.st().push_u(v)
            }
            Op::LoadVarFlt => {
                let v = self.with_entry(|e, _| e.get_float()).unwrap_or(0.0);
                self.st().push_f(v)
            }
            _ => {
                let found = self.with_entry(|e, st| e.with_string(|v| st.strs.set_string(v)));
                if found.is_none() {
                    self.set_string(b"");
                }
                Ok(())
            }
        }
    }

    /// SAVEVAR_UINT / _FLT / _STR: the top of the integer or float stack (left there), or the current string,
    /// into the current variable.
    #[inline(never)]
    fn save_var(&mut self, op: Op) -> Result<(), VmError> {
        match op {
            Op::SaveVarUint => {
                let v = *self.st().top_u()?;
                self.with_entry(|e, _| e.set_int(v));
            }
            Op::SaveVarFlt => {
                let v = *self.st().floats.last().ok_or(VmError::StackUnderflow)?;
                self.with_entry(|e, _| e.set_float(v));
            }
            _ => {
                self.with_entry(|e, st| e.set_string(st.strs.string()));
            }
        }
        Ok(())
    }

    /// Sets the declared parameters of a script function from the call's arguments and returns the
    /// index of the function's first instruction. `ip` points at the first operand of the `FuncDecl`:
    /// name, namespace, package, hasBody, end, argc, then the parameter names.
    #[inline(never)]
    fn bind_params(
        &mut self,
        block: &CodeBlock,
        ip: usize,
        call: &ScriptCall,
    ) -> Result<usize, VmError> {
        let code = block.words();
        let declared = fetch(code, ip + 5)? as usize;
        let given = (call.argv.len().saturating_sub(1)).min(declared);
        for i in 0..given {
            let param = block.ident(fetch(code, ip + 6 + i)?).unwrap_or(b"");
            self.set_cur_var(param, true);
            self.with_entry(|e, _| e.set_string(&call.argv[i + 1]));
        }
        ip.checked_add(declared)
            .and_then(|end| end.checked_add(6))
            .ok_or(VmError::CodeOutOfRange { ip })
    }

    /// A declaration the declare policy refused: the rules' error line, if they have one.
    #[cold]
    #[inline(never)]
    fn refuse_declaration(&mut self, namespace: &[u8], name: &[u8]) {
        if let Some(line) = self.ex.rules.refused_declaration {
            self.vm.error(line(namespace, name));
        }
    }

    /// `OP_FUNC_DECL`: registers the function (unless `no_calls`) and returns the index to continue at.
    #[inline(never)]
    fn declare_function(
        &mut self,
        block: &Rc<CodeBlock>,
        ip: usize,
        mode: Mode,
    ) -> Result<usize, VmError> {
        let code = block.words();
        // A non-zero identifier operand that names nothing is a bad index, not an empty name.
        let ident = |word: u32| match block.ident(word) {
            None if word != 0 => Err(VmError::BadIndex { ip }),
            found => Ok(found.map(<[u8]>::to_vec)),
        };
        let name = ident(fetch(code, ip)?)?.unwrap_or_default();
        let namespace = ident(fetch(code, ip + 1)?)?.unwrap_or_default();
        let package = ident(fetch(code, ip + 2)?)?;
        let has_body = fetch(code, ip + 3)? != 0;
        let end = fetch(code, ip + 4)? as usize;
        // Under `nocalls` the whole declaration is skipped. Otherwise `Namespace::find(namespace, package)` comes
        // first, so a package exists even when its declaration is then skipped or refused. With
        // [`ClassicRules::keep_natives`] a declaration that would replace a native is skipped silently (with the
        // packages unlinked, the name is looked up in the base layer of the namespace and its class parents; for
        // a global function in the global namespace too). Then the declare policy.
        if !mode.is_no_calls() {
            self.vm
                .namespaces_mut()
                .touch(&namespace, package.as_deref());
        }
        let kept = |vm: &Vm| {
            vm.namespaces()
                .shadows_native(&namespace, package.as_deref(), &name)
        };
        let skipped = mode.is_no_calls() || (self.ex.rules.keep_natives && kept(self.vm));
        if !skipped {
            if !self.vm.may_declare(&namespace, &name, mode.is_trusted()) {
                self.refuse_declaration(&namespace, &name);
                return Ok(end);
            }
            // An existing entry in that layer is redefined in place: call sites bound to it, and the
            // function that may be running right now, are unaffected until they next read it.
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

    /// The arguments of a call: the call frame on the string stack, into a recycled list.
    #[inline(never)]
    fn take_args(&mut self, name: &[u8]) -> Result<Vec<Vec<u8>>, VmError> {
        let mut st = self.st();
        let mut argv = st.argv_pool.pop().unwrap_or_default();
        st.strs.args_into(name, &mut argv)?;
        Ok(argv)
    }

    /// `OP_CALLFUNC` / `OP_CALLFUNC_RESOLVE`. Only the lookup and the script-call path live here; the
    /// rest is split out so that this frame, which is multiplied by the call depth, stays small and 256
    /// nested calls fit in a default thread stack.
    ///
    /// Operands: function name, namespace, call type. Call type 0 is a plain call (bound per call site,
    /// see [`Cx::resolve_plain`]), 1 a method call on the object `argv[1]` names (looked up in the
    /// object's namespace chain, every time; the compiler always emits it with a null namespace),
    /// 2 is `Parent::` (looked up below the layer of the function that is running, every time);
    /// anything else is not supported.
    ///
    /// The object opcodes are dispatched from here as well (`object_op`): giving them their own arm in
    /// `exec_loop` made that frame about 250 bytes larger (63 KB at 256 levels, measured), sharing this
    /// arm costs nothing.
    #[inline(never)]
    fn call_function(
        &mut self,
        block: &Rc<CodeBlock>,
        op: Op,
        ip: &mut usize,
        running: Option<&ScriptCall>,
        mode: Mode,
    ) -> Result<(), VmError> {
        if !matches!(op, Op::CallFunc | Op::CallFuncResolve) || mode.is_no_calls() {
            // Under `nocalls` a call opcode is skipped by `object_op` too (a separate check here costs
            // every level of recursion some stack).
            // ADD_OBJECT can run a script `onAdd`: it skips `object_op`'s frame.
            return if op == Op::AddObject {
                self.add_object(block, ip)
            } else {
                self.object_op(block, op, ip, mode)
            };
        }
        let op_ip = ip.saturating_sub(1);
        let name = block.ident(fetch(block.words(), *ip)?).unwrap_or(b"");
        fetch(block.words(), *ip + 1)?;
        fetch(block.words(), *ip + 2)?;
        *ip += 3;
        let argv = self.take_args(name)?;
        let Some((entry, callee)) = self.resolve_call(block, op, op_ip, running, name, &argv)?
        else {
            return Ok(());
        };
        let Some((target_block, offset)) = script_target(&entry) else {
            return self.call_native(block, name, argv, (entry, callee, running), ip);
        };
        if offset == 0 {
            self.set_string(b"");
            return Ok(());
        }
        // A method call makes its object the current one (TGE's `thisObject`) until it returns;
        // plain and Parent:: calls keep the current one, so natives they reach see it.
        // It also holds the object against deletion until it returns; only this path does.
        let outer = self.vm.this_object();
        if let Callee::Method(object) = callee {
            self.vm.set_this_object(Some(object));
            self.vm.hold(object);
        }
        let call = self.vm.new_call(argv, entry);
        let mut result = self.exec(&target_block, offset, Some(call), Mode::PLAIN);
        if let Callee::Method(object) = callee {
            result = self.vm.release_after(object, result);
        }
        self.vm.set_this_object(outer);
        result
    }

    /// A call under `exec(file, true)`: the arguments are taken and the result is "" (TGE
    /// `if(!nsEntry || noCalls)`). A plain call to a function that does not exist still warns as usual
    /// (`OP_CALLFUNC_RESOLVE`'s lookup and its "Unable to find function" come before any noCalls test), and so does a
    /// method call on an object that does not exist (`Unable to find object`, before the noCalls test; unless
    /// [`ClassicRules::quiet_missing_object_without_calls`]). A `Parent::` call names no object: it is skipped silently.
    #[cold]
    #[inline(never)]
    fn skip_call(&mut self, block: &CodeBlock, op: Op, ip: &mut usize) -> Result<(), VmError> {
        let op_ip = ip.saturating_sub(1);
        let name = block
            .ident(fetch(block.words(), *ip)?)
            .unwrap_or(b"")
            .to_vec();
        let call_type = fetch(block.words(), *ip + 2)?;
        *ip += 3;
        let argv = self.st().strs.args(&name)?;
        if call_type == 0 && self.resolve_plain(block, op, op_ip, &name).is_none() {
            return self.call_missing(block, op, op_ip, &name);
        }
        if call_type == 1 && !self.ex.rules.quiet_missing_object_without_calls {
            let target = argv.get(1).map_or(&b""[..], Vec::as_slice);
            if self.vm.objects().find(target).is_none() {
                return self
                    .vm
                    .warn_missing_object(block.file_line(op_ip), target, &name);
            }
        }
        self.set_string(b"");
        Ok(())
    }

    /// Finds what a call opcode calls: the entry, and for a method call the object. A function that
    /// does not exist warns and gives None (the call is over); an unsupported call type is an error.
    #[inline(never)]
    fn resolve_call(
        &mut self,
        block: &CodeBlock,
        op: Op,
        op_ip: usize,
        running: Option<&ScriptCall>,
        name: &[u8],
        argv: &[Vec<u8>],
    ) -> Result<Option<(EntryRef, Callee)>, VmError> {
        let call_type = fetch(block.words(), op_ip + 3)?;
        let (entry, callee) = match call_type {
            0 => (self.resolve_plain(block, op, op_ip, name), Callee::Plain),
            1 => match self.resolve_method((block, op_ip), name, argv)? {
                Some((entry, object)) => (Some(entry), Callee::Method(object)),
                None => return Ok(None),
            },
            2 => (
                self.resolve_parent(running, name),
                if running.is_some() {
                    Callee::Parent
                } else {
                    Callee::Plain
                },
            ),
            _ => return Err(VmError::Unsupported(op.name())),
        };
        let Some(entry) = entry else {
            self.call_missing(block, op, op_ip, name)?;
            return Ok(None);
        };
        Ok(Some((entry, callee)))
    }

    /// A method call: `argv[1]` names the object, its namespace chain holds the method. A missing
    /// object or method warns and leaves the empty string; None then.
    #[inline(never)]
    fn resolve_method(
        &mut self,
        (block, op_ip): (&CodeBlock, usize),
        name: &[u8],
        argv: &[Vec<u8>],
    ) -> Result<Option<(EntryRef, ObjId)>, VmError> {
        let target = argv.get(1).map_or(&b""[..], Vec::as_slice);
        let Some(object) = self.vm.objects().find(target) else {
            self.vm
                .warn_missing_object(block.file_line(op_ip), target, name)?;
            return Ok(None);
        };
        let found = {
            let namespace = self
                .vm
                .objects()
                .get(object)
                .map_or(&b""[..], |o| o.namespace());
            self.vm.namespaces().lookup(namespace, name)
        };
        match found {
            Some(entry) => Ok(Some((entry, object))),
            None => {
                let namespace = self
                    .vm
                    .objects()
                    .get(object)
                    .map(|o| o.namespace().to_vec())
                    .unwrap_or_default();
                self.vm
                    .warn_unknown_method(block.file_line(op_ip), name, object, &namespace);
                Ok(None)
            }
        }
    }

    /// A plain function call: the call site's binding if it has one, otherwise a lookup in the active
    /// layers. `OP_CALLFUNC_RESOLVE` then binds the site to the entry it found, as TGE does by
    /// rewriting the opcode; the binding is never re-resolved (package changes do not affect it), but
    /// it sees a redefinition because it refers to the entry itself.
    #[inline(never)]
    fn resolve_plain(
        &self,
        block: &CodeBlock,
        op: Op,
        op_ip: usize,
        name: &[u8],
    ) -> Option<EntryRef> {
        // A binding made by another `Vm` that ran the same block, or before `resetAllOpCallFunc`, does not
        // count (see `CodeBlock`).
        let epoch = self.vm.binding_epoch();
        if let Some(bound) = block.bound_site(op_ip, epoch) {
            return Some(bound);
        }
        let namespace = block.ident(block.words().get(op_ip + 2).copied()?);
        let found = self.vm.namespaces().lookup(namespace.unwrap_or(b""), name);
        if op == Op::CallFuncResolve
            && let Some(entry) = &found
        {
            block.bind_site(op_ip, epoch, entry);
        }
        found
    }

    /// `Parent::name` from the running function: found through the layer position the function was
    /// called from, which `Parent::` reads against the chain as it is now (see [`Holder`](crate::ns::Holder)).
    /// There is nothing at top level.
    #[inline(never)]
    fn resolve_parent(&self, running: Option<&ScriptCall>, name: &[u8]) -> Option<EntryRef> {
        let running = running?;
        let entry = running.entry.borrow();
        self.vm
            .namespaces()
            .lookup_parent(&entry.namespace, running.holder, name)
    }

    /// The function does not exist. `OP_CALLFUNC_RESOLVE` warns "Unable to find function" with the
    /// qualified name and leaves the result as it is; with the loud report a BackTrace follows, which leaves the
    /// empty string. The other calls warn "Unknown command" (a `Parent::` call names the bare function) and
    /// empty the result. `op_ip` is the code index of the call opcode: its operands give the namespace and the
    /// call type, its line goes in the message.
    #[cold]
    #[inline(never)]
    fn call_missing(
        &mut self,
        block: &CodeBlock,
        op: Op,
        op_ip: usize,
        name: &[u8],
    ) -> Result<(), VmError> {
        let label = block.file_line(op_ip);
        let shown = String::from_utf8_lossy(name);
        let operand = |at: usize| block.words().get(op_ip + at).copied().unwrap_or(0);
        let call_type = operand(3);
        let qualified = match block.ident(operand(2)) {
            Some(ns) if call_type == 0 => format!("{}::{shown}", String::from_utf8_lossy(ns)),
            _ => shown.into_owned(),
        };
        if op == Op::CallFuncResolve && call_type == 0 {
            self.vm
                .warn(format!("{label}: Unable to find function {qualified}"));
            if !self.vm.console_rules().backtraces {
                return Ok(());
            }
            self.vm.print_backtrace()?;
        } else {
            self.vm
                .warn(format!("{label}: Unknown command {qualified}."));
        }
        self.set_string(b"");
        Ok(())
    }

    /// Calls a native function after checking its argument count; a method call makes its object the
    /// current one while the native runs. `ip` is the next instruction; when it was consumed by the result
    /// delivery, `ip` moves past it. This frame stays alive while the native runs (which may re-enter
    /// the VM), so the warning paths live in out-of-line helpers.
    #[inline(never)]
    fn call_native(
        &mut self,
        block: &CodeBlock,
        name: &[u8],
        argv: Vec<Vec<u8>>,
        (entry, callee, running): (EntryRef, Callee, Option<&ScriptCall>),
        ip: &mut usize,
    ) -> Result<(), VmError> {
        let Callable::Native { min, max, usage, f } = entry.borrow().callable.clone() else {
            return Ok(());
        };
        // `ip` is past the three operands: the call opcode is four words back (`op_ip`, not kept in a
        // local here: this frame stays alive while the native runs).
        if !argc_ok(argv.len(), min, max) {
            return self.warn_wrong_argc_in_script(block, *ip, callee, running, name, &usage);
        }
        let outer = self.vm.this_object();
        if let Callee::Method(object) = callee {
            self.vm.set_this_object(Some(object));
        }
        let call = native_call(block, name, argv, &entry, self.vm.this_object());
        let profiled = self.vm.profiling();
        if profiled {
            self.vm.profile_enter(&entry);
        }
        let result = f(self.vm, &call);
        if profiled {
            self.vm.profile_leave();
        }
        self.recycle_argv(call.argv);
        self.vm.set_this_object(outer);
        let next = block
            .code()
            .get(*ip)
            .and_then(|w| self.ex.decoder.decode(*w));
        if self.deliver_native(result, next, name, block, *ip, running)? {
            *ip += 1;
        }
        Ok(())
    }

    /// "Call to X in F uses result of void function call.", where F is the running function's declared name, or
    /// the code block's name at its top level (`exec` runs a file under its name); code without one (evaluated
    /// text) prints `(null)`. A warning; with the loud report, a named one is an error between blank lines with a
    /// BackTrace.
    #[cold]
    #[inline(never)]
    fn warn_void_result(
        &mut self,
        block: &CodeBlock,
        ip: usize,
        running: Option<&ScriptCall>,
        name: &[u8],
    ) -> Result<(), VmError> {
        // The message is built in a helper that has returned before the BackTrace runs a script
        // (a replaced `backtrace` can end up here again): this frame stays small.
        if self.log_void_result(block, ip, running, name) {
            self.vm.print_backtrace()?;
            self.vm.print(b"");
        }
        Ok(())
    }

    /// Logs the message of [`warn_void_result`](Self::warn_void_result); true when a BackTrace and a
    /// blank line follow.
    #[cold]
    #[inline(never)]
    fn log_void_result(
        &mut self,
        block: &CodeBlock,
        ip: usize,
        running: Option<&ScriptCall>,
        name: &[u8],
    ) -> bool {
        let label = block.file_line(ip.saturating_sub(4));
        let function = match running {
            Some(call) => script_name(&call.entry),
            None => block.name().map(<[u8]>::to_vec),
        };
        let shown = lossy(name);
        match function {
            Some(function) if !self.vm.console_rules().backtraces => {
                self.vm.warn(format!(
                    "{label}: Call to {shown} in {} uses result of void function call.",
                    lossy(&function)
                ));
                false
            }
            Some(function) => {
                self.vm.print(b"");
                self.vm.error(format!(
                    "{label}: Call to {shown} in {} uses result of void function call.",
                    lossy(&function)
                ));
                true
            }
            None => {
                self.vm.warn(format!(
                    "{label}: Call to {shown} in (null) uses result of void function call."
                ));
                false
            }
        }
    }

    /// Delivers a native function's result as TGE does, peeking at the next opcode.
    /// Returns true when that opcode was consumed and must be skipped. `block`, `ip` (the index after
    /// the call opcode's operands: the opcode is four words back) and `running` locate the call for the
    /// void-result warning.
    #[inline(never)]
    fn deliver_native(
        &mut self,
        result: Native,
        next: Option<Op>,
        name: &[u8],
        block: &CodeBlock,
        ip: usize,
        running: Option<&ScriptCall>,
    ) -> Result<bool, VmError> {
        match result {
            Native::Fail(e) => Err(e),
            Native::Void => {
                if next != Some(Op::StrToNone) {
                    self.warn_void_result(block, ip, running, name)?;
                }
                self.set_string(b"");
                Ok(false)
            }
            typed => self.st().deliver_typed(&typed, next),
        }
    }

    /// A script call with the wrong argument count: the warning names the namespace of the object a method was
    /// called on, the namespace a `Parent::` lookup starts in (the one below the running function's), and nothing
    /// before `::` for a plain call, then the usage. The result is left as it is; with the loud report the pair sits
    /// between blank lines and is followed by a BackTrace, which leaves the empty string.
    #[cold]
    #[inline(never)]
    fn warn_wrong_argc_in_script(
        &mut self,
        block: &CodeBlock,
        ip: usize,
        callee: Callee,
        running: Option<&ScriptCall>,
        name: &[u8],
        usage: &str,
    ) -> Result<(), VmError> {
        let namespaces = self.vm.namespaces();
        let scope = match callee {
            Callee::Plain => Vec::new(),
            Callee::Method(id) => self
                .vm
                .objects()
                .get(id)
                .map(|o| namespaces.display(o.namespace()))
                .unwrap_or_default(),
            Callee::Parent => running
                .and_then(|call| {
                    let entry = call.entry.borrow();
                    namespaces.parent_scope(&entry.namespace, call.holder)
                })
                .map(|scope| namespaces.display(&scope))
                .unwrap_or_default(),
        };
        let label = block.file_line(ip.saturating_sub(4));
        let loud = self.vm.console_rules().backtraces;
        if loud {
            self.vm.print(b"");
        }
        self.vm.warn(format!(
            "{label}: {}::{} - wrong number of arguments.",
            lossy(&scope),
            lossy(name)
        ));
        self.vm.warn(format!("{label}: usage: {usage}"));
        if !loud {
            return Ok(());
        }
        self.vm.print_backtrace()?;
        self.vm.print(b"");
        // The call skips the result delivery, so it returns what the string stack holds: the empty string the
        // `backtrace();` it evaluated left there (a void native's result).
        self.set_string(b"");
        Ok(())
    }

    // ---- the object opcodes: `new` and `datablock` declarations and field access ----

    /// Runs one object opcode (dispatched from `call_function`, see there); `ip` is its first operand
    /// and is moved past the operands the opcode consumed.
    #[inline(never)]
    fn object_op(
        &mut self,
        block: &CodeBlock,
        op: Op,
        ip: &mut usize,
        mode: Mode,
    ) -> Result<(), VmError> {
        let at = *ip;
        match op {
            Op::CreateObject => *ip = self.create_object(block, at, mode.is_no_calls())?,
            Op::AddObject => self.add_object(block, ip)?,
            // Reached only under `exec(file, true)` (see `call_function`).
            Op::CallFunc | Op::CallFuncResolve => self.skip_call(block, op, ip)?,
            Op::EndObject => {
                // A child declaration pops its id.
                if fetch(block.words(), at)? == 0 {
                    self.st().pop_u()?;
                }
                *ip = at + 1;
            }
            Op::SetCurObject => self.set_cur_object(),
            Op::SetCurObjectNew => self.st().cur_object = FieldTarget::New,
            Op::SetCurField => {
                self.set_cur_field(block, at)?;
                *ip = at + 1;
            }
            Op::SetCurFieldArray => self.set_cur_field_array(),
            Op::LoadFieldUint | Op::LoadFieldFlt | Op::LoadFieldStr => self.load_field(op)?,
            Op::SaveFieldUint | Op::SaveFieldFlt | Op::SaveFieldStr => self.save_field(op)?,
            _ => return Err(VmError::Unsupported(op.name())),
        }
        Ok(())
    }

    /// OP_CREATE_OBJECT. `ip` is the first operand: the parent object's name (identifier, 0 for none),
    /// the datablock flag, the code index where the declaration ends. The class, the name and the
    /// constructor arguments were pushed as a call frame. Returns the next instruction: the body, or the
    /// end of the declaration when the object cannot be made.
    #[inline(never)]
    fn create_object(
        &mut self,
        block: &CodeBlock,
        ip: usize,
        no_calls: bool,
    ) -> Result<usize, VmError> {
        let code = block.words();
        let parent_word = fetch(code, ip)?;
        let is_datablock = fetch(code, ip + 1)? != 0;
        let fail = fetch(code, ip + 2)? as usize;
        let parent = match block.ident(parent_word) {
            None if parent_word != 0 => return Err(VmError::BadIndex { ip }),
            found => found.map(<[u8]>::to_vec),
        };
        let argv = self.st().strs.args(b"")?;
        self.vm.begin_new_object(
            (block, ip.saturating_sub(1)),
            (parent, is_datablock),
            (ip + 3, fail),
            argv,
            no_calls,
        )
    }

    /// OP_ADD_OBJECT. `ip` is its operand, the place-at-root flag, and moves past it (to the fail jump
    /// when the object cannot be registered); see [`Vm::add_new_object`]. The id replaces the top-level
    /// 0 on the integer stack, or is pushed for the children.
    ///
    /// This frame is on the recursion path of a script `onAdd`, multiplied by the call depth.
    #[inline(never)]
    fn add_object(&mut self, block: &CodeBlock, ip: &mut usize) -> Result<(), VmError> {
        let at = *ip;
        let place_at_root = fetch(block.words(), at)? != 0;
        let ex = self.ex;
        let target = move || ex.st().top_u().map(|top| *top);
        match self
            .vm
            .add_new_object((block, at.saturating_sub(1)), place_at_root, &target)?
        {
            AddedObject::Failed(fail) => *ip = fail,
            AddedObject::Added(id) => {
                self.place_id(id, place_at_root)?;
                *ip = at + 1;
            }
        }
        Ok(())
    }

    /// Where a new object's id goes (see `add_object`).
    #[inline(never)]
    fn place_id(&mut self, id: ObjId, place_at_root: bool) -> Result<(), VmError> {
        let mut st = self.st();
        if place_at_root {
            *st.top_u()? = id;
            Ok(())
        } else {
            st.push_u(id)
        }
    }

    /// OP_SETCUROBJECT: the object the current string names (`Sim::findObject`), or none.
    #[inline(never)]
    fn set_cur_object(&mut self) {
        let mut st = self.st();
        st.cur_object = self
            .vm
            .objects()
            .find(st.strs.string())
            .map_or(FieldTarget::None, FieldTarget::Live);
    }

    /// OP_SETCURFIELD: `ip` is the identifier operand. The array index is cleared.
    #[inline(never)]
    fn set_cur_field(&mut self, block: &CodeBlock, ip: usize) -> Result<(), VmError> {
        let word = fetch(block.words(), ip)?;
        let name = match block.ident(word) {
            None if word != 0 => return Err(VmError::BadIndex { ip }),
            found => found.unwrap_or(b""),
        };
        let mut st = self.st();
        st.cur_field.clear();
        st.cur_field.extend_from_slice(name);
        st.cur_field_array.clear();
        Ok(())
    }

    /// OP_SETCURFIELD_ARRAY: the current string is the index (cut to the longest field name).
    #[inline(never)]
    fn set_cur_field_array(&mut self) {
        let mut st = self.st();
        let st = &mut *st;
        let index = st.strs.string();
        let keep = index.len().min(MAX_FIELD_NAME);
        st.cur_field_array.clear();
        st.cur_field_array.extend_from_slice(&index[..keep]);
    }

    /// OP_LOADFIELD_UINT / _FLT / _STR: the field as `atoi`, `atof` or text (0 / 0 / "" without an
    /// object).
    #[inline(never)]
    fn load_field(&mut self, op: Op) -> Result<(), VmError> {
        let value = {
            let st = self.st();
            self.vm
                .read_field_of(st.cur_object, &st.cur_field, &st.cur_field_array)
        };
        let mut st = self.st();
        match op {
            Op::LoadFieldUint => st.push_u(atoi(&value) as u32),
            // `dAtof`: rounded to a float, as `OP_STR_TO_FLT` and `OP_LOADVAR_FLT` are.
            Op::LoadFieldFlt => st.push_f(to_f64(to_f32(atof(&value)))),
            _ => {
                st.strs.set_string(&value);
                Ok(())
            }
        }
    }

    /// OP_SAVEFIELD_UINT / _FLT / _STR. The typed forms first write their value (left on its stack) to the
    /// string stack, as TGE does, then save that text. The field's name is taken out of the state
    /// while the write runs (a static-write observer may run script) and put back after.
    #[inline(never)]
    fn save_field(&mut self, op: Op) -> Result<(), VmError> {
        let (target, field, array, value) = {
            let mut st = self.st();
            match op {
                Op::SaveFieldUint => {
                    let v = *st.top_u()?;
                    st.strs.set_int(v);
                }
                Op::SaveFieldFlt => {
                    let v = *st.floats.last().ok_or(VmError::StackUnderflow)?;
                    st.strs.set_float(v);
                }
                _ => {}
            }
            let value = st.strs.string().to_vec();
            (
                st.cur_object,
                std::mem::take(&mut st.cur_field),
                std::mem::take(&mut st.cur_field_array),
                value,
            )
        };
        self.vm.write_field_of(target, &field, &array, &value);
        let mut st = self.st();
        st.cur_field = field;
        st.cur_field_array = array;
        Ok(())
    }
}
