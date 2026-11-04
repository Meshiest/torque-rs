//! The classic executor (TGE 1.4's VM): TGE 1.4-style scripts compiled by torque-ts-compiler's classic compiler,
//! loaded from their written bytes and run; their console lines and variables checked.

use std::rc::Rc;

use torque_ts_bytecode::classic::{ClassicDialect, OpcodeTable};
use torque_ts_compiler::classic::{Profile, compile};
use torque_ts_vm::code::CodeBlock;
use torque_ts_vm::exec::classic::{self, ClassicExecutor, TGE14_DIALECT};
use torque_ts_vm::host::Purpose;
use torque_ts_vm::vm::{Level, Vm, VmError};

fn classic_vm() -> Vm {
    let mut vm = Vm::with_executor(Rc::new(ClassicExecutor::new()));
    classic::install(&mut vm);
    vm
}

/// Compiles `src` as `test.cs`, writes the DSO and reads it back, and runs it in `vm`; returns the run's result.
fn run_in(vm: &mut Vm, src: &str) -> Result<String, VmError> {
    let out = compile(
        src.as_bytes(),
        Some(b"test.cs"),
        Purpose::Run,
        &Profile::tge14(),
    );
    assert!(!out.syntax_error, "{src}: {:?}", out.diagnostics);
    let dso = out.dso.expect("code");
    let dialect = ClassicDialect::tge14();
    let bytes = dialect.to_bytes(&dso);
    let read = dialect.parse(&bytes).expect("the file reads back");
    let block = CodeBlock::from_dso(&read, Some(b"test.cs"), &TGE14_DIALECT).expect("loads");
    vm.run(&Rc::new(block))
        .map(|r| String::from_utf8_lossy(&r).into_owned())
}

fn levels(vm: &mut Vm) -> Vec<(Level, String)> {
    vm.drain_log()
        .into_iter()
        .map(|l| (l.level, String::from_utf8_lossy(&l.text).into_owned()))
        .collect()
}

/// The console lines of `src` run in a fresh classic `Vm`.
fn lines(src: &str) -> Vec<String> {
    let mut vm = classic_vm();
    run_in(&mut vm, src).unwrap_or_else(|e| panic!("{src}: {e}"));
    levels(&mut vm).into_iter().map(|(_, t)| t).collect()
}

#[test]
fn values_convert_between_the_three_stacks() {
    assert_eq!(
        lines(
            "echo(1 + 2); echo(1.5 * 2); echo(10 / 4); echo(7 % 3); echo(1 << 4); echo(~0); echo(!0);\
             echo(\"a\" @ \"b\"); echo(\"a\" SPC \"b\"); echo(\"abc\" $= \"ABC\"); echo(5 - 7); echo(\"3abc\" + 1);\
             echo(\"x\" !$= \"y\"); echo(2 > 1 && 0); echo(0 || 3); echo(-(4)); echo(1, 2);"
        ),
        [
            // `||` and `&&` give the operand that decided them, not a truth value.
            "3", "3", "2.5", "1", "16", "-1", "1", "ab", "a b", "1", "-2", "4", "1", "0", "3", "-4",
            "12"
        ]
    );
}

#[test]
fn functions_loops_and_switches_run_as_tge_1_4s() {
    let src = r#"
function sumTo(%n)
{
   %total = 0;
   for (%i = 1; %i <= %n; %i++)
      %total += %i;
   return %total;
}

function countDown(%n)
{
   %steps = 0;
   do
   {
      %n--;
      %steps++;
   } while (%n > 0)
   return %steps;
}

function firstOver(%limit)
{
   %i = 0;
   for (;;)
   {
      if (%i * %i > %limit)
         break;
      %i++;
   }
   return %i;
}

function describe(%kind)
{
   switch$ (%kind)
   {
      case "apple" or "pear":
         return "fruit";
      case "carrot":
         return "vegetable";
      default:
         return "unknown";
   }
}

function fib(%n)
{
   return %n < 2 ? %n : fib(%n - 1) + fib(%n - 2);
}

echo(sumTo(10));
echo(countDown(3));
echo(countDown(0));
echo(firstOver(20));
echo(describe("PEAR") SPC describe("carrot") SPC describe("rock"));
echo(fib(12));
$list[1, "x"] = "cell";
echo($list1_x);
"#;
    assert_eq!(
        lines(src),
        [
            "55",
            "3",
            "1",
            "5",
            "fruit vegetable unknown",
            "144",
            "cell"
        ]
    );
}

#[test]
fn objects_fields_methods_and_packages() {
    let src = r#"
function Bag::add(%this, %item, %amount)
{
   %this.slot[%item] += %amount;
   return %this.slot[%item];
}

function Bag::describe(%this)
{
   return "bag of" SPC %this.capacity;
}

package Loud
{
   function Bag::describe(%this)
   {
      return Parent::describe(%this) @ "!";
   }
};

new ScriptObject(Sack)
{
   class = Bag;
   capacity = 10 * 2;
};

echo(Sack.add("rock", 2));
echo(Sack.add("rock", 3));
echo(Sack.slot["rock"]);
echo(Sack.describe());
activatePackage(Loud);
echo(Sack.describe());
deactivatePackage(Loud);
echo(Sack.describe());
echo(isObject(Sack) SPC Sack.getName());
"#;
    assert_eq!(
        lines(src),
        [
            "2",
            "5",
            "5",
            "bag of 20",
            "bag of 20!",
            "bag of 20",
            "1 Sack"
        ]
    );
}

/// TGE 1.4's reports: a missing function warns and leaves the result alone, a native given the wrong argument
/// count warns twice, a void native's result used warns.
#[test]
fn calls_report_as_tge_1_4_does() {
    let mut vm = classic_vm();
    let src = "$a = \"kept\"; $a = noSuchFunction(1); echo(\"next\"); $b = echo(\"x\");";
    run_in(&mut vm, src).unwrap();
    let log = levels(&mut vm);
    assert_eq!(
        log,
        [
            (
                Level::Warning,
                "test.cs (0): Unable to find function noSuchFunction".to_string()
            ),
            (Level::Normal, "next".to_string()),
            (Level::Normal, "x".to_string()),
            (
                Level::Warning,
                "test.cs (0): Call to echo in test.cs uses result of void function call."
                    .to_string()
            ),
        ]
    );
    run_in(&mut vm, "isObject();").unwrap();
    let log = levels(&mut vm);
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(
        log[0]
            .1
            .ends_with("::isObject - wrong number of arguments."),
        "{log:?}"
    );
    assert!(log[1].1.contains("usage: "), "{log:?}");
}

#[test]
fn a_run_that_makes_no_calls_warns_about_a_missing_object() {
    let src = b"function f(%o) { Parent::g(%o); }\nMissing.thing();\nnope();\n$z = 1;";
    let run = |rules: classic::ClassicRules| {
        let mut vm = Vm::with_executor(Rc::new(ClassicExecutor::with_dialect(
            &TGE14_DIALECT,
            OpcodeTable::TGE14,
            rules,
        )));
        classic::install(&mut vm);
        let files = torque_ts_vm::host::MemFiles::new();
        files.insert("a.cs", src);
        vm.set_file_host(files);
        run_in(&mut vm, "exec(\"a.cs\", true);").unwrap();
        levels(&mut vm)
            .into_iter()
            .map(|(_, t)| t)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        run(classic::ClassicRules::TGE14),
        [
            "Executing a.cs.",
            "a.cs (0): Unable to find object: 'Missing' attempting to call function 'thing'",
            "a.cs (0): Unable to find function nope",
        ]
    );
    let quiet = classic::ClassicRules {
        quiet_missing_object_without_calls: true,
        ..classic::ClassicRules::TGE14
    };
    assert_eq!(
        run(quiet),
        ["Executing a.cs.", "a.cs (0): Unable to find function nope"]
    );
}
