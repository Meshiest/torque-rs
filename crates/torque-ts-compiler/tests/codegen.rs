//! The code generator, for the built-in (Torque3D) dialect: the code each construct compiles to (pinned as
//! decoded instructions), the tables and the line-break and local-variable tables, round trips through the bytes,
//! the text form and the decoder, the messages, the bounds, and another build's dialect.

use crate::common;

use std::time::Instant;

use torque_ts_bytecode::decode::{decode_dso, verify};
use torque_ts_bytecode::dsotext::{parse as parse_text, render};
use torque_ts_bytecode::opcodes::{Op, OpcodeTable};
use torque_ts_bytecode::{Dialect, DsoFile, FileLayout};
use torque_ts_compiler::codegen::{MAX_CODE_WORDS, MAX_VISITS};
use torque_ts_compiler::{Level, Output, Purpose, compile};

fn t3d() -> Dialect {
    Dialect::torque3d()
}

fn run(src: &str) -> Output {
    compile(src.as_bytes(), Some(b"t.cs"), Purpose::Run, &t3d())
}

fn text(src: &str) -> String {
    let out = run(src);
    render(
        &out.dso.unwrap_or_else(|| {
            panic!("{src:?} has no code: {:?}", common::texts(&out.diagnostics))
        }),
        &t3d(),
    )
}

/// One section of a DSO's text form: its title line and its indented lines.
fn section(text: &str, title: &str) -> String {
    let mut out = String::new();
    let mut on = false;
    for line in text.lines() {
        if line.starts_with(&format!("{title} ")) {
            on = true;
        } else if on && !line.starts_with("  ") {
            break;
        }
        if on {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The instructions of a program's code as the decoder prints them, without addresses: `OP_NAME operands; ...`.
fn listing(src: &str) -> String {
    let dso = run(src)
        .dso
        .unwrap_or_else(|| panic!("{src:?} has no code"));
    let ins = decode_dso(&dso, &t3d()).unwrap();
    torque_ts_bytecode::decode::render(&dso, &ins)
        .lines()
        .map(|l| l.split_once(": ").map_or(l, |(_, ins)| ins))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The opcodes of a program's code, the final `OP_RETURN_VOID` included.
fn ops(src: &str) -> Vec<Op> {
    decode_dso(&run(src).dso.unwrap(), &t3d())
        .unwrap()
        .iter()
        .map(|i| i.op)
        .collect()
}

/// Compiles, then checks the DSO against itself: its bytes read back to it, its text form reads back to it,
/// its code decodes and verifies and ends in `OP_RETURN_VOID`.
fn round_trip(src: &str, dialect: &Dialect) -> DsoFile {
    let out = compile(src.as_bytes(), Some(b"t.cs"), Purpose::Run, dialect);
    assert!(
        !out.syntax_error,
        "{src}: {:?}",
        common::texts(&out.diagnostics)
    );
    let dso = out.dso.unwrap_or_else(|| panic!("{src}: no code"));
    let bytes = dso.to_bytes(dialect);
    assert_eq!(DsoFile::parse(&bytes, dialect).unwrap(), dso, "{src}");
    let text = render(&dso, dialect);
    assert_eq!(parse_text(&text).unwrap().to_bytes(dialect), bytes);
    let ins = decode_dso(&dso, dialect).unwrap_or_else(|e| panic!("{src}: {e}\n{text}"));
    let problems = verify(&dso, &ins);
    assert!(problems.is_empty(), "{src}: {problems:?}\n{text}");
    assert_eq!(ins.last().map(|i| i.op), Some(Op::ReturnVoid), "{src}");
    dso
}

const PROGRAMS: &[&str] = &[
    "$x = 1;",
    "function f(%a, %b) { %x = %a + %b; %x++; return %x; }",
    "function Ns::m(%this) { %this.v[1, 2] += 3; return %this.v @ \"s\" SPC 'tag'; }",
    "package P { function f() { do { $i--; } while ($i > 0) } function Ns::g() {} };",
    "for (;;) { if ($a) break; else continue; }",
    "for (%i = 0; ; %i++) { if (%i > 3) break; }",
    "for (; $i < 3;) $i++;",
    "switch$ ($s) { case \"a\" or \"b\": echo(1); default: echo(2); }",
    "switch ($n) { case 1: case2(); case 2 or 3: $x = $n ? -1.5 : !$n; }",
    "new ScriptObject(Obj : Parent, 1, \"a\") { f = 1; f[2] = 3; new SimSet(Child); };",
    "datablock ItemData(Thing : Base) { shape = \"x\"; };",
    "$o = new (%cls)(); $o.delete(); Parent::onAdd(%this); Foo::bar(1 $= 2, 3 !$= 4);",
    "$x = $a[1] = ~$b | $c ^ ($d & 1) << 2 >> 1 % 3 - 4 / 5 * 6 && 7 || 8;",
    "/* a block\n comment */ $y = 0x10 + 1.5e3 + 0.5 + .25; // the rest",
    "function f(%a, %b = 1, %c ? = %a @ \"x\") { foreach (%o in %a) { if (%o) break; continue; } return %c; }",
    "foreach$ ($w in \"a b\") { assert($w !$= \"\", \"empty word\"); assert(1); }",
    "singleton Material(M) { a = 1; }; new ScriptObject([inner]) { new SimSet([kid]); };",
    "$x = $o->child.f + $o-->(%n);",
    "datablock (%cls)(\"n\" @ 1 : Base) {};",
];

#[test]
fn compiled_files_round_trip_through_bytes_text_and_the_decoder() {
    for src in PROGRAMS {
        let dso = round_trip(src, &t3d());
        assert_eq!(dso.version, 50);
        // The same program compiles to the same bytes every time.
        assert_eq!(run(src).dso, Some(dso));
    }
}

#[test]
fn expressions_compile_to_the_code_the_nodes_write() {
    for (src, expected) in [
        // Arithmetic: the right operand first, as floats; no conversion follows.
        (
            "$a = $b - $c * 3;",
            r#"OP_LOADIMMED_FLT 3.0; OP_SETCURVAR "$c"; OP_LOADVAR_FLT; OP_MUL; OP_SETCURVAR "$b"; OP_LOADVAR_FLT; OP_SUB; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_FLT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // Two number constants fold into one (`1 + 2 * 3` has one, `2 * 3`).
        (
            "$a = 1 + 2 * 3; $b = 1 / 0;",
            r#"OP_LOADIMMED_FLT 6.0; OP_LOADIMMED_FLT 1.0; OP_ADD; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_FLT; OP_POP_STK; OP_LOADIMMED_FLT 0.0; OP_SETCURVAR_CREATE "$b"; OP_SAVEVAR_FLT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // Comparisons are float operations with an integer result; `&&` and `||` jump past their right side.
        (
            "$a = $b < 2 && $c;",
            r#"OP_LOADIMMED_FLT 2.0; OP_SETCURVAR "$b"; OP_LOADVAR_FLT; OP_CMPLT; OP_JMPIFNOT_NP ->000013; OP_SETCURVAR "$c"; OP_LOADVAR_UINT; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_UINT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // A variable's remainder by 2, 4 or 8 is a bitwise and.
        (
            "$a = $b % 8;",
            r#"OP_LOADIMMED_UINT 7; OP_SETCURVAR "$b"; OP_LOADVAR_UINT; OP_BITAND; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_UINT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        (
            "$a = 9 % 8;",
            r#"OP_LOADIMMED_UINT 8; OP_LOADIMMED_UINT 9; OP_MOD; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_UINT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // Concatenation: the left side first; `SPC`, `TAB` and `NL` append their character.
        (
            r#"$a = "x" @ $b SPC "y";"#,
            r#"OP_LOADIMMED_STR "x"; OP_SETCURVAR "$b"; OP_LOADVAR_STR; OP_REWIND_STR; OP_ADVANCE_STR_APPENDCHAR ' '; OP_LOADIMMED_STR "y"; OP_REWIND_STR; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_STR; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // String comparison: the left side first; `!$=` adds a `!`.
        (
            r#"$a = $b !$= "y";"#,
            r#"OP_SETCURVAR "$b"; OP_LOADVAR_STR; OP_LOADIMMED_STR "y"; OP_COMPARE_STR; OP_NOT; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_UINT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // `!` of a float or a string is `OP_NOTF`; `-x` is a float negation.
        (
            "$a = !$b; $c = !1.5; $d = -$e;",
            r#"OP_SETCURVAR "$b"; OP_LOADVAR_UINT; OP_NOT; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_UINT; OP_POP_STK; OP_LOADIMMED_FLT 1.5; OP_NOTF; OP_SETCURVAR_CREATE "$c"; OP_SAVEVAR_UINT; OP_POP_STK; OP_SETCURVAR "$e"; OP_LOADVAR_FLT; OP_NEG; OP_SETCURVAR_CREATE "$d"; OP_SAVEVAR_FLT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // `?:` of two strings is a string; its test is a float unless it is an integer.
        (
            r#"$a = $b ? "y" : "n";"#,
            r#"OP_SETCURVAR "$b"; OP_LOADVAR_FLT; OP_JMPIFFNOT ->000010; OP_LOADIMMED_STR "y"; OP_JMP ->000012; OP_LOADIMMED_STR "n"; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_STR; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // An indexed variable: its name, the index (a comma list joined with `_`), `OP_SETCURVAR_ARRAY`.
        (
            "$a = $b[1, 2];",
            r#"OP_LOADIMMED_IDENT "$b"; OP_LOADIMMED_STR "1"; OP_ADVANCE_STR_APPENDCHAR '_'; OP_LOADIMMED_STR "2"; OP_REWIND_STR; OP_REWIND_STR; OP_SETCURVAR_ARRAY; OP_POP_STK; OP_LOADVAR_STR; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_STR; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // Strings and names used as numbers.
        (
            r#"$a = "2" * true;"#,
            r#"OP_LOADIMMED_FLT 1.0; OP_LOADIMMED_FLT 2.0; OP_MUL; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_FLT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
    ] {
        assert_eq!(listing(src), expected, "{src}");
    }
}

#[test]
fn assignments_fields_and_calls_compile_to_the_code_the_nodes_write() {
    for (src, expected) in [
        // A compound assignment of a global: the value, the variable, load, operate, save.
        (
            "$a += 2;",
            r#"OP_LOADIMMED_FLT 2.0; OP_SETCURVAR_CREATE "$a"; OP_LOADVAR_FLT; OP_ADD; OP_SAVEVAR_FLT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        (
            "$a[1] |= 2;",
            r#"OP_LOADIMMED_UINT 2; OP_LOADIMMED_IDENT "$a"; OP_LOADIMMED_STR "1"; OP_REWIND_STR; OP_SETCURVAR_ARRAY_CREATE; OP_POP_STK; OP_LOADVAR_UINT; OP_BITOR; OP_SAVEVAR_UINT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // A field: the index, the object, the field; each object and index is popped once used.
        (
            "$a = %o.f[1];",
            r#"OP_LOADIMMED_STR "1"; OP_LOAD_LOCAL_VAR_STR r0; OP_SETCUROBJECT; OP_SETCURFIELD "f"; OP_POP_STK; OP_SETCURFIELD_ARRAY; OP_POP_STK; OP_LOADFIELD_STR; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_STR; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        (
            "%o.f = 1;",
            r#"OP_LOADIMMED_STR "1"; OP_LOAD_LOCAL_VAR_STR r0; OP_SETCUROBJECT; OP_SETCURFIELD "f"; OP_POP_STK; OP_SAVEFIELD_STR; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        (
            "%o.f *= 2;",
            r#"OP_LOADIMMED_FLT 2.0; OP_LOAD_LOCAL_VAR_STR r0; OP_SETCUROBJECT; OP_SETCURFIELD "f"; OP_POP_STK; OP_LOADFIELD_FLT; OP_MUL; OP_SAVEFIELD_FLT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // Calls: a frame, each argument as its own type, then the name, the namespace and the kind of call.
        (
            r#"foo(1, "a", 2.5);"#,
            r#"OP_PUSH_FRAME 3; OP_LOADIMMED_UINT 1; OP_PUSH; OP_LOADIMMED_STR "a"; OP_PUSH; OP_LOADIMMED_FLT 2.5; OP_PUSH; OP_CALLFUNC "foo" - 0; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        (
            "Ns::f(); %o.m($x); Parent::f(); PARENT::f(); Parents::f();",
            r#"OP_PUSH_FRAME 0; OP_CALLFUNC "f" "Ns" 1; OP_POP_STK; OP_PUSH_FRAME 2; OP_LOAD_LOCAL_VAR_STR r0; OP_PUSH; OP_SETCURVAR "$x"; OP_LOADVAR_STR; OP_PUSH; OP_CALLFUNC "m" - 2; OP_POP_STK; OP_PUSH_FRAME 0; OP_CALLFUNC "f" "Parent" 3; OP_POP_STK; OP_PUSH_FRAME 0; OP_CALLFUNC "f" "Parent" 3; OP_POP_STK; OP_PUSH_FRAME 0; OP_CALLFUNC "f" "Parents" 1; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // A call's value used: no pop.
        (
            "$a = foo();",
            r#"OP_PUSH_FRAME 0; OP_CALLFUNC "foo" - 0; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_STR; OP_POP_STK; OP_RETURN_VOID"#,
        ),
    ] {
        assert_eq!(listing(src), expected, "{src}");
    }
}

#[test]
fn statements_compile_to_the_code_the_nodes_write() {
    for (src, expected) in [
        // `if` of a string tests it as a string; of an integer with `OP_JMPIFNOT`; `else` jumps over.
        (
            r#"if ($a) $b = 1; else $b = 2;"#,
            r#"OP_SETCURVAR "$a"; OP_LOADVAR_STR; OP_JMPNOTSTRING ->000015; OP_LOADIMMED_UINT 1; OP_SETCURVAR_CREATE "$b"; OP_SAVEVAR_UINT; OP_POP_STK; OP_JMP ->000022; OP_LOADIMMED_UINT 2; OP_SETCURVAR_CREATE "$b"; OP_SAVEVAR_UINT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        (
            "if (!$a) foo();",
            r#"OP_SETCURVAR "$a"; OP_LOADVAR_UINT; OP_NOT; OP_JMPIFNOT ->000016; OP_PUSH_FRAME 0; OP_CALLFUNC "foo" - 0; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // `while`: test, body, test again; `break` and `continue` jump to the end and to the second test.
        (
            "while ($a) { if ($b) continue; break; }",
            r#"OP_SETCURVAR "$a"; OP_LOADVAR_FLT; OP_JMPIFFNOT ->000022; OP_SETCURVAR "$b"; OP_LOADVAR_STR; OP_JMPNOTSTRING ->000014; OP_JMP ->000016; OP_JMP ->000022; OP_SETCURVAR "$a"; OP_LOADVAR_FLT; OP_JMPIFF ->000006; OP_RETURN_VOID"#,
        ),
        // `do`-`while`: the body first.
        (
            "do foo(); while (1)",
            r#"OP_PUSH_FRAME 0; OP_CALLFUNC "foo" - 0; OP_POP_STK; OP_LOADIMMED_UINT 1; OP_JMPIF ->000000; OP_RETURN_VOID"#,
        ),
        // `for` without parts: the test is the constant 1.
        (
            "for (;;) break;",
            r#"OP_LOADIMMED_UINT 1; OP_JMPIFNOT ->000010; OP_JMP ->000010; OP_LOADIMMED_UINT 1; OP_JMPIF ->000004; OP_RETURN_VOID"#,
        ),
        (
            "for ($i = 0; $i < 2; $i++) {}",
            r#"OP_LOADIMMED_UINT 0; OP_SETCURVAR_CREATE "$i"; OP_SAVEVAR_UINT; OP_POP_STK; OP_LOADIMMED_FLT 2.0; OP_SETCURVAR "$i"; OP_LOADVAR_FLT; OP_CMPLT; OP_JMPIFNOT ->000034; OP_LOADIMMED_FLT 1.0; OP_SETCURVAR_CREATE "$i"; OP_LOADVAR_FLT; OP_ADD; OP_SAVEVAR_FLT; OP_POP_STK; OP_LOADIMMED_FLT 2.0; OP_SETCURVAR "$i"; OP_LOADVAR_FLT; OP_CMPLT; OP_JMPIF ->000016; OP_RETURN_VOID"#,
        ),
        // A `switch` is a chain of `if`s; a case of several values tests each one.
        (
            "switch ($a) { case 1 or 2: foo(); default: bar(); }",
            r#"OP_LOADIMMED_FLT 1.0; OP_SETCURVAR "$a"; OP_LOADVAR_FLT; OP_CMPEQ; OP_JMPIF_NP ->000016; OP_LOADIMMED_FLT 2.0; OP_SETCURVAR "$a"; OP_LOADVAR_FLT; OP_CMPEQ; OP_JMPIFNOT ->000029; OP_PUSH_FRAME 0; OP_CALLFUNC "foo" - 0; OP_POP_STK; OP_JMP ->000038; OP_PUSH_FRAME 0; OP_CALLFUNC "bar" - 0; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        (
            r#"switch$ ($a) { case "x": foo(); }"#,
            r#"OP_SETCURVAR "$a"; OP_LOADVAR_STR; OP_LOADIMMED_STR "x"; OP_COMPARE_STR; OP_JMPIFNOT ->000018; OP_PUSH_FRAME 0; OP_CALLFUNC "foo" - 0; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // `return` of each type.
        (
            r#"function f() { return; } function g() { return 1; } function h() { return 1.5; } function i() { return "s"; }"#,
            r#"OP_FUNC_DECL "f" - - body=1 line=1 ->000013 n=0 0; OP_RETURN_VOID; OP_RETURN_VOID; OP_FUNC_DECL "g" - - body=1 line=1 ->000028 n=0 0; OP_LOADIMMED_UINT 1; OP_RETURN_UINT; OP_RETURN_VOID; OP_FUNC_DECL "h" - - body=1 line=1 ->000043 n=0 0; OP_LOADIMMED_FLT 1.5; OP_RETURN_FLT; OP_RETURN_VOID; OP_FUNC_DECL "i" - - body=1 line=1 ->000058 n=0 0; OP_LOADIMMED_STR "s"; OP_RETURN; OP_RETURN_VOID; OP_RETURN_VOID"#,
        ),
    ] {
        assert_eq!(listing(src), expected, "{src}");
    }
}

#[test]
fn functions_keep_their_locals_in_registers() {
    // Parameters first, then each local as it is first met (`%z`, read for `%y`'s type, before `%y`). `%x++`
    // of a local whose value is not used is `OP_INC`; a local that is only read is a string register (with a
    // warning).
    let src = "function Ns::f(%a, %b)\n{\n   %x = %a * 2;\n   %x++;\n   %y = %z;\n   return %x;\n}";
    assert_eq!(
        listing(src),
        r#"OP_FUNC_DECL "f" "Ns" - body=1 line=7 ->000036 n=2 5 r0 r1 0 0 ->000000 ->000000; OP_LOADIMMED_FLT 2.0; OP_LOAD_LOCAL_VAR_FLT r0; OP_MUL; OP_SAVE_LOCAL_VAR_FLT r2; OP_POP_STK; OP_INC r2; OP_LOAD_LOCAL_VAR_STR r3; OP_SAVE_LOCAL_VAR_STR r4; OP_POP_STK; OP_LOAD_LOCAL_VAR_FLT r2; OP_RETURN_FLT; OP_RETURN_VOID; OP_RETURN_VOID"#
    );
    let out = run(src);
    assert_eq!(
        common::texts(&out.diagnostics),
        [
            "Script Warning: Variable %z referenced before used when compiling script. File: t.cs\nLine Num: 5\nLine: \"   %y = %z;\""
        ]
    );
    assert_eq!(
        section(&text(src), "local-variables"),
        "local-variables 1\n  \"Ns::f\" \"%a\" \"%b\" \"%x\" \"%z\" \"%y\"\n"
    );
    // A variable assigned values of two types has none: it is read as a string.
    assert!(
        listing("function f() { %v = 1; %v = \"s\"; return %v; }")
            .contains("OP_LOAD_LOCAL_VAR_STR r0; OP_RETURN;")
    );
    // Functions of the same name share one entry; names are matched ignoring case.
    let t = text("function f(%a) {} function g() { %q = 1; } package P { function F(%b) {} };");
    assert_eq!(
        section(&t, "local-variables"),
        "local-variables 2\n  \"(null)::f\" \"%a\" \"%b\"\n  \"(null)::g\" \"%q\"\n"
    );
    // At the top level, locals use the file's own registers, with a warning in a file (not in `eval`'d text).
    let out = run("%t = 1;");
    assert_eq!(
        common::texts(&out.diagnostics),
        [
            "Attemping to use local variable in global scope. File: t.cs\nLine Num: 1\nLine: \"%t = 1;\""
        ]
    );
    assert!(
        compile(b"%t = 1;", None, Purpose::Run, &t3d())
            .diagnostics
            .is_empty()
    );
    // `%x[1]` and `$x` are not registers.
    assert_eq!(
        ops("function f() { %x[1] = 1; }"),
        [
            Op::FuncDecl,
            Op::LoadImmedUint,
            Op::LoadImmedIdent,
            Op::LoadImmedStr,
            Op::RewindStr,
            Op::SetCurVarArrayCreate,
            Op::PopStk,
            Op::SaveVarUint,
            Op::PopStk,
            Op::ReturnVoid,
            Op::ReturnVoid
        ]
    );
}

#[test]
fn declarations_compile_to_the_code_the_nodes_write() {
    for (src, expected) in [
        // An object: a frame of class, name and arguments, the declaration (parent, datablock, internal name,
        // singleton, line, where to go when it fails), its fields, its children.
        (
            r#"new ScriptObject(A : B, 1) { f = 2; new SimSet(C); };"#,
            r#"OP_LOADIMMED_UINT 0; OP_PUSH_FRAME 3; OP_LOADIMMED_IDENT "ScriptObject"; OP_PUSH; OP_LOADIMMED_IDENT "A"; OP_PUSH; OP_LOADIMMED_UINT 1; OP_PUSH; OP_CREATE_OBJECT "B" 0 0 0 1 ->000058; OP_LOADIMMED_STR "2"; OP_SETCUROBJECT_NEW; OP_SETCURFIELD "f"; OP_SAVEFIELD_STR; OP_POP_STK; OP_ADD_OBJECT 1; OP_PUSH_FRAME 2; OP_LOADIMMED_IDENT "SimSet"; OP_PUSH; OP_LOADIMMED_IDENT "C"; OP_PUSH; OP_CREATE_OBJECT - 0 0 0 1 ->000055; OP_ADD_OBJECT 0; OP_END_OBJECT 0; OP_FINISH_OBJECT; OP_END_OBJECT 1; OP_FINISH_OBJECT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        (
            r#"datablock D(N) { a = 1; };"#,
            r#"OP_LOADIMMED_UINT 0; OP_PUSH_FRAME 2; OP_LOADIMMED_IDENT "D"; OP_PUSH; OP_LOADIMMED_IDENT "N"; OP_PUSH; OP_CREATE_OBJECT - 1 0 0 1 ->000032; OP_LOADIMMED_STR "1"; OP_SETCUROBJECT_NEW; OP_SETCURFIELD "a"; OP_SAVEFIELD_STR; OP_POP_STK; OP_ADD_OBJECT 1; OP_END_OBJECT 1; OP_FINISH_OBJECT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // A package's function names the package; an empty body is no body.
        (
            "package P { function f() {} };",
            r#"OP_FUNC_DECL "f" - "P" body=0 line=1 ->000012 n=0 0; OP_RETURN_VOID; OP_RETURN_VOID"#,
        ),
    ] {
        assert_eq!(listing(src), expected, "{src}");
    }
}

#[test]
fn every_statement_records_its_line_and_first_code_word() {
    // Top level and in functions; a loop its own line, a `switch` its cases' lines, a function its line again
    // at its closing `OP_RETURN_VOID`; `break` outside a loop records nothing.
    let src = "$a = 1;\nfunction f(%a) {\n  while (%a)\n     break;\n  switch (%a) {\n    case 1:\n      %y = 2;\n  }\n}\nbreak;\nreturn;\n";
    assert_eq!(
        section(&text(src), "line-breaks"),
        "line-breaks 7\n  1 0\n  3 21\n  4 25\n  6 31\n  7 38\n  9 43\n  11 44\n"
    );
}

#[test]
fn names_and_constants_enter_the_tables_in_compile_order() {
    let names = |src: &str, title: &str| -> Vec<String> {
        section(&text(src), title)
            .lines()
            .skip(1)
            .filter_map(|l| l.split_once('"'))
            .map(|(_, rest)| rest.trim_end_matches('"').to_string())
            .collect()
    };
    for (src, title, expected) in [
        // A function's parameters, then its name, namespace and package, then the body's names.
        (
            "package MyPk { function MyNs::myname(%p) { $v = 1; } };",
            "global-strings",
            &["%p", "myname", "MyNs", "MyPk", "$v"][..],
        ),
        // A binary operation: the right operand first; `$=` and `@`: the left one.
        (
            r#"function f() { return ("a" + "b") @ ("c" $= "d"); }"#,
            "function-strings",
            &["c", "d"][..],
        ),
        (
            r#"function f() { return "a" @ "b"; }"#,
            "function-strings",
            &["a", "b"][..],
        ),
        // A field assignment: the field, the value, the index, the object.
        (
            r#"$zo.zf["zis"] = "zva";"#,
            "global-strings",
            &["zf", "zva", "zis", "$zo"][..],
        ),
        // A call: the name, the namespace, then the arguments.
        (
            r#"ZNs::zf("za", $zb);"#,
            "global-strings",
            &["zf", "ZNs", "za", "$zb"][..],
        ),
        // An object: the class, the name, the arguments, the parent, the fields.
        (
            r#"new ZCls(ZNm : ZPar, "za1") { zfa = "zva"; };"#,
            "global-strings",
            &["ZCls", "ZNm", "za1", "ZPar", "zfa", "zva"][..],
        ),
        // `?:`: the test, the true branch, the false branch.
        (
            r#"foo($zt ? "zta" : "ztb");"#,
            "global-strings",
            &["foo", "$zt", "zta", "ztb"][..],
        ),
    ] {
        assert_eq!(names(src, title), expected, "{src}");
    }
    // The numbers of `"a" + "b"` went to the float table: 0 (with a warning each).
    let t = text(r#"function f() { return ("a" + "b") @ 1; }"#);
    assert_eq!(
        section(&t, "function-floats"),
        "function-floats 1\n  0 0000000000000000 0.0\n"
    );
}

#[test]
fn numbers_used_as_text_go_into_the_table_of_the_code_that_uses_them() {
    // The same constant `1` as a string, at the top level and in a function: one entry in each table.
    let t = text("$a = 1 @ \"\";\nfunction f() { return 1 @ \"\"; }");
    assert!(section(&t, "global-strings").contains("\"1\""));
    assert!(section(&t, "function-strings").contains("\"1\""));
    // Once per table however often it is used.
    let t = text("$a = 1 @ \"\"; $b = 1 @ \"\";\nfunction f() { return 1 @ \"\" @ 1; }");
    assert_eq!(section(&t, "global-strings").matches("\"1\"").count(), 1);
    assert_eq!(section(&t, "function-strings").matches("\"1\"").count(), 1);
    // Floats as text use `%g`; a float constant is a single-precision number.
    let t = text("$a = 0.1 @ 1e20;");
    assert!(section(&t, "global-strings").contains("\"1e+20\""), "{t}");
    assert!(section(&t, "global-strings").contains("\"0.1\""), "{t}");
    let t = text("$a = 0.1 + $b;");
    assert_eq!(
        section(&t, "global-floats"),
        format!(
            "global-floats 1\n  0 {:016x} {:?}\n",
            f64::from(0.1f32).to_bits(),
            f64::from(0.1f32)
        )
    );
}

#[test]
fn constructs_that_compile_to_nothing_or_to_the_same_code() {
    // `else {}` and an empty `default:` add no `else`.
    assert_eq!(text("if ($a) $b = 1; else {}"), text("if ($a) $b = 1;"));
    assert_eq!(
        text("switch ($a) { case 1: $b = 1; default: }"),
        text("switch ($a) { case 1: $b = 1; }")
    );
    // `%o.f = {1, 2}` keeps only the list's first expression.
    assert_eq!(text("%o.f = {1, 2, \"x\"};"), text("%o.f = 1;"));
    // `*x` (the tag dereference) compiles to nothing, and its operand is not even compiled.
    assert_eq!(
        ops("foo(*$x);"),
        [
            Op::PushFrame,
            Op::Push,
            Op::CallFunc,
            Op::PopStk,
            Op::ReturnVoid
        ]
    );
    assert!(!text("foo(*$x);").contains("$x"));
    // A `for` without parts and a `while (1)` are the same loop.
    assert_eq!(listing("for (;;) foo();"), listing("while (1) foo();"));
    // `break` as a value is the identifier `break`.
    assert_eq!(
        ops("$x = break;"),
        [
            Op::LoadImmedIdent,
            Op::SetCurVarCreate,
            Op::SaveVarStr,
            Op::PopStk,
            Op::ReturnVoid
        ]
    );
    // Comments compile to nothing.
    assert_eq!(text("/* a */ $x = 1; /* b\n*/"), text("$x = 1;"));
}

#[test]
fn the_statements_before_a_syntax_error_compile_as_alone() {
    // The default reductions make `if (1) $x = 1;` a statement before the bad `)` is found.
    for (bad, good) in [
        ("$a = 1; if (1) $x = 1; )", "$a = 1; if (1) $x = 1;"),
        (
            "$a = 1; function f() { return 1; } )",
            "$a = 1; function f() { return 1; }",
        ),
        ("$a = 1; $b = 2 )", "$a = 1;"),
        ("$a = 1; /* never closed", "$a = 1;"),
    ] {
        let out = run(bad);
        assert!(out.syntax_error, "{bad}");
        assert_eq!(render(&out.dso.unwrap(), &t3d()), text(good), "{bad}");
        assert!(
            compile(bad.as_bytes(), Some(b"t.cs"), Purpose::Check, &t3d())
                .dso
                .is_none()
        );
    }
    let out = run("$a = 1; /* never closed");
    assert_eq!(
        common::texts(&out.diagnostics).last().map(String::as_str),
        Some("unexpected end of file inside block comment")
    );
}

#[test]
fn the_compilers_messages() {
    let out = run("break;\ncontinue;\nwhile (1) break;");
    assert_eq!(
        common::texts(&out.diagnostics),
        [
            "t.cs (1): break outside of loop... ignoring.",
            "t.cs (2): continue outside of loop... ignoring.",
        ]
    );
    assert!(out.diagnostics.iter().all(|d| d.level == Level::Warning));
    // `eval`'d text has no file: `printf("%s", NULL)`.
    let out = compile(b"break;", None, Purpose::Run, &t3d());
    assert_eq!(
        common::texts(&out.diagnostics),
        ["(null) (1): break outside of loop... ignoring."]
    );
    // Strings used as numbers.
    let out = run("$x = \"abc\" + 1;\n$y = \"1.5\" + 1;\n$z = Foo * 2;\n$w = \"true\" + 0;");
    assert_eq!(
        common::texts(&out.diagnostics),
        [
            "t.cs (1): string always evaluates to 0.",
            "t.cs (3): string always evaluates to 0.",
        ]
    );
    let out = compile(b"$x = \"abc\" + 1;", None, Purpose::Run, &t3d());
    assert!(out.diagnostics.is_empty(), "no file, no warning");
    // Nothing to compile: no code.
    for src in ["", "// only a comment\n", "  \n\t", "/* only a comment */"] {
        let out = run(src);
        assert!(out.dso.is_none() && !out.syntax_error && out.diagnostics.is_empty());
    }
}

#[test]
fn random_programs_compile_to_consistent_code() {
    let mut rng = common::Rng::new(0xc0de);
    let (mut programs, mut words) = (0, 0);
    for i in 0..2000 {
        let src = common::program::random_program(&mut rng, 1 + i % 5, 4);
        let out = run(&src);
        assert!(
            !out.syntax_error,
            "{src}: {:?}",
            common::texts(&out.diagnostics)
        );
        if out.dso.is_none() {
            continue;
        }
        let dso = round_trip(&src, &t3d());
        assert_eq!(run(&src).dso, Some(dso.clone()));
        programs += 1;
        words += dso.code.len();
    }
    eprintln!("{programs} random programs ({words} code words) verify");
    assert!(programs > 1500);
}

/// `switch (foo(x, x, ...)) { case 1 or 1 or ...: }`: the test expression is compiled once per case value.
fn wide_switch(arg: &str, args: usize, values: usize) -> Vec<u8> {
    let mut src = b"switch (foo(".to_vec();
    src.extend(format!("{arg},").repeat(args).bytes());
    src.extend_from_slice(arg.as_bytes());
    src.extend_from_slice(b")) { case 1");
    src.extend(b" or 1".repeat(values));
    src.extend_from_slice(b": $x = 1; }");
    src
}

#[test]
fn the_generators_bounds_give_clean_errors() {
    assert_eq!((MAX_VISITS, MAX_CODE_WORDS), (1 << 26, 1 << 24));
    let too_large = "the script is too large to compile";
    // `*$x` costs a visit and a code word (its `OP_PUSH`) and nothing else, so the bounds are reached fast.
    // MAX_CODE_WORDS: 1000 values over a 20001-argument call make 20 million words.
    let start = Instant::now();
    let out = compile(
        &wide_switch("*$x", 20_000, 1000),
        Some(b"t.cs"),
        Purpose::Run,
        &t3d(),
    );
    let words_secs = start.elapsed().as_secs_f64();
    assert!(out.syntax_error && out.dso.is_none());
    assert_eq!(common::texts(&out.diagnostics), [too_large]);
    assert!(out.work < MAX_VISITS);
    // MAX_VISITS: 4000 values over it would be 80 million visits.
    let start = Instant::now();
    let out = compile(
        &wide_switch("*$x", 20_000, 4000),
        Some(b"t.cs"),
        Purpose::Run,
        &t3d(),
    );
    let visits_secs = start.elapsed().as_secs_f64();
    assert!(out.syntax_error && out.dso.is_none());
    assert_eq!(common::texts(&out.diagnostics), [too_large]);
    assert!(out.work > MAX_VISITS);
    eprintln!("code-word bound reached in {words_secs:.2} s, visit bound in {visits_secs:.2} s");
    assert!(words_secs < 30.0 && visits_secs < 60.0);
    // Below both, the same shape compiles.
    let out = compile(
        &wide_switch("1", 100, 100),
        Some(b"t.cs"),
        Purpose::Run,
        &t3d(),
    );
    assert!(!out.syntax_error && out.dso.is_some());
}

#[test]
fn work_is_the_parse_plus_one_visit_per_node() {
    // `$x = 1;`: the statement, the assignment and the constant.
    let parsed = torque_ts_compiler::parser::parse(b"$x = 1;", Some(b"t.cs"));
    assert_eq!(run("$x = 1;").work, parsed.work + 3);
    // Nothing to generate: just the parse.
    assert_eq!(
        run("// nothing\n").work,
        torque_ts_compiler::parser::parse(b"// nothing\n", Some(b"t.cs")).work
    );
}

/// `compile` on a thread with a 1 MiB stack, as a script's `eval` at a deep call has: the compiler runs on its
/// own worker, so the caller's stack does not matter.
fn compile_on_one_mebibyte(src: Vec<u8>) -> Output {
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(move || compile(&src, Some(b"t.cs"), Purpose::Run, &Dialect::torque3d()))
        .unwrap()
        .join()
        .unwrap()
}

/// The deepest chains the parser accepts, one per way a node nests on its left.
fn deep_chains(n: usize) -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("+", format!("$x = $y{};", " + 1".repeat(n))),
        ("@", format!("$x = 1{};", " @ \"a\"".repeat(n))),
        ("$=", format!("$x = 1{};", " $= 1".repeat(n))),
        ("||", format!("$x = 1{};", " || 1".repeat(n))),
        ("?:", format!("$x = 1{};", " ? 1 : 1".repeat(n))),
        (".f", format!("$x = $a{};", ".f".repeat(n))),
        (".f[1]", format!("$x = $a{};", ".f[1]".repeat(n))),
        (".m()", format!("$x = $a{};", ".m()".repeat(n))),
        (".m(1)", format!("$x = $a{};", ".m(1)".repeat(n))),
    ]
    .into_iter()
    .map(|(name, src)| (name, src.into_bytes()))
    .collect()
}

#[test]
fn the_deepest_trees_compile_from_a_one_mebibyte_thread() {
    for (name, src) in deep_chains(4000) {
        let out = compile_on_one_mebibyte(src);
        assert!(
            !out.syntax_error && out.dso.is_some(),
            "{name}: {:?}",
            common::texts(&out.diagnostics)
        );
    }
    // One level too many is a clean error.
    for (name, src) in deep_chains(5000) {
        let out = compile_on_one_mebibyte(src);
        assert!(out.syntax_error && out.dso.is_none(), "{name}");
    }
}

#[test]
fn the_generator_needs_no_more_than_half_of_the_workers_stack() {
    // `compile_here` on a thread with half of WORKER_STACK: the deepest generator paths (a debug build's frames
    // are the largest) still fit with a factor of two to spare for the build and the platform.
    let half = torque_ts_compiler::WORKER_STACK / 2;
    let mut deepest: Vec<(&str, Vec<u8>)> = deep_chains(4090);
    let mut switch = b"switch ($y".to_vec();
    switch.extend(b" + 1".repeat(4090));
    switch.extend_from_slice(b") { case 1");
    switch.extend(b" or 1".repeat(900));
    switch.extend_from_slice(b": $x = 1; }");
    deepest.push(("switch", switch));
    for (name, src) in deepest {
        let out = std::thread::Builder::new()
            .stack_size(half)
            .spawn(move || {
                torque_ts_compiler::compile_here(
                    &src,
                    Some(b"t.cs"),
                    Purpose::Run,
                    &Dialect::torque3d(),
                )
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(
            !out.syntax_error && out.dso.is_some(),
            "{name}: {:?}",
            common::texts(&out.diagnostics)
        );
    }
}

#[test]
fn the_source_bound_is_exact() {
    use torque_ts_compiler::MAX_SOURCE_BYTES;
    // Comment lines make no tokens, so the longest source the compiler takes is cheap to build: a source of
    // exactly MAX_SOURCE_BYTES compiles (to no code), one byte more is refused up front.
    let mut src = "//\n".repeat((MAX_SOURCE_BYTES - 1) / 3).into_bytes();
    src.resize(MAX_SOURCE_BYTES, b'\n');
    let start = Instant::now();
    let out = compile(&src, Some(b"big.cs"), Purpose::Run, &t3d());
    assert!(!out.syntax_error && out.dso.is_none() && out.diagnostics.is_empty());
    assert!(out.work >= MAX_SOURCE_BYTES as u64);
    let secs = start.elapsed().as_secs_f64();
    src.push(b'\n');
    let out = compile(&src, Some(b"big.cs"), Purpose::Run, &t3d());
    assert!(out.syntax_error && out.dso.is_none());
    eprintln!("a source of {MAX_SOURCE_BYTES} bytes: {secs:.2} s");
    assert!(secs < 60.0);
}

/// Another build of the format: its numbering (every code moved), a version, no local-variable table.
fn other_dialect() -> Dialect {
    let mut codes = [0u32; Op::COUNT];
    for (i, c) in codes.iter_mut().enumerate() {
        *c = (Op::COUNT - 1 - i) as u32 + 300;
    }
    Dialect {
        dso_version: 7,
        layout: FileLayout {
            local_variable_table: false,
        },
        opcodes: OpcodeTable::from_codes(codes).unwrap(),
    }
}

#[test]
fn a_dialect_profile_sets_the_numbering_version_and_layout() {
    let other = other_dialect();
    let src = "function f(%a) { %b = %a @ \"s\"; return %b; } $x = 0.1 + $y;";
    let ours = round_trip(src, &t3d());
    let theirs = round_trip(src, &other);
    assert_eq!(theirs.version, 7);
    assert!(theirs.local_variables.is_empty());
    assert!(!ours.local_variables.is_empty());
    // The same instructions, other code words.
    let names = |d: &DsoFile, dialect: &Dialect| -> Vec<Op> {
        decode_dso(d, dialect)
            .unwrap()
            .iter()
            .map(|i| i.op)
            .collect()
    };
    assert_eq!(names(&ours, &t3d()), names(&theirs, &other));
    assert_ne!(ours.code, theirs.code);
    // The same tables.
    assert_eq!(theirs.global_strings, ours.global_strings);
    assert_eq!(theirs.global_floats, vec![f64::from(0.1f32)]);
}

#[test]
fn torque3d_constructs_compile_to_their_nodes_code() {
    for (src, expected) in [
        // `foreach`: the container, OP_ITER_BEGIN (global flag, register, address past the loop), OP_ITER (the
        // `continue` target) leaving for OP_ITER_END (the `break` target), the body, a jump back.
        (
            "function f(%s) { foreach (%o in %s) { if (%o) break; continue; } }",
            r#"OP_FUNC_DECL "f" - - body=1 line=1 ->000034 n=1 2 r0 0 ->000000; OP_LOAD_LOCAL_VAR_STR r0; OP_ITER_BEGIN 0 r1 ->000033; OP_ITER ->000032; OP_LOAD_LOCAL_VAR_UINT r1; OP_JMPIFNOT ->000028; OP_JMP ->000032; OP_JMP ->000020; OP_JMP ->000020; OP_ITER_END; OP_RETURN_VOID; OP_RETURN_VOID"#,
        ),
        // `foreach$` over a global variable names it; `break` outside the body's loops leaves the iteration.
        (
            "foreach$ ($w in \"a b\") echo($w);",
            r#"OP_LOADIMMED_STR "a b"; OP_ITER_BEGIN_STR 1 "$w" ->000026; OP_ITER ->000025; OP_PUSH_FRAME 1; OP_SETCURVAR "$w"; OP_LOADVAR_STR; OP_PUSH; OP_CALLFUNC "echo" - 0; OP_POP_STK; OP_JMP ->000007; OP_ITER_END; OP_RETURN_VOID"#,
        ),
        // `assert`: the message in the current string table, the test as an integer, OP_ASSERT.
        (
            "assert($x, \"no x\"); function f() { assert(1); }",
            r#"OP_SETCURVAR "$x"; OP_LOADVAR_UINT; OP_ASSERT "no x"; OP_FUNC_DECL "f" - - body=1 line=1 ->000022 n=0 0; OP_LOADIMMED_UINT 1; OP_ASSERT "TorqueScript assert!"; OP_RETURN_VOID; OP_RETURN_VOID"#,
        ),
        // `->` and `-->`: the object, OP_SETCUROBJECT, OP_POP_STK, the name, OP_SETCUROBJECT_INTERNAL recursive.
        (
            "$a = $o->child; $b = $o-->(%n);",
            r#"OP_SETCURVAR "$o"; OP_LOADVAR_STR; OP_SETCUROBJECT; OP_POP_STK; OP_LOADIMMED_IDENT "child"; OP_SETCUROBJECT_INTERNAL 0; OP_SETCURVAR_CREATE "$a"; OP_SAVEVAR_UINT; OP_POP_STK; OP_SETCURVAR "$o"; OP_LOADVAR_STR; OP_SETCUROBJECT; OP_POP_STK; OP_LOAD_LOCAL_VAR_STR r0; OP_SETCUROBJECT_INTERNAL 1; OP_SETCURVAR_CREATE "$b"; OP_SAVEVAR_UINT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // Internal names and singletons set OP_CREATE_OBJECT's flags.
        (
            "new C([n]); singleton S(m);",
            r#"OP_LOADIMMED_UINT 0; OP_PUSH_FRAME 2; OP_LOADIMMED_IDENT "C"; OP_PUSH; OP_LOADIMMED_IDENT "n"; OP_PUSH; OP_CREATE_OBJECT - 0 1 0 1 ->000024; OP_ADD_OBJECT 1; OP_END_OBJECT 1; OP_FINISH_OBJECT; OP_POP_STK; OP_LOADIMMED_UINT 0; OP_PUSH_FRAME 2; OP_LOADIMMED_IDENT "S"; OP_PUSH; OP_LOADIMMED_IDENT "m"; OP_PUSH; OP_CREATE_OBJECT - 0 0 1 1 ->000050; OP_ADD_OBJECT 1; OP_END_OBJECT 1; OP_FINISH_OBJECT; OP_POP_STK; OP_RETURN_VOID"#,
        ),
        // Defaults: a flag per argument, the start of each default's code after the body's OP_RETURN_VOID, each
        // ended by OP_DEFAULT_END; the declaration ends after them.
        (
            "function f(%a, %b = 2, %c ? = %a @ \"x\", %d ?) { return %b; }",
            r#"OP_FUNC_DECL "f" - - body=1 line=1 ->000036 n=4 4 r0 r1 r2 r3 0 1 1 0 ->000000 ->000027 ->000030 ->000000; OP_LOAD_LOCAL_VAR_STR r1; OP_RETURN; OP_RETURN_VOID; OP_LOADIMMED_UINT 2; OP_DEFAULT_END; OP_LOAD_LOCAL_VAR_STR r0; OP_LOADIMMED_STR "x"; OP_REWIND_STR; OP_DEFAULT_END; OP_RETURN_VOID"#,
        ),
    ] {
        assert_eq!(listing(src), expected, "{src}");
    }
}
