//! The classic dialect end to end: TGE 1.4-style scripts compiled with [`Profile::tge14`], then each DSO written,
//! read back, decoded, checked and turned into its text form and back, all without a difference.

mod common;

use std::path::Path;

use torque_ts_bytecode::DsoFile;
use torque_ts_bytecode::classic::decode::{decode_dso, verify};
use torque_ts_bytecode::classic::{ClassicDialect, Op, dsotext};
use torque_ts_compiler::Purpose;
use torque_ts_compiler::classic::{Generation, Profile, compile};
use torque_ts_compiler::lexer::{Language, Lexed, Lexer, TokenValue, tok};

/// A script in TGE 1.4's language: every statement form its grammar has, `do`-`while` and the `for` forms that
/// leave out a part included.
const CLASSIC: &str = r#"
// A small inventory, in the style of a TGE 1.4 game script.
$Inventory::count = 0;

function Inventory::add(%this, %item, %amount)
{
   if (%amount $= "")
      %amount = 1;
   %this.slot[%item] += %amount;
   $Inventory::count++;
   return %this.slot[%item];
}

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
   for (; %i < 0; )
      %i = 0;
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

function grade(%score)
{
   switch (%score)
   {
      case 10:
         return "A";
      case 8 or 9:
         return "B";
   }
   return %score >= 5 ? "pass" : "fail";
}

package Loud
{
   function describe(%kind)
   {
      return strupr(Parent::describe(%kind));
   }
};

datablock SimDataBlock(BaseItem)
{
   weight = 1.5;
   name = 'item';
};

new ScriptObject(Bag)
{
   class = Inventory;
   capacity = 10 * 2;
   new ScriptObject(Pocket)
   {
      capacity = 1;
   };
};

%list = "a" SPC "b" TAB "c" NL "d" @ "e";
%mask = (~0x0f & 255) | (1 << 3) ^ 6 % 4;
%neg = -(1.25 + 2) / 4 - !0;
$arr[1, "x"] = Bag.add("rock", 2);
echo(sumTo(10) @ " " @ countDown(3) @ " " @ firstOver(20) @ " " @ describe("pear"));
"#;

fn profile() -> Profile {
    Profile::tge14()
}

fn compiled(name: &str, source: &[u8]) -> DsoFile {
    let out = compile(source, Some(name.as_bytes()), Purpose::Run, &profile());
    assert!(
        !out.syntax_error,
        "{name}: {:?}",
        common::texts(&out.diagnostics)
    );
    out.dso.expect("a script with statements has code")
}

/// Writes, reads, decodes, checks and re-texts `dso`. `consistent`: its jumps land on instructions (not so after a
/// size mismatch).
fn round_trip(name: &str, dso: &DsoFile, consistent: bool) {
    let dialect = ClassicDialect::tge14();
    assert_eq!(dso.version, ClassicDialect::TGE14_DSO_VERSION, "{name}");
    assert!(dso.local_variables.is_empty(), "{name}");
    let bytes = dialect.to_bytes(dso);
    let back = dialect.parse(&bytes).expect("the written file reads back");
    assert_eq!(&back, dso, "{name}: bytes");
    assert_eq!(dialect.to_bytes(&back), bytes, "{name}: rewritten bytes");
    let instructions = decode_dso(dso, &dialect).expect("the code decodes");
    let problems = verify(dso, &dialect, &instructions);
    assert!(!consistent || problems.is_empty(), "{name}: {problems:?}");
    let text = dsotext::render(dso, &dialect);
    let parsed = dsotext::parse(&text, &dialect).expect("the text form reads back");
    assert_eq!(&parsed, dso, "{name}: text form");
}

#[test]
fn a_tge14_script_compiles_and_round_trips() {
    let dso = compiled("classic.cs", CLASSIC.as_bytes());
    round_trip("classic.cs", &dso, true);
    let dialect = ClassicDialect::tge14();
    let ops: Vec<Op> = decode_dso(&dso, &dialect)
        .unwrap()
        .iter()
        .map(|i| i.op)
        .collect();
    for op in [
        Op::FuncDecl,
        Op::CreateObject,
        Op::AddObject,
        Op::EndObject,
        Op::CallFuncResolve,
        Op::CallFunc,
        Op::TagToStr,
        Op::AdvanceStrAppendChar,
        Op::AdvanceStrComma,
        Op::CompareStr,
        Op::SetCurVarArrayCreate,
        Op::SaveFieldFlt,
        Op::OnesComplement,
        Op::JmpIfNp,
    ] {
        assert!(ops.contains(&op), "{} is not in the code", op.name());
    }
}

#[test]
fn the_corpus_scripts_in_the_classic_language_round_trip() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .expect("the corpus folder")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "cs"))
        .collect();
    names.sort();
    let mut compiled_files = 0;
    for path in names {
        let source = std::fs::read(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let out = compile(&source, Some(name.as_bytes()), Purpose::Run, &profile());
        // A corpus file written in Torque3D's language may not parse as TGE 1.4's.
        if out.syntax_error {
            continue;
        }
        let mismatch =
            common::texts(&out.diagnostics).contains(&"precompile size mismatch".to_string());
        if let Some(dso) = out.dso {
            round_trip(&name, &dso, !mismatch);
            compiled_files += 1;
        }
    }
    assert!(compiled_files >= 15, "only {compiled_files} files compiled");
}

/// A `do`-`while` loop runs its block before the first test: no jump precedes the block.
#[test]
fn a_do_while_loop_tests_after_its_block() {
    let dso = compiled("do.cs", b"do { $x++; } while ($x < 3)");
    let dialect = ClassicDialect::tge14();
    let ops: Vec<Op> = decode_dso(&dso, &dialect)
        .unwrap()
        .iter()
        .map(|i| i.op)
        .collect();
    let first_jump = ops
        .iter()
        .position(|op| matches!(op, Op::JmpIf | Op::JmpIfNot | Op::JmpIfF | Op::JmpIfFNot));
    let first_save = ops.iter().position(|op| *op == Op::SaveVarFlt);
    assert!(first_save < first_jump, "{ops:?}");
    assert_eq!(
        ops.iter()
            .filter(|op| matches!(op, Op::JmpIf | Op::JmpIfF))
            .count(),
        1
    );
    round_trip("do.cs", &dso, true);
}

/// The names Torque3D made keywords are names in TGE 1.4, and its additions to the grammar are syntax errors.
#[test]
fn torque3d_additions_are_not_classic() {
    for ok in [
        &b"foreach(1); assert(2); singleton(3); in(4);"[..],
        b"$a = 1.5e3; $b = 10.25;",
        b"for ($i = 0; ; $i++) break;",
    ] {
        let out = compile(ok, Some(b"ok.cs"), Purpose::Check, &profile());
        assert!(!out.syntax_error, "{}", String::from_utf8_lossy(ok));
    }
    for bad in [
        &b"foreach (%x in $set) echo(%x);"[..],
        b"new ScriptObject([inner]);",
        b"singleton ScriptObject(One) {};",
        b"$x = %obj->child;",
        b"function f(%a = 1) {}",
        b"datablock SimDataBlock(Empty) {};",
        b"$x = .5;",
        b"namespace = 1;",
    ] {
        let out = compile(bad, Some(b"bad.cs"), Purpose::Check, &profile());
        assert!(out.syntax_error, "{}", String::from_utf8_lossy(bad));
    }
}

/// TGE 1.4's scanner rounds a float constant to a single-precision number.
#[test]
fn classic_floats_round_to_single_precision() {
    let mut lexer = Lexer::with_language(b"0.1", Language::Tge14);
    match lexer.next_token() {
        Lexed::Token(t) => {
            assert_eq!(t.kind, tok::FLTCONST);
            assert_eq!(t.value, TokenValue::Float(f64::from(0.1f32)));
        }
        other => panic!("{other:?}"),
    }
    let mut lexer = Lexer::new(b"0.1");
    match lexer.next_token() {
        Lexed::Token(t) => assert_eq!(t.value, TokenValue::Float(f64::from(0.1f32))),
        other => panic!("{other:?}"),
    }
}

/// The first pass's operand order decides the order of the string table; the code and its size do not change.
#[test]
fn the_operand_order_reorders_only_the_tables() {
    let source = b"$x = \"left\" @ \"right\"; $y = $c ? \"yes\" : \"no\";";
    let mut right_first = profile();
    right_first.generation = Generation {
        right_operand_first: true,
        ..Generation::TGE14
    };
    let plain = compile(source, Some(b"t.cs"), Purpose::Run, &profile())
        .dso
        .unwrap();
    let swapped = compile(source, Some(b"t.cs"), Purpose::Run, &right_first)
        .dso
        .unwrap();
    let dialect = ClassicDialect::tge14();
    let strings = |d: &DsoFile| dialect.global_strings(d).as_bytes().to_vec();
    let order = |d: &DsoFile, a: &[u8], b: &[u8]| {
        let t = strings(d);
        let at = |w: &[u8]| t.windows(w.len()).position(|x| x == w).unwrap();
        at(a) < at(b)
    };
    assert!(order(&plain, b"left", b"right"));
    assert!(!order(&swapped, b"left", b"right"));
    assert!(order(&plain, b"yes", b"no"));
    assert!(!order(&swapped, b"yes", b"no"));
    assert_eq!(plain.code.len(), swapped.code.len());
}

/// `compile()`'s size mismatch is TGE 1.4's error; `exec`'s run of the same text warns.
#[test]
fn a_size_mismatch_is_reported_per_purpose() {
    let source = b"for (\"a\" @ \"b\"; $i < 1; $i++) {}";
    let run = compile(source, Some(b"m.cs"), Purpose::Run, &profile());
    let check = compile(source, Some(b"m.cs"), Purpose::Check, &profile());
    assert!(common::texts(&run.diagnostics).contains(&"precompile size mismatch".to_string()));
    assert!(
        check
            .diagnostics
            .iter()
            .any(|d| d.text == Generation::TGE14.check_mismatch)
    );
}
