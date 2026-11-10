//! Torque's behaviour (Torque3D's where it differs from TGE 1.4) is what a `Vm` does until a game installs other rules,
//! through natives called from Rust (a `Vm` without an executor runs natives, not scripts).

use torque_ts_vm::fields::{self, FieldRules, FieldType, Value};
use torque_ts_vm::policy::{ConsoleRules, Limits, TagRules};
use torque_ts_vm::vm::{Level, Vm, VmError};

fn call(vm: &mut Vm, name: &str, args: &[&str]) -> Result<String, VmError> {
    let args: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
    vm.invoke(None, name.as_bytes(), &args)
        .map(|r| String::from_utf8_lossy(&r).into_owned())
}

fn text(vm: &mut Vm, name: &str, args: &[&str]) -> String {
    call(vm, name, args).expect(name)
}

fn lines(vm: &mut Vm) -> Vec<(Level, String)> {
    vm.drain_log()
        .into_iter()
        .map(|l| (l.level, String::from_utf8_lossy(&l.text).into_owned()))
        .collect()
}

#[test]
fn a_missing_global_function_warns_unknown_command_unless_the_rules_silence_it() {
    let mut vm = Vm::new();
    assert_eq!(text(&mut vm, "noSuchFunction", &[]), "");
    assert_eq!(
        lines(&mut vm),
        [(
            Level::Warning,
            "noSuchFunction: Unknown command.".to_string()
        )]
    );
    vm.set_console_rules(ConsoleRules {
        unknown_command: false,
        ..ConsoleRules::default()
    });
    assert_eq!(text(&mut vm, "noSuchFunction", &[]), "");
    assert!(lines(&mut vm).is_empty());
}

#[test]
fn a_flood_of_unknown_commands_prints_a_few_lines_a_second_and_how_many_it_left_out() {
    let mut vm = Vm::new();
    let mut printed = Vec::new();
    // 16,384 a tick for 10 ticks of 32 ms (a server's packets of remote commands nothing defines).
    for _ in 0..10 {
        for i in 0..16_384 {
            assert_eq!(
                text(&mut vm, &format!("clientCmdNothing{}", i % 7), &[]),
                ""
            );
        }
        printed.extend(lines(&mut vm));
        vm.advance_time(32);
    }
    assert_eq!(
        printed.len(),
        torque_ts_vm::vm::MAX_UNKNOWN_COMMAND_LINES as usize
    );
    // The next second: what was left out, then its own lines.
    vm.advance_time(1000);
    text(&mut vm, "clientCmdNothing", &[]);
    let next = lines(&mut vm);
    let left_out = 16_384 * 10 - torque_ts_vm::vm::MAX_UNKNOWN_COMMAND_LINES as usize;
    assert_eq!(
        next,
        [
            (
                Level::Warning,
                format!("{left_out} more unknown commands were not printed.")
            ),
            (
                Level::Warning,
                "clientCmdNothing: Unknown command.".to_string()
            ),
        ]
    );
}

#[test]
fn each_source_has_its_own_unknown_command_lines_and_a_timer_reports_what_it_left_out() {
    use torque_ts_vm::vm::{CommandSource, MAX_UNKNOWN_COMMAND_LINES};
    let mut vm = Vm::new();
    let max = MAX_UNKNOWN_COMMAND_LINES as usize;
    for source in [CommandSource(7), CommandSource(9)] {
        for _ in 0..max + 10 {
            vm.invoke_from(source, None, b"clientCmdNothing", &[])
                .unwrap();
        }
    }
    // A flood from source 7 does not silence source 9.
    assert_eq!(lines(&mut vm).len(), 2 * max);
    // The second ends with no other command: the clock alone prints what each left out.
    vm.advance_time(500);
    assert!(lines(&mut vm).is_empty());
    vm.advance_time(500);
    assert_eq!(
        lines(&mut vm),
        [
            (
                Level::Warning,
                "10 more unknown commands from source 7 were not printed.".to_string()
            ),
            (
                Level::Warning,
                "10 more unknown commands from source 9 were not printed.".to_string()
            ),
        ]
    );
    // A new second prints again.
    vm.invoke_from(CommandSource(7), None, b"clientCmdNothing", &[])
        .unwrap();
    assert_eq!(lines(&mut vm).len(), 1);
}

#[test]
fn strpos_finds_an_empty_needle_at_its_offset() {
    let mut vm = Vm::new();
    assert_eq!(text(&mut vm, "strpos", &["abc", ""]), "0");
    assert_eq!(text(&mut vm, "strpos", &["abc", "", "3"]), "3");
    assert_eq!(text(&mut vm, "strpos", &["abc", "", "4"]), "-1");
    assert_eq!(text(&mut vm, "strpos", &["abc", "c", "-1"]), "-1");
    assert_eq!(text(&mut vm, "strpos", &["abcabc", "c", "3"]), "5");
}

#[test]
fn the_unit_functions_take_an_unsigned_index() {
    let mut vm = Vm::new();
    // Past the last word, and a negative index (far past it): the text unchanged.
    assert_eq!(text(&mut vm, "removeWord", &["a b c", "5"]), "a b c");
    assert_eq!(text(&mut vm, "removeWord", &["a b c", "-1"]), "a b c");
    assert_eq!(text(&mut vm, "removeWord", &["a b c", "1"]), "a c");
    assert_eq!(text(&mut vm, "getWord", &["a b c", "-1"]), "");
    assert_eq!(text(&mut vm, "setWord", &["a b", "3", "x"]), "a b  x");
    // A negative index pads past the string-size cap.
    assert_eq!(
        call(&mut vm, "setWord", &["a b", "-1", "x"]),
        Err(VmError::StringTooLong)
    );
}

#[test]
fn error_and_get_sub_str_print_one_error_line() {
    let mut vm = Vm::new();
    text(&mut vm, "error", &["one ", "two"]);
    assert_eq!(text(&mut vm, "getSubStr", &["abc", "-1", "2"]), "");
    assert_eq!(
        lines(&mut vm),
        [
            (Level::Error, "one two".to_string()),
            (
                Level::Error,
                "getSubStr(...): error, starting position and desired length must be >= 0: (-1, 2)"
                    .to_string()
            ),
        ]
    );
}

#[test]
fn a_dead_tag_removed_again_is_ignored_and_ids_are_not_range_checked() {
    let mut vm = Vm::new();
    let tag = text(&mut vm, "addTaggedString", &["hello"]);
    text(&mut vm, "removeTaggedString", &[&tag]);
    // Both counts are 0 now: the second removal changes nothing, so the next two strings get two ids.
    text(&mut vm, "removeTaggedString", &[&tag]);
    let a = text(&mut vm, "addTaggedString", &["a"]);
    let b = text(&mut vm, "addTaggedString", &["b"]);
    assert_ne!(a, b);
    assert_eq!(text(&mut vm, "getTaggedString", &["\u{1}99999"]), "");
    assert!(lines(&mut vm).is_empty());
    vm.set_tag_rules(TagRules {
        range_errors: true,
        ..TagRules::default()
    });
    text(&mut vm, "getTaggedString", &["\u{1}99999"]);
    assert_eq!(
        lines(&mut vm),
        [(
            Level::Error,
            "NetStringTable::lookupString: tag id 99999 is past the table (16 slots)".to_string()
        )]
    );
}

#[test]
fn the_float_types_read_with_percent_g_unless_the_rules_say_percent_f() {
    let color = Value::ColorF([0.5, 1.0, 0.25, 1.0]);
    let point = Value::Floats2([1.5, 2.0]);
    let list = Value::FloatList(vec![0.5, 1.0].into());
    let none = |_| None;
    assert_eq!(
        fields::get(FieldType::ColorF, &color, &none),
        b"0.5 1 0.25 1"
    );
    assert_eq!(fields::get(FieldType::Point2F, &point, &none), b"1.5 2");
    assert_eq!(fields::get(FieldType::F32Vector, &list, &none), b"0.5 1");
    let fixed = FieldRules {
        fixed_color_f: true,
        fixed_point2f: true,
        fixed_float_list: true,
        ..FieldRules::default()
    };
    assert_eq!(
        fields::get_with(fixed, FieldType::ColorF, &color, &none),
        b"0.500000 1.000000 0.250000 1.000000"
    );
    assert_eq!(
        fields::get_with(fixed, FieldType::Point2F, &point, &none),
        b"1.500000 2.000000"
    );
    assert_eq!(
        fields::get_with(fixed, FieldType::F32Vector, &list, &none),
        b"0.500000 1.000000"
    );
}

#[test]
fn the_datablock_range_is_16384_ids_from_3() {
    let limits = Limits::default();
    assert_eq!(limits.datablock_ids, 16384);
    assert_eq!(limits.datablock_id_limit(), 16386);
    assert_eq!(limits.max_datablocks(), 16383);
    let mut vm = Vm::new();
    assert_eq!(vm.objects().datablock_id_limit(), 16386);
    vm.set_limits(Limits {
        datablock_ids: 8192,
        ..limits
    });
    assert_eq!(vm.objects().datablock_id_limit(), 8194);
    assert!(vm.objects().is_datablock_id(8194));
    assert!(!vm.objects().is_datablock_id(8195));
}

#[test]
fn the_random_generator_starts_at_the_global_seed() {
    let vm = Vm::new();
    assert_eq!(vm.random_seed(), torque_ts_vm::vm::INITIAL_RANDOM_SEED);
    assert_eq!(torque_ts_vm::vm::INITIAL_RANDOM_SEED, 1_376_312_589);
}

#[test]
fn the_object_tables_start_after_the_datablock_range() {
    use torque_ts_vm::sim::{Objects, SCRIPT_OBJECT, SimConfig, SimObject};
    let mut objects = Objects::new();
    let first = SimConfig::default().last_dynamic_id + 1;
    assert_eq!(first, 16387);
    assert_eq!(objects.find(b"ActiveActionMapSet"), Some(first));
    assert_eq!(objects.find(b"DataBlockGroup"), Some(first + 11));
    let object = SimObject::new(SCRIPT_OBJECT, &objects.classes);
    assert_eq!(objects.register(object).map(|r| r.id), Some(first + 12));
}

#[test]
fn an_unknown_package_is_reported_in_the_runtimes_own_words() {
    let mut vm = Vm::new();
    text(&mut vm, "activatePackage", &["NoSuchPackage"]);
    assert_eq!(
        lines(&mut vm),
        [(
            Level::Error,
            "activatePackage: no package named NoSuchPackage".to_string()
        )]
    );
}

#[test]
fn removing_a_dead_tag_twice_keeps_counts() {
    let mut vm = Vm::new();
    vm.set_tag_rules(TagRules {
        ignore_dead_removal: false,
        ..TagRules::default()
    });
    let tag = text(&mut vm, "addTaggedString", &["hello"]);
    text(&mut vm, "removeTaggedString", &[&tag]);
    // The rule only prints the line: the slot stays free, so the next string takes it and the one after another.
    text(&mut vm, "removeTaggedString", &[&tag]);
    assert!(
        lines(&mut vm)
            .iter()
            .any(|(_, l)| l.contains("Ref count is already 0"))
    );
    let a = text(&mut vm, "addTaggedString", &["a"]);
    let b = text(&mut vm, "addTaggedString", &["b"]);
    assert_eq!(a, tag);
    assert_ne!(a, b);
    assert_eq!(text(&mut vm, "getTaggedString", &[&a]), "a");
}
