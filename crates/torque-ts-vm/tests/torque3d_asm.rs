//! The Torque3D executor on code torque-ts-compiler does not write (docblocks, `OP_TERMINATE_REWIND_STR`,
//! `OP_SETCURFIELD_TYPE`, the debugger's `OP_BREAK`) and on broken code: hand-assembled DSOs (version 50, Torque3D's
//! numbering, names in two code words each).

use std::rc::Rc;

use torque_ts_bytecode::dsotext;
use torque_ts_bytecode::opcodes::Op;
use torque_ts_bytecode::{DsoFile, IdentEntry};
use torque_ts_vm::code::{CodeBlock, torque3d_format};
use torque_ts_vm::vm::{Vm, VmError};

/// A top-level block written word by word.
#[derive(Default)]
struct Asm {
    code: Vec<u32>,
    strings: Vec<u8>,
    idents: Vec<IdentEntry>,
}

impl Asm {
    fn op(&mut self, op: Op) -> &mut Asm {
        self.code.push(torque3d_format().opcodes.code(op));
        self
    }

    fn word(&mut self, w: u32) -> &mut Asm {
        self.code.push(w);
        self
    }

    /// A string of the global table; its offset.
    fn string(&mut self, text: &str) -> u32 {
        let at = u32::try_from(self.strings.len()).unwrap();
        self.strings.extend_from_slice(text.as_bytes());
        self.strings.push(0);
        at
    }

    /// A string operand.
    fn text(&mut self, text: &str) -> &mut Asm {
        let at = self.string(text);
        self.word(at)
    }

    /// A name operand: two words, the identifier table naming the first.
    fn ident(&mut self, name: &str) -> &mut Asm {
        let at = self.string(name);
        let position = u32::try_from(self.code.len()).unwrap();
        self.idents.push(IdentEntry {
            string_index: at,
            positions: vec![position],
        });
        self.word(0).word(0)
    }

    fn dso(&self) -> DsoFile {
        DsoFile {
            version: 50,
            global_strings: self.strings.clone(),
            global_floats: Vec::new(),
            function_strings: Vec::new(),
            function_floats: Vec::new(),
            local_variables: Vec::new(),
            code: self.code.clone(),
            line_breaks: Vec::new(),
            identifiers: self.idents.clone(),
        }
    }

    fn run(&self, vm: &mut Vm) -> Result<String, VmError> {
        let dso = self.dso();
        let block = CodeBlock::from_dso(&dso, Some(b"asm.cs"), torque3d_format())
            .unwrap_or_else(|e| panic!("{e}\n{}", dsotext::render(&dso, torque3d_format())));
        vm.run(&Rc::new(block))
            .map(|r| String::from_utf8_lossy(&r).into_owned())
    }
}

fn source(vm: &mut Vm, src: &str) {
    let r = vm.invoke(None, b"eval", &[src.as_bytes().to_vec()]);
    assert!(r.is_ok(), "{src}: {r:?}");
}

#[test]
fn terminate_rewind_joins_the_two_top_values() {
    let mut a = Asm::default();
    a.op(Op::LoadImmedStr).text("ab");
    a.op(Op::LoadImmedUint).word(12);
    a.op(Op::TerminateRewindStr);
    a.op(Op::Return);
    assert_eq!(a.run(&mut Vm::new()).unwrap(), "ab12");
}

#[test]
fn a_docblock_runs_nothing() {
    let mut a = Asm::default();
    a.op(Op::DocblockStr).text("@class Foo\nwhat Foo is");
    a.op(Op::LoadImmedUint).word(5);
    a.op(Op::DocblockStr).text("what the next function does");
    a.op(Op::ReturnUint);
    let mut vm = Vm::new();
    assert_eq!(a.run(&mut vm).unwrap(), "5");
    assert!(vm.drain_log().is_empty());
}

#[test]
fn setcurfield_type_tags_a_dynamic_field() {
    let mut vm = Vm::new();
    source(&mut vm, "new ScriptObject(O) { f = 1; };");
    let mut a = Asm::default();
    // O.f's type, then O.g[2]'s.
    a.op(Op::LoadImmedStr).text("O");
    a.op(Op::SetCurObject);
    a.op(Op::SetCurField).ident("f");
    a.op(Op::PopStk);
    a.op(Op::SetCurFieldType).word(7);
    a.op(Op::LoadImmedStr).text("2");
    a.op(Op::LoadImmedStr).text("O");
    a.op(Op::SetCurObject);
    a.op(Op::SetCurField).ident("g");
    a.op(Op::PopStk);
    a.op(Op::SetCurFieldArray);
    a.op(Op::PopStk);
    a.op(Op::SetCurFieldType).word(9);
    // Without an object nothing is tagged.
    a.op(Op::LoadImmedStr).text("NoSuchObject");
    a.op(Op::SetCurObject);
    a.op(Op::SetCurField).ident("f");
    a.op(Op::PopStk);
    a.op(Op::SetCurFieldType).word(3);
    a.op(Op::ReturnVoid);
    a.run(&mut vm).unwrap();
    let id = vm.objects().find(b"O").unwrap();
    let o = vm.objects().get(id).unwrap();
    assert_eq!(o.dynamic_field_type(b"F"), Some(7));
    assert_eq!(o.dynamic_field_type(b"g2"), Some(9));
    assert_eq!(o.dynamic_field_type(b"h"), None);
    // The tag changes nothing about the value.
    assert_eq!(vm.script_field(id, b"f", b""), b"1");
}

#[test]
fn jmpif_reads_its_value_as_a_float() {
    // "0.5" is 0 as an integer and true as a float: Torque3D's `OP_JMPIF` tests the float.
    let mut a = Asm::default();
    a.op(Op::LoadImmedStr).text("0.5");
    a.op(Op::JmpIf).word(7);
    a.op(Op::LoadImmedStr).text("fell through");
    a.op(Op::Return);
    a.op(Op::LoadImmedStr).text("jumped");
    a.op(Op::Return);
    assert_eq!(a.run(&mut Vm::new()).unwrap(), "jumped");
}

#[test]
fn the_debuggers_break_and_invalid_opcodes_stop_the_run() {
    let mut vm = Vm::new();
    let mut a = Asm::default();
    a.op(Op::Break);
    assert_eq!(a.run(&mut vm), Err(VmError::Unsupported("OP_BREAK")));
    let mut a = Asm::default();
    a.op(Op::Invalid);
    assert_eq!(a.run(&mut vm), Err(VmError::Unsupported("OP_INVALID")));
    let mut a = Asm::default();
    a.word(999);
    assert_eq!(
        a.run(&mut vm),
        Err(VmError::InvalidOpcode { ip: 0, word: 999 })
    );
    // Running off the end of the code.
    let mut a = Asm::default();
    a.op(Op::LoadImmedUint).word(1);
    assert_eq!(a.run(&mut vm), Err(VmError::CodeOutOfRange { ip: 2 }));
    // The Vm runs on.
    let mut a = Asm::default();
    a.op(Op::LoadImmedStr).text("fine").op(Op::Return);
    assert_eq!(a.run(&mut vm).unwrap(), "fine");
}

#[test]
fn broken_code_is_an_error_not_a_crash() {
    let mut vm = Vm::new();
    // A pop from the empty stack.
    let mut a = Asm::default();
    a.op(Op::PopStk);
    assert_eq!(a.run(&mut vm), Err(VmError::StackUnderflow));
    // A string offset past the table.
    let mut a = Asm::default();
    a.op(Op::LoadImmedStr).word(1000);
    assert_eq!(a.run(&mut vm), Err(VmError::BadIndex { ip: 1 }));
    // A float index past the table.
    let mut a = Asm::default();
    a.op(Op::LoadImmedFlt).word(0);
    assert_eq!(a.run(&mut vm), Err(VmError::BadIndex { ip: 1 }));
    // `OP_ITER_END` with no loop open, `OP_ITER` with none.
    let mut a = Asm::default();
    a.op(Op::IterEnd);
    assert_eq!(a.run(&mut vm), Err(VmError::StackUnderflow));
    let mut a = Asm::default();
    a.op(Op::Iter).word(0);
    assert_eq!(a.run(&mut vm), Err(VmError::StackUnderflow));
    // A call with no argument list open.
    let mut a = Asm::default();
    a.op(Op::CallFunc).ident("echo").word(0).word(0).word(0);
    assert_eq!(a.run(&mut vm), Err(VmError::StackUnderflow));
    // An unknown call type.
    let mut a = Asm::default();
    a.op(Op::PushFrame).word(0);
    a.op(Op::CallFunc).ident("echo").word(0).word(0).word(9);
    assert_eq!(a.run(&mut vm), Err(VmError::Unsupported("OP_CALLFUNC")));
    // `OP_ADD_OBJECT` and `OP_FINISH_OBJECT` with no declaration open do nothing.
    let mut a = Asm::default();
    a.op(Op::LoadImmedUint).word(0);
    a.op(Op::AddObject).word(1);
    a.op(Op::FinishObject);
    a.op(Op::Return);
    assert_eq!(a.run(&mut vm).unwrap(), "0");
}

#[test]
fn a_register_outside_a_functions_frame_is_an_error() {
    let mut vm = Vm::new();
    let mut a = Asm::default();
    // function f() with one register; its body reads register 5.
    a.op(Op::FuncDecl)
        .ident("f")
        .word(0)
        .word(0)
        .word(0)
        .word(0)
        .word(1)
        .word(0) // end, patched below
        .word(0)
        .word(1);
    let body = a.code.len();
    a.op(Op::LoadLocalVarStr).word(5);
    a.op(Op::Return);
    let end = u32::try_from(a.code.len()).unwrap();
    a.code[body - 3] = end;
    a.op(Op::ReturnVoid);
    a.run(&mut vm).unwrap();
    assert_eq!(
        vm.invoke(None, b"f", &[]),
        Err(VmError::BadIndex { ip: 12 })
    );
}

#[test]
fn a_top_level_grows_its_registers() {
    let mut a = Asm::default();
    a.op(Op::LoadImmedStr).text("kept");
    a.op(Op::SaveLocalVarStr).word(40);
    a.op(Op::PopStk);
    a.op(Op::LoadLocalVarStr).word(41);
    a.op(Op::LoadLocalVarStr).word(40);
    a.op(Op::TerminateRewindStr);
    a.op(Op::Return);
    assert_eq!(a.run(&mut Vm::new()).unwrap(), "kept");
}

/// A top level that puts `text` in register 0 and runs `body` on it.
fn on_register(text: &str, body: impl FnOnce(&mut Asm)) -> Asm {
    let mut a = Asm::default();
    a.op(Op::LoadImmedStr).text(text);
    a.op(Op::SaveLocalVarStr).word(0);
    a.op(Op::PopStk);
    body(&mut a);
    a
}

/// `<register 0>.<suffix>` read as text.
fn read_component(a: &mut Asm, suffix: &str) {
    a.op(Op::LoadLocalVarStr).word(0);
    a.op(Op::SetCurObject);
    a.op(Op::SetCurField).ident(suffix);
    a.op(Op::PopStk);
    a.op(Op::LoadFieldStr);
    a.op(Op::Return);
}

#[test]
fn a_component_read_of_a_huge_value_stays_within_its_buffer() {
    let big = format!("{0} {0} {0} {0}", "a".repeat(1 << 20));
    let a = on_register(&big, |a| read_component(a, "xyzw"));
    let got = a.run(&mut Vm::new()).unwrap();
    assert_eq!(got.len(), 127);
    // More than four letters is no component.
    let a = on_register("1 2 3 4", |a| read_component(a, "xyzwx"));
    assert_eq!(a.run(&mut Vm::new()).unwrap(), "");
    let a = on_register("1 2 3 4", |a| read_component(a, "wzyx"));
    assert_eq!(a.run(&mut Vm::new()).unwrap(), "4 3 2 1");
}

/// The steps a component write of `previous`'s second word takes.
fn component_write_steps(previous: &str) -> (u64, String) {
    let a = on_register(previous, |a| {
        a.op(Op::LoadImmedStr).text("b");
        a.op(Op::LoadLocalVarStr).word(0);
        a.op(Op::SetCurObject);
        a.op(Op::SetCurField).ident("y");
        a.op(Op::PopStk);
        a.op(Op::SaveFieldStr);
        a.op(Op::PopStk);
        a.op(Op::LoadLocalVarStr).word(0);
        a.op(Op::Return);
    });
    let mut vm = Vm::new();
    vm.set_step_limit(Some(u64::MAX));
    let got = a.run(&mut vm).unwrap();
    (vm.steps(), got)
}

#[test]
fn a_component_write_is_cut_to_its_buffer_and_charged() {
    let (small, got) = component_write_steps("a a");
    assert_eq!(got, "a b");
    // A long first word: the value is cut at 1023 bytes before the write, the result too, and the work counts.
    let (large, got) = component_write_steps(&format!("{} a", "a".repeat(1 << 16)));
    assert_eq!(got.len(), 1023);
    assert!(large > small + 1023 / 64, "{small} {large}");
}

/// A function `f` with `registers` registers whose body calls itself `depth` more times (the depth in register 0).
fn recursive_with_registers(registers: u32) -> Asm {
    let mut a = Asm::default();
    a.op(Op::FuncDecl)
        .ident("f")
        .word(0)
        .word(0)
        .word(0)
        .word(0)
        .word(1)
        .word(0)
        .word(1)
        .word(registers)
        .word(0) // the parameter's register
        .word(0) // its flags
        .word(0); // its default
    let end_slot = 1 + 2 + 2 + 2 + 1;
    // if (%n > 0) f(%n - 1);
    a.op(Op::LoadImmedFlt).word(0);
    a.op(Op::LoadLocalVarFlt).word(0);
    a.op(Op::CmpGr);
    let jump = a.code.len() + 1;
    a.op(Op::JmpIfNot).word(0);
    a.op(Op::PushFrame).word(1);
    a.op(Op::LoadImmedFlt).word(1);
    a.op(Op::LoadLocalVarFlt).word(0);
    a.op(Op::Sub);
    a.op(Op::Push);
    a.op(Op::CallFunc).ident("f").word(0).word(0).word(0);
    a.op(Op::PopStk);
    a.code[jump] = u32::try_from(a.code.len()).unwrap();
    a.op(Op::ReturnVoid);
    a.code[end_slot] = u32::try_from(a.code.len()).unwrap();
    a.op(Op::ReturnVoid);
    a
}

fn with_floats(a: &Asm, floats: &[f64]) -> CodeBlock {
    let mut dso = a.dso();
    dso.global_floats = floats.to_vec();
    dso.function_floats = floats.to_vec();
    CodeBlock::from_dso(&dso, Some(b"asm.cs"), torque3d_format()).unwrap()
}

#[test]
fn register_counts_are_bounded_and_charged() {
    let mut vm = Vm::new();
    // A declaration that asks for four billion registers is refused before anything is allocated.
    vm.run(&Rc::new(with_floats(
        &recursive_with_registers(u32::MAX),
        &[0.0, 1.0],
    )))
    .unwrap();
    assert_eq!(
        vm.invoke(None, b"f", &[b"0".to_vec()]),
        Err(VmError::BadIndex { ip: 10 })
    );
    // 4096 registers a frame: 100 nested frames would need far more than all frames may have together.
    let mut vm = Vm::new();
    vm.run(&Rc::new(with_floats(
        &recursive_with_registers(4096),
        &[0.0, 1.0],
    )))
    .unwrap();
    assert_eq!(
        vm.invoke(None, b"f", &[b"100".to_vec()]),
        Err(VmError::StackOverflow)
    );
    // A few frames run, and their registers are charged.
    vm.set_step_limit(Some(u64::MAX));
    vm.invoke(None, b"f", &[b"3".to_vec()]).unwrap();
    assert!(vm.steps() >= 4 * 4096 / 64, "{}", vm.steps());
    // A top level's register past the frame bound.
    let mut a = Asm::default();
    a.op(Op::LoadLocalVarStr).word(1 << 20);
    assert_eq!(a.run(&mut vm), Err(VmError::BadIndex { ip: 1 }));
}
