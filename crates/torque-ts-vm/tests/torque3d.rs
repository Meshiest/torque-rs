//! The Torque3D executor (`Vm::new`): scripts compiled by torque-ts-compiler and run, their console lines and
//! variables checked against what Torque3D's interpreter does with them.

use std::rc::Rc;

use torque_ts_vm::code::{CodeBlock, torque3d_format};
use torque_ts_vm::host::{MemFiles, Purpose};
use torque_ts_vm::vm::{Level, Vm, VmError};

/// Compiles `src` as the file `test.cs` and runs it in `vm`; returns the run's result.
fn run_in(vm: &mut Vm, src: &str) -> Result<String, VmError> {
    let out = torque_ts_compiler::compile(
        src.as_bytes(),
        Some(b"test.cs"),
        Purpose::Run,
        torque3d_format(),
    );
    assert!(!out.syntax_error, "{src}: {:?}", out.diagnostics);
    let dso = out.dso.expect("code");
    let block = CodeBlock::from_dso(&dso, Some(b"test.cs"), torque3d_format()).expect("loads");
    vm.run(&Rc::new(block))
        .map(|r| String::from_utf8_lossy(&r).into_owned())
}

/// The console lines of `src` run in a fresh `Vm`.
fn lines(src: &str) -> Vec<String> {
    let mut vm = Vm::new();
    run_in(&mut vm, src).unwrap_or_else(|e| panic!("{src}: {e}"));
    texts(&mut vm)
}

fn texts(vm: &mut Vm) -> Vec<String> {
    vm.drain_log()
        .into_iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect()
}

fn levels(vm: &mut Vm) -> Vec<(Level, String)> {
    vm.drain_log()
        .into_iter()
        .map(|l| (l.level, String::from_utf8_lossy(&l.text).into_owned()))
        .collect()
}

#[test]
fn values_keep_their_type_on_the_stack() {
    assert_eq!(
        lines(
            "echo(1 + 2); echo(1.5 * 2); echo(10 / 4); echo(0.1 + 0.2); echo(7 % 3); echo(1 << 4);\
             echo(~0); echo(!0); echo(\"a\" @ \"b\"); echo(\"a\" SPC \"b\"); echo(\"abc\" $= \"ABC\");\
             echo(\"x\" TAB \"y\" NL \"z\"); echo(5 - 7); echo(\"3abc\" + 1); echo(1 / 3); %t = 1; echo(%t / 3);"
        ),
        [
            "3",
            "3",
            "2.5",
            "0.300000004",
            "1",
            "16",
            "-1",
            "1",
            "ab",
            "a b",
            "1",
            // The console shows a tab as `^`.
            "x^y\nz",
            "-2",
            "4",
            // A constant expression is folded by the compiler in 64 bits; a value read as a float is a 32-bit
            // float's.
            "0.333333333",
            "0.333333333"
        ]
    );
}

#[test]
fn locals_live_in_registers_and_functions_return_typed_values() {
    let src = "function add(%a, %b) { %c = %a + %b; return %c; }\
               function half(%x) { return %x / 2; }\
               function count() { return 7; }\
               function word() { return \"w\"; }\
               echo(add(2, 3)); echo(half(3)); echo(count() + 1); echo(word() @ count());\
               $f = half(5); echo($f);";
    assert_eq!(lines(src), ["5", "1.5", "8", "w7", "2.5"]);
}

#[test]
fn missing_arguments_take_their_default_values() {
    let src = "function base() { return 10; }\
               function d(%a, %b = 10, %c = base() * 2) { return %a SPC %b SPC %c; }\
               function e(%a, %b) { return \"[\" @ %b @ \"]\"; }\
               echo(d(1)); echo(d(1, 2)); echo(d(1, 2, 3)); echo(e(1));";
    assert_eq!(lines(src), ["1 10 20", "1 2 20", "1 2 3", "[]"]);
}

#[test]
fn foreach_walks_a_set_and_foreach_string_walks_words() {
    let src = "new SimSet(S);\
               S.add(new ScriptObject(A)); S.add(new ScriptObject(B)); S.add(new ScriptObject(C));\
               foreach (%o in S) { if (%o.getName() $= \"B\") continue; echo(%o.getName()); }\
               foreach$ (%w in \"a b  c\") echo(\"[\" @ %w @ \"]\");\
               foreach$ ($g in \"x y z\") { if ($g $= \"y\") break; echo($g); }\
               echo(\"after \" @ $g);";
    assert_eq!(
        lines(src),
        ["A", "C", "[a]", "[b]", "[]", "[c]", "x", "after y"]
    );
}

#[test]
fn foreach_over_what_is_not_a_set_reports_it_and_skips_the_loop() {
    let mut vm = Vm::new();
    run_in(
        &mut vm,
        "foreach (%o in \"nothing\") echo(%o); echo(\"on\");",
    )
    .unwrap();
    assert_eq!(
        levels(&mut vm),
        [
            (Level::Error, "No SimSet object 'nothing'".to_string()),
            (
                Level::Error,
                "Did you mean to use 'foreach$' instead of 'foreach'?".to_string()
            ),
            (Level::Normal, "on".to_string()),
        ]
    );
}

#[test]
fn a_return_inside_foreach_leaves_the_stack_as_it_found_it() {
    let src = "function first(%s) { foreach$ (%w in %s) { foreach$ (%v in \"q r\") return %w @ %v; } return \"none\"; }\
               echo(first(\"x y\") @ \"-\" @ first(\"\") @ \"-\" @ first(\"p\"));";
    assert_eq!(lines(src), ["xq-none-pq"]);
}

#[test]
fn a_failed_assert_logs_an_error_and_the_run_goes_on() {
    let mut vm = Vm::new();
    run_in(
        &mut vm,
        "assert(1 == 1, \"fine\");\nassert(1 == 2, \"boom\");\necho(\"on\");",
    )
    .unwrap();
    assert_eq!(
        levels(&mut vm),
        [
            (Level::Error, "test.cs(2,0): {Fatal} - boom".to_string()),
            (Level::Normal, "on".to_string()),
        ]
    );
}

#[test]
fn internal_names_are_found_through_sets() {
    let src = "new SimGroup(G) { new ScriptObject([first]); new SimSet([inner]) { new ScriptObject([deep]) { v = 7; }; }; };\
               echo(G->first.getInternalName());\
               echo(G->deep);\
               echo(G-->deep.v);\
               echo(G.findObjectByInternalName(\"FIRST\") == G->first);\
               echo(nameToID(first)); echo(G.findObjectByInternalName(\"deep\", true).v);";
    let mut vm = Vm::new();
    run_in(&mut vm, src).unwrap();
    assert_eq!(texts(&mut vm), ["first", "0", "7", "1", "-1", "7"]);
    run_in(
        &mut vm,
        "%x = 5; echo(%x->a);\necho(Nothing->a);\necho(\"end\");",
    )
    .unwrap();
    assert_eq!(
        levels(&mut vm),
        [
            (
                Level::Error,
                "test.cs (1): Attempt to use ->, but the group object wasn't found.".to_string()
            ),
            (Level::Normal, "0".to_string()),
            (
                Level::Error,
                "test.cs (2): Attempt to use ->, but the group object wasn't found.".to_string()
            ),
            (Level::Normal, "0".to_string()),
            (Level::Normal, "end".to_string()),
        ]
    );
}

#[test]
fn a_singleton_fills_in_the_object_its_name_already_has() {
    let src = "singleton ScriptObject(One) { a = 1; };\nsingleton ScriptObject(One) { b = 2; };\n\
               echo(One.a SPC One.b SPC (nameToID(One) $= One.getId()));\n\
               singleton SimSet(One) { c = 3; };\necho(One.c);";
    let mut vm = Vm::new();
    run_in(&mut vm, src).unwrap();
    assert_eq!(
        levels(&mut vm),
        [
            (
                Level::Warning,
                "test.cs (2): Singleton Object was already created with name One. Using existing object."
                    .to_string()
            ),
            (Level::Normal, "1 2 1".to_string()),
            (
                Level::Error,
                "test.cs (4): Cannot re-declare object [One] with a different class [SimSet] - was [ScriptObject]."
                    .to_string()
            ),
            (Level::Normal, "".to_string()),
        ]
    );
}

#[test]
fn a_declaration_inside_a_field_value_does_not_replace_the_outer_one() {
    let src = "%o = new ScriptObject(Outer) { child = new ScriptObject(Inner) { v = 1; }; x = 5; };\
               echo(%o == nameToID(Outer)); echo(Outer.x); echo(Outer.child == nameToID(Inner)); echo(Inner.v);";
    assert_eq!(lines(src), ["1", "5", "1", "1"]);
}

#[test]
fn an_object_may_not_take_the_name_of_a_class() {
    let mut vm = Vm::new();
    run_in(&mut vm, "%o = new ScriptObject(SimSet); echo(%o);").unwrap();
    assert_eq!(
        texts(&mut vm),
        [
            "test.cs (1): Cannot name object [SimSet] the same name as a script class.",
            "0"
        ]
    );
}

#[test]
fn children_join_the_group_they_are_declared_in() {
    let src = "new SimGroup(G) { new ScriptObject(K1); new SimGroup(H) { new ScriptObject(K2); }; };\
               echo(G.getCount() SPC H.getCount() SPC (K2.getGroup() == nameToID(H)));";
    assert_eq!(lines(src), ["2 1 1"]);
}

#[test]
fn methods_parent_calls_and_packages() {
    let src = "function ScriptObject::hi(%this, %x) { return \"so\" @ %x; }\
               function Foo::hi(%this, %x) { return \"foo\" @ Parent::hi(%this, %x); }\
               package P { function Foo::hi(%this, %x) { return \"p\" @ Parent::hi(%this, %x); } };\
               new ScriptObject(F) { class = Foo; };\
               echo(F.hi(1)); activatePackage(P); echo(F.hi(2)); deactivatePackage(P); echo(F.hi(3));\
               echo(Foo::hi(F, 4));";
    assert_eq!(lines(src), ["fooso1", "pfooso2", "fooso3", "fooso4"]);
}

#[test]
fn calls_that_find_nothing_warn_as_torque3d_does() {
    let mut vm = Vm::new();
    run_in(
        &mut vm,
        "echo(nope());\necho(Ns::nope());\necho(Missing.thing());\nnew ScriptObject(O);\necho(O.thing());\necho(1);",
    )
    .unwrap();
    let id = vm.objects().find(b"O").unwrap();
    let got = texts(&mut vm);
    assert_eq!(
        got,
        [
            "test.cs (1): Unable to find function nope",
            "",
            "test.cs (2): Unable to find function Ns::nope",
            "",
            "test.cs (3): Unable to find object: 'Missing' attempting to call function 'thing'",
            "",
            "test.cs (5): Unknown command thing.",
            &format!("  Object O({id}) O -> ScriptObject -> SimObject"),
            "",
            "1",
        ]
    );
}

#[test]
fn a_message_about_the_last_statement_of_a_block_names_line_0() {
    // The line of a call is looked up two words after the call opcode, and the line-break table ends at the start
    // of the block's last statement: there the lookup finds nothing (Torque3D's lookup, kept).
    let mut vm = Vm::new();
    run_in(&mut vm, "echo(1);\nnope();").unwrap();
    assert_eq!(
        texts(&mut vm),
        ["1", "test.cs (0): Unable to find function nope"]
    );
}

#[test]
fn natives_warn_about_argument_counts_and_void_results() {
    let mut vm = Vm::new();
    run_in(
        &mut vm,
        "strlen();\n$x = echo(\"a\");\n$Con::warnVoidAssignment = 0;\n$y = echo(\"b\");",
    )
    .unwrap();
    assert_eq!(
        texts(&mut vm),
        [
            "test.cs (1): ::strlen - wrong number of arguments. got 1, expected 2 to 2",
            "test.cs (1): usage: strlen(text) - the length in bytes",
            "a",
            "test.cs (2): Call to echo in test.cs uses result of void function call.",
            "b",
        ]
    );
}

#[test]
fn natives_results_are_typed() {
    let src = "echo(strlen(\"abc\") + 1); echo(mSqrt(2)); echo(isObject(0) + 1); echo(getWord(\"a b\", 1));";
    assert_eq!(lines(src), ["4", "1.41421354", "1", "b"]);
}

#[test]
fn field_components_read_and_write_words_of_a_value() {
    let src = "%v = \"1 2 3\"; echo(%v.y); echo(%v.zx); %v.z = 9; echo(%v);\
               $p = \"4 5 6 7\"; echo($p.w); $p.x = 0; echo($p); %c = \"10 20 30\"; %c.rg = \"8 9\"; echo(%c);\
               new ScriptObject(O) { pos = \"1 1 1\"; }; echo(O.pos.y);";
    assert_eq!(
        lines(src),
        ["2", "3 1", "1 2 9", "7", "0 5 6 7", "8 9 30", "1"]
    );
}

#[test]
fn evaluated_text_runs_in_a_frame_of_its_own() {
    let src = "function f() { %x = 1; eval(\"%x = 2; $seen = %x;\"); return %x; } echo(f() SPC $seen);\
               echo(eval(\"return 4 + 5;\"));";
    assert_eq!(lines(src), ["1 2", "9"]);
}

#[test]
fn array_variables_and_globals() {
    let src = "$a[1] = \"one\"; $a[1, 2] = \"pair\"; echo($a1 SPC $a1_2);\
               function g() { %l[3] = 4; %l3 = 5; return %l3 SPC %l[3]; } echo(g());\
               $n = 3; $n++; $n += 2; echo($n); $s = \"x\"; $s = $s @ \"y\"; echo($s);";
    // `%l3` is a register, `%l[3]` the frame's variable `%l3`: two variables.
    assert_eq!(lines(src), ["one pair", "5 4", "6", "xy"]);
}

#[test]
fn control_flow() {
    let src = "for (%i = 0; %i < 3; %i++) echo(%i);\
               %j = 0; while (%j < 2) { %j++; if (%j == 1) continue; echo(\"w\" @ %j); }\
               if (\"abc\") echo(\"t1\"); if (\"0\") echo(\"t2\"); if (\"false\") echo(\"t3\"); else echo(\"f3\");\
               switch$ (\"b\") { case \"a\": echo(\"A\"); case \"b\" or \"c\": echo(\"BC\"); default: echo(\"D\"); }\
               switch (2) { case 1: echo(1); case 2: echo(2); }\
               echo(1 ? \"yes\" : \"no\"); echo(0 || 5 > 3); echo(1 && 0);";
    assert_eq!(
        lines(src),
        ["0", "1", "2", "w2", "t1", "f3", "BC", "2", "yes", "1", "0"]
    );
}

#[test]
fn tagged_strings() {
    let src = "%t = 'hello'; echo(getTaggedString(%t)); echo(getSubStr(%t, 0, 1) $= \"\\x01\"); echo(%t $= 'hello');";
    assert_eq!(lines(src), ["hello", "1", "1"]);
}

#[test]
fn a_run_that_makes_no_calls_declares_and_creates_nothing() {
    let mut vm = Vm::new();
    let files = MemFiles::new();
    files.insert(
        "a.cs",
        b"function f() { return 1; }\nnew ScriptObject(X);\necho(\"top\");\nnope();\n$z = 1;",
    );
    vm.set_file_host(files);
    run_in(
        &mut vm,
        "exec(\"a.cs\", true); echo(isFunction(f) SPC isObject(X));",
    )
    .unwrap();
    let got = texts(&mut vm);
    assert_eq!(
        got,
        [
            "Executing a.cs.",
            "a.cs (4): Unable to find function nope",
            "0 0"
        ]
    );
}

#[test]
fn the_step_limit_stops_a_loop() {
    let mut vm = Vm::new();
    vm.set_step_limit(Some(10_000));
    assert_eq!(run_in(&mut vm, "while (1) {}"), Err(VmError::StepLimit));
    // The Vm is usable afterwards.
    vm.set_step_limit(None);
    assert_eq!(run_in(&mut vm, "return 1 + 1;").unwrap(), "2");
}

#[test]
fn deep_recursion_stops_at_the_call_depth_budget() {
    let handle = std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            let mut vm = Vm::new();
            let src = "function r(%n) { if (%n <= 0) return 0; return 1 + r(%n - 1); }";
            run_in(&mut vm, src).unwrap();
            let fine = run_in(&mut vm, "return r(250);");
            let deep = run_in(&mut vm, "return r(400);");
            (fine, deep)
        })
        .unwrap();
    let (fine, deep) = handle.join().unwrap();
    assert_eq!(fine.unwrap(), "250");
    assert_eq!(deep, Err(VmError::CallDepth));
}

#[test]
fn script_functions_called_from_rust_get_text_arguments() {
    let mut vm = Vm::new();
    run_in(&mut vm, "function twice(%x) { return %x * 2; }").unwrap();
    let r = vm.invoke(None, b"twice", &[b"21".to_vec()]).unwrap();
    assert_eq!(r, b"42");
    run_in(
        &mut vm,
        "new ScriptObject(O); function O::name(%this) { return %this.getName(); }",
    )
    .unwrap();
    let id = vm.objects().find(b"O").unwrap();
    assert_eq!(vm.call_method(id, b"name", &[]).unwrap(), b"O");
}

#[test]
fn a_native_that_runs_script_shares_the_executor() {
    let src = "function inner(%a) { return %a @ \"!\"; } echo(call(\"inner\", \"x\") @ call(\"inner\", 1 + 1));";
    assert_eq!(lines(src), ["x!2!"]);
}
