use crate::common;

use common::{Sample, encode, t3d};
use torque_ts_bytecode::DsoFile;
use torque_ts_bytecode::decode::{
    DecodeError, Instruction, MAX_PROBLEMS, OperandKind, Table, decode_dso, render, verify,
};
use torque_ts_bytecode::opcodes::Op;

/// The code word of `op` in Torque3D's numbering.
fn c(op: Op) -> u32 {
    t3d().opcodes.code(op)
}

fn decode(dso: &DsoFile) -> Result<Vec<Instruction>, DecodeError> {
    decode_dso(dso, &t3d())
}

fn check(dso: &DsoFile) -> Vec<String> {
    verify(dso, &decode(dso).unwrap())
}

fn dso_with(code: Vec<u32>, identifiers: Vec<(u32, Vec<u32>)>) -> DsoFile {
    dso_with_tables(
        code,
        identifiers,
        b"t1add\0a\0",
        b"hi\0",
        vec![2.5],
        vec![7.0],
    )
}

/// A file around `code` with the given string tables and float tables.
fn dso_with_tables(
    code: Vec<u32>,
    identifiers: Vec<(u32, Vec<u32>)>,
    global_strings: &[u8],
    function_strings: &[u8],
    global_floats: Vec<f64>,
    function_floats: Vec<f64>,
) -> DsoFile {
    let s = Sample {
        version: 50,
        global_strings: global_strings.to_vec(),
        global_floats,
        function_strings: function_strings.to_vec(),
        function_floats,
        local_variables: vec![],
        code,
        line_breaks: vec![],
        identifiers,
    };
    DsoFile::parse(&encode(&s), &t3d()).unwrap()
}

#[test]
fn the_opcode_table_is_complete_unique_and_numbered_in_declaration_order() {
    assert_eq!(Op::ALL.len(), 90);
    let table = &t3d().opcodes;
    for (i, op) in Op::ALL.iter().enumerate() {
        assert_eq!(table.code(*op), i as u32);
        assert_eq!(table.op(i as u32), Some(*op));
    }
    assert!(table.op(90).is_none());
    assert!(table.op(0xFFFF_FFFF).is_none());
    let mut names: Vec<_> = Op::ALL.iter().map(|o| o.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 90);
    assert_eq!(c(Op::FuncDecl), 0);
    assert_eq!(c(Op::ReturnVoid), 15);
    assert_eq!(c(Op::PopStk), 69);
    assert_eq!(c(Op::PushFrame), 82);
    assert_eq!(c(Op::Invalid), 89);
    assert_eq!(Op::FuncDecl.name(), "OP_FUNC_DECL");
}

#[test]
fn decodes_a_simple_sequence() {
    let dso = dso_with(vec![c(Op::LoadImmedUint), 5, c(Op::Return)], vec![]);
    let ins = decode(&dso).unwrap();
    assert_eq!(ins.len(), 2);
    assert_eq!(ins[0].op, Op::LoadImmedUint);
    assert_eq!(ins[0].operands.len(), 1);
    assert_eq!(ins[0].operands[0].kind, OperandKind::Immediate);
    assert_eq!(ins[0].operands[0].word, 5);
    assert_eq!(ins[1].op, Op::Return);
    assert_eq!(ins[1].address, 2);
    assert!(check(&dso).is_empty());
}

#[test]
fn names_take_two_words() {
    // 0: SetCurVar <name: 1, 2>   3: LoadVarStr   4: CallFunc <name: 5, 6> <namespace: 7, 8> 0
    let code = vec![
        c(Op::SetCurVar),
        0,
        0,
        c(Op::LoadVarStr),
        c(Op::CallFunc),
        0,
        0,
        0,
        0,
        0,
        c(Op::ReturnVoid),
    ];
    let mut dso = dso_with(code, vec![(0, vec![1, 5])]);
    let ins = decode(&dso).unwrap();
    assert_eq!(
        ins.iter()
            .map(|i| (i.address, i.words()))
            .collect::<Vec<_>>(),
        vec![(0, 3), (3, 1), (4, 6), (10, 1)]
    );
    assert_eq!(ins[2].operands[1].address, 7);
    assert!(check(&dso).is_empty());
    // A nonzero high word is a problem: the loader writes the whole name over both words.
    dso.code[2] = 9;
    assert_one_problem(&check(&dso), "identifier operand 1 has a nonzero high word");
}

/// Code for `function t1add(%a) { "hi"; }` followed by one top-level string load.
fn function_sample() -> DsoFile {
    //  0: FuncDecl  1-2: name  3-4: namespace  5-6: package  7: body | line << 1  8: end  9: argc
    // 10: registers  11: argument 0's register  12: its flags  13: its default
    // 14: LoadImmedStr 0  16: ReturnVoid  17: LoadImmedStr 0
    let code = vec![
        c(Op::FuncDecl),
        0,
        0,
        0,
        0,
        0,
        0,
        1 | (4 << 1),
        17,
        1,
        1,
        0,
        0,
        0,
        c(Op::LoadImmedStr),
        0,
        c(Op::ReturnVoid),
        c(Op::LoadImmedStr),
        0,
    ];
    // The name at word 1 -> "t1add" (offset 0).
    dso_with(code, vec![(0, vec![1])])
}

#[test]
fn a_function_body_uses_the_function_string_table() {
    let dso = function_sample();
    let ins = decode(&dso).unwrap();
    assert_eq!(ins.len(), 4);
    assert_eq!(ins[0].op, Op::FuncDecl);
    // name, namespace, package, body and line, end, count, registers, then one argument's three words
    assert_eq!(ins[0].operands.len(), 10);
    assert_eq!(ins[0].words(), 14);
    assert_eq!(ins[0].operands[3].kind, OperandKind::BodyAndLine);
    assert_eq!(ins[0].operands[7].kind, OperandKind::Register);
    assert_eq!(ins[0].operands[9].kind, OperandKind::Target);
    assert_eq!(
        ins[1].operands[0].kind,
        OperandKind::StringIndex(Table::Function)
    );
    assert_eq!(ins[3].address, 17);
    assert_eq!(
        ins[3].operands[0].kind,
        OperandKind::StringIndex(Table::Global)
    );
    let problems = check(&dso);
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn render_resolves_identifiers_strings_registers_and_lines() {
    let dso = function_sample();
    let text = render(&dso, &decode(&dso).unwrap());
    assert!(
        text.contains("OP_FUNC_DECL \"t1add\" - - body=1 line=4 ->000017 n=1 1 r0 0 ->000000"),
        "{text}"
    );
    assert!(text.contains("OP_LOADIMMED_STR \"hi\""), "{text}");
}

#[test]
fn iterator_loops_name_a_global_or_a_register() {
    // 0: IterBegin global <name: 2, 3> ->7   5: Iter ->7   7: IterEnd
    // 8: IterBeginStr local r0 ->14         12: Iter ->14 14: IterEnd
    let code = vec![
        c(Op::IterBegin),
        1,
        0,
        0,
        7,
        c(Op::Iter),
        7,
        c(Op::IterEnd),
        c(Op::IterBeginStr),
        0,
        0,
        14,
        c(Op::Iter),
        14,
        c(Op::IterEnd),
    ];
    let dso = dso_with(code, vec![(0, vec![2])]);
    let ins = decode(&dso).unwrap();
    let kinds = |i: usize| ins[i].operands.iter().map(|o| o.kind).collect::<Vec<_>>();
    assert_eq!(
        kinds(0),
        vec![OperandKind::Flag, OperandKind::Ident, OperandKind::Target]
    );
    assert_eq!(
        kinds(3),
        vec![
            OperandKind::Flag,
            OperandKind::Register,
            OperandKind::Target
        ]
    );
    assert_eq!(ins[3].address, 8);
    assert!(check(&dso).is_empty(), "{:?}", check(&dso));
}

#[test]
fn render_quotes_string_bytes_explicitly_instead_of_decoding_them() {
    // The tables hold 8-bit text, so a byte such as 0xB0 (the degree sign) is not UTF-8 and must come out as
    // an escape, not as U+FFFD. Offsets: 0, 9, 18, 29, 38, 42.
    let strings: &[u8] = b"25\xb0 Ramp\0say \"hi\"\0back\\slash\0tab\there\0\xff\xbe\xe9\0\0";
    let code = vec![
        c(Op::LoadImmedStr),
        0,
        c(Op::LoadImmedStr),
        9,
        c(Op::LoadImmedStr),
        18,
        c(Op::LoadImmedStr),
        29,
        c(Op::LoadImmedStr),
        38,
        c(Op::LoadImmedStr),
        42,
        c(Op::LoadImmedStr),
        999,
        c(Op::SetCurVar),
        0,
        0,
        c(Op::Return),
    ];
    let dso = dso_with_tables(code, vec![(0, vec![15])], strings, b"", vec![], vec![]);
    let text = render(&dso, &decode(&dso).unwrap());
    let expected = [
        r#"000000: OP_LOADIMMED_STR "25\xb0 Ramp""#,
        r#"000002: OP_LOADIMMED_STR "say \"hi\"""#,
        r#"000004: OP_LOADIMMED_STR "back\\slash""#,
        r#"000006: OP_LOADIMMED_STR "tab\x09here""#,
        r#"000008: OP_LOADIMMED_STR "\xff\xbe\xe9""#,
        r#"000010: OP_LOADIMMED_STR """#,
        "000012: OP_LOADIMMED_STR <bad index>",
        r#"000014: OP_SETCURVAR "25\xb0 Ramp""#,
        "000017: OP_RETURN",
    ];
    assert_eq!(text, expected.join("\n") + "\n");
    assert!(!text.contains('\u{fffd}'), "{text}");
}

#[test]
fn invalid_and_unknown_opcodes_are_errors() {
    for word in [c(Op::Invalid), 90, 0xFFFF] {
        let dso = dso_with(vec![word], vec![]);
        assert_eq!(
            decode(&dso).unwrap_err(),
            DecodeError::InvalidOpcode { address: 0, word }
        );
    }
}

#[test]
fn missing_operands_are_an_error() {
    let dso = dso_with(vec![c(Op::LoadImmedUint)], vec![]);
    assert!(matches!(
        decode(&dso).unwrap_err(),
        DecodeError::TruncatedOperands {
            address: 0,
            op: Op::LoadImmedUint
        }
    ));
    // Half a name.
    let dso = dso_with(vec![c(Op::SetCurVar), 0], vec![]);
    assert!(matches!(
        decode(&dso).unwrap_err(),
        DecodeError::TruncatedOperands {
            op: Op::SetCurVar,
            ..
        }
    ));
    // A function declaration whose argument count runs past the end of the code.
    let dso = dso_with(
        vec![c(Op::FuncDecl), 0, 0, 0, 0, 0, 0, 1, 99, 1000, 0, 0],
        vec![],
    );
    assert!(matches!(
        decode(&dso).unwrap_err(),
        DecodeError::TruncatedOperands {
            op: Op::FuncDecl,
            ..
        }
    ));
}

#[test]
fn verify_reports_a_branch_into_the_middle_of_an_instruction() {
    // 0: Jmp ->3   2: LoadImmedUint 7   (address 3 is the operand of the load)
    let dso = dso_with(vec![c(Op::Jmp), 3, c(Op::LoadImmedUint), 7], vec![]);
    let problems = check(&dso);
    assert!(
        problems.iter().any(|p| p.contains("branch target")),
        "{problems:?}"
    );
    // The same jump to address 2 is fine.
    let dso = dso_with(vec![c(Op::Jmp), 2, c(Op::LoadImmedUint), 7], vec![]);
    assert!(check(&dso).is_empty());
}

#[test]
fn verify_reports_identifier_entries_that_do_not_point_at_identifier_operands() {
    // The identifier table claims word 1, but that is the immediate of LoadImmedUint.
    let dso = dso_with(
        vec![c(Op::LoadImmedUint), 5, c(Op::Return)],
        vec![(0, vec![1])],
    );
    let problems = check(&dso);
    assert!(
        problems.iter().any(|p| p.contains("identifier")),
        "{problems:?}"
    );
}

#[test]
fn verify_reports_string_and_float_indexes_that_are_out_of_range() {
    // LoadImmedStr 999 (no such offset) and LoadImmedFlt 5 (only one float).
    let dso = dso_with(
        vec![
            c(Op::LoadImmedStr),
            999,
            c(Op::LoadImmedFlt),
            5,
            c(Op::Return),
        ],
        vec![],
    );
    let problems = check(&dso);
    assert!(
        problems.iter().any(|p| p.contains("string")),
        "{problems:?}"
    );
    assert!(problems.iter().any(|p| p.contains("float")), "{problems:?}");
}

#[test]
fn verify_reports_registers_beyond_the_functions_count() {
    let mut dso = function_sample();
    // LoadLocalVarStr r1 in place of the string load: the function has one register.
    dso.code[14] = c(Op::LoadLocalVarStr);
    dso.code[15] = 1;
    assert_one_problem(
        &check(&dso),
        "OP_LOAD_LOCAL_VAR_STR at 14: register 1 is outside the function's 1 registers",
    );
    dso.code[15] = 0;
    assert!(check(&dso).is_empty());
}

/// Asserts that `verify` found exactly one problem and that its text contains `fragment`.
fn assert_one_problem(problems: &[String], fragment: &str) {
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains(fragment), "{problems:?}");
}

/// `SetCurVar <ident>` then `Return`: the identifier operand is code word 1.
fn set_cur_var(ident_word: u32) -> Vec<u32> {
    vec![c(Op::SetCurVar), ident_word, 0, c(Op::Return)]
}

#[test]
fn verify_reports_an_identifier_whose_string_index_is_not_a_string_start() {
    // The global table is "t1add\0a\0": strings start at 0 and 6; 2 is inside "t1add".
    for start in [0, 6] {
        let dso = dso_with(set_cur_var(0), vec![(start, vec![1])]);
        let problems = check(&dso);
        assert!(problems.is_empty(), "{start}: {problems:?}");
    }
    for inside in [2, 5, 7, 100] {
        let dso = dso_with(set_cur_var(0), vec![(inside, vec![1])]);
        let problems = check(&dso);
        assert_one_problem(
            &problems,
            &format!(
                "identifier string index {inside} is not the start of a string in the global table"
            ),
        );
    }
}

#[test]
fn verify_reports_the_same_code_position_listed_twice_in_the_identifier_table() {
    // Twice in two entries, and twice inside one entry.
    for identifiers in [vec![(0, vec![1]), (6, vec![1])], vec![(0, vec![1, 1])]] {
        let dso = dso_with(set_cur_var(0), identifiers.clone());
        let problems = check(&dso);
        assert_one_problem(&problems, "identifier position 1 appears more than once");
    }
    // Listed once it is fine.
    let dso = dso_with(set_cur_var(0), vec![(0, vec![1])]);
    assert!(check(&dso).is_empty());
}

#[test]
fn verify_reports_a_nonzero_identifier_operand_without_an_identifier_entry() {
    let dso = dso_with(set_cur_var(7), vec![]);
    let problems = check(&dso);
    assert_one_problem(
        &problems,
        "OP_SETCURVAR at 0: identifier operand 1 has value 7 but no identifier entry",
    );
    // A zero word is what an unpatched identifier looks like on disk, so it is fine without an entry...
    let dso = dso_with(set_cur_var(0), vec![]);
    assert!(check(&dso).is_empty());
    // ...and a nonzero one is fine once an entry claims it.
    let dso = dso_with(set_cur_var(7), vec![(0, vec![1])]);
    assert!(check(&dso).is_empty());
}

#[test]
fn verify_checks_string_and_float_immediates_against_the_table_of_their_own_scope() {
    // The global tables are "t1add\0a\0" and 3 floats; the function tables are "hi\0" and 1 float.
    // String offset 6 and float index 2 exist globally but not in the function tables.
    //  0: FuncDecl, name (identifier), namespace, package, body and line, end=16, 0 arguments, 0 registers
    // 11: LoadImmedStr 6  13: LoadImmedFlt 2   15: Return       (inside the function)
    // 16: LoadImmedStr 6  18: LoadImmedFlt 2   20: Return       (top level)
    let code = vec![
        c(Op::FuncDecl),
        0,
        0,
        0,
        0,
        0,
        0,
        1,
        16,
        0,
        0,
        c(Op::LoadImmedStr),
        6,
        c(Op::LoadImmedFlt),
        2,
        c(Op::Return),
        c(Op::LoadImmedStr),
        6,
        c(Op::LoadImmedFlt),
        2,
        c(Op::Return),
    ];
    let dso = dso_with_tables(
        code,
        vec![(0, vec![1])],
        b"t1add\0a\0",
        b"hi\0",
        vec![1.0, 2.0, 3.0],
        vec![7.0],
    );
    let problems = check(&dso);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(
        problems[0].contains(
            "OP_LOADIMMED_STR at 11: string offset 6 is not a string start in the Function table"
        ),
        "{problems:?}"
    );
    assert!(
        problems[1]
            .contains("OP_LOADIMMED_FLT at 13: float index 2 is outside the Function table of 1"),
        "{problems:?}"
    );
}

#[test]
fn verify_stops_collecting_at_max_problems() {
    // 60 string loads with an offset that is not in any table, then a Return.
    let mut code = Vec::new();
    for _ in 0..60 {
        code.extend([c(Op::LoadImmedStr), 999]);
    }
    code.push(c(Op::Return));
    let dso = dso_with(code, vec![]);
    let problems = check(&dso);
    assert_eq!(MAX_PROBLEMS, 50);
    assert_eq!(problems.len(), MAX_PROBLEMS);
}

/// A small deterministic LCG; the high bits are the usable ones.
fn next(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    *state
}

/// A value in `0..bound` (`bound` at most 65536).
fn pick(state: &mut u32, bound: u32) -> u32 {
    (next(state) >> 16) % bound
}

/// An operand-ish word. Plain words are 0, 1 or a small value (a valid opcode, index or target);
/// hostile ones add 0xFFFF_FFFF, 90 (not an opcode) and a full random u32.
fn operand_word(state: &mut u32, hostile: bool) -> u32 {
    match pick(state, if hostile { 10 } else { 7 }) {
        0 => 0,
        1 => 1,
        2..=6 => pick(state, 40),
        7 => 0xFFFF_FFFF,
        8 => 90,
        _ => next(state),
    }
}

/// Appends one instruction-shaped run: a valid opcode followed by operand words. One pick in
/// eight is `FuncDecl`, whose operand count depends on its own operands: it gets its ten fixed words with an
/// argument count of 0 or 1, then 0..=8 words.
fn push_instruction(code: &mut Vec<u32>, state: &mut u32, hostile: bool) {
    let op = loop {
        let op = if pick(state, 8) == 0 {
            Op::FuncDecl
        } else {
            Op::ALL[pick(state, Op::ALL.len() as u32) as usize]
        };
        if op != Op::Invalid {
            break op;
        }
    };
    code.push(c(op));
    if op == Op::FuncDecl {
        for k in 0..10 {
            code.push(if k == 8 {
                pick(state, 2)
            } else {
                operand_word(state, hostile)
            });
        }
    }
    // Half the time the words the fixed operands take, else 0..=8 words.
    let words = if pick(state, 2) == 0 {
        op.operands().iter().map(|s| s.words()).sum()
    } else {
        pick(state, 9)
    };
    for _ in 0..words {
        code.push(operand_word(state, hostile));
    }
}

/// A file around `code` whose identifier table points at pseudo-random code positions.
fn garbage_dso(code: Vec<u32>, state: &mut u32) -> DsoFile {
    let bound = code.len() as u32 + 2;
    let mut identifiers = Vec::new();
    for _ in 0..pick(state, 3) {
        let positions = (0..1 + pick(state, 3))
            .map(|_| pick(state, bound))
            .collect();
        identifiers.push((pick(state, 12), positions));
    }
    dso_with(code, identifiers)
}

/// What a sweep reached, so the test can prove it exercised more than the error paths.
#[derive(Default)]
struct Reached {
    ok: usize,
    with_operands: usize,
    with_func_decl: usize,
}

/// Decodes, and when that succeeds also verifies and renders. None of it may panic.
fn exercise(dso: &DsoFile, reached: &mut Reached) {
    if let Ok(ins) = decode(dso) {
        let _ = verify(dso, &ins);
        let _ = render(dso, &ins);
        reached.ok += 1;
        if ins.iter().any(|i| !i.operands.is_empty()) {
            reached.with_operands += 1;
        }
        if ins.iter().any(|i| i.op == Op::FuncDecl) {
            reached.with_func_decl += 1;
        }
    }
}

#[test]
fn garbage_code_never_panics() {
    // Deterministic pseudo-random sweeps: whatever the words are, the answer is Ok or Err.
    let mut state: u32 = 0x1234_5678;

    // Whole instructions: valid opcodes each followed by a random number of mixed operand words.
    let mut whole = Reached::default();
    for _ in 0..300 {
        // One case in three may also use the hostile words.
        let hostile = pick(&mut state, 3) == 0;
        let mut code = Vec::new();
        for _ in 0..1 + pick(&mut state, 8) {
            push_instruction(&mut code, &mut state, hostile);
        }
        let dso = garbage_dso(code, &mut state);
        exercise(&dso, &mut whole);
    }
    assert!(whole.ok >= 60, "only {} of 300 decoded", whole.ok);
    assert!(
        whole.with_operands >= 30,
        "only {} decoded instructions with operands",
        whole.with_operands
    );
    assert!(
        whole.with_func_decl >= 10,
        "only {} decoded a function declaration",
        whole.with_func_decl
    );

    // Function-declaration-shaped input: `end` before, inside or beyond the code, sometimes a
    // huge argument count, then a few instructions.
    let mut functions = Reached::default();
    for _ in 0..300 {
        let hostile = pick(&mut state, 3) == 0;
        let mut code = Vec::new();
        for _ in 0..pick(&mut state, 3) {
            code.push(c(Op::Return));
        }
        let decl = code.len() as u32;
        let mut body = Vec::new();
        for _ in 0..pick(&mut state, 5) {
            push_instruction(&mut body, &mut state, hostile);
        }
        let argc = match pick(&mut state, 8) {
            0 => 0xFFFF_FFFF,
            n => n % 4,
        };
        let end = match pick(&mut state, 4) {
            0 => pick(&mut state, decl + 1),
            1 => next(&mut state),
            _ => decl + 11 + 3 * argc.min(8) + pick(&mut state, body.len() as u32 + 1),
        };
        code.push(c(Op::FuncDecl));
        for _ in 0..6 {
            code.push(operand_word(&mut state, hostile));
        }
        code.extend([
            pick(&mut state, 2),
            end,
            argc,
            operand_word(&mut state, hostile),
        ]);
        for _ in 0..3 * argc.min(8) {
            code.push(operand_word(&mut state, hostile));
        }
        code.extend(body);
        let dso = garbage_dso(code, &mut state);
        exercise(&dso, &mut functions);
    }
    assert!(
        functions.with_func_decl >= 100,
        "only {} of 300 decoded a function declaration",
        functions.with_func_decl
    );
}
