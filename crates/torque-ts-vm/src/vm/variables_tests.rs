use super::*;

#[test]
fn atob_is_true_for_true_and_nonzero_numbers() {
    assert!(atob(b"TRUE") && atob(b"1") && atob(b"0.5") && atob(b"nan"));
    assert!(!atob(b"false") && !atob(b"0") && !atob(b"") && !atob(b"yes"));
}

#[test]
fn atob_converts_the_number_through_a_float() {
    // `export`'s append flag is `(F32)atof(s) != 0`, as a bool field's.
    for (text, want) in [
        (&b"1e-50"[..], false),
        (b"-1e-50", false),
        (b"7e-46", false),
        (b"7.1e-46", true),
        (b"1e-45", true),
        (b"0x1p-200", false),
        (b"1e-300", false),
        (b"1", true),
        (b"0", false),
        (b"true", true),
    ] {
        assert_eq!(atob(text), want, "{}", String::from_utf8_lossy(text));
    }
}

#[test]
fn names_compare_ignoring_ascii_case() {
    use std::cmp::Ordering::{Equal, Greater, Less};
    assert_eq!(cmp_ignoring_case(b"$A", b"$a"), Equal);
    assert_eq!(cmp_ignoring_case(b"$a", b"$B"), Less);
    assert_eq!(cmp_ignoring_case(b"$ab", b"$A"), Greater);
}

/// Exports `globals` (name, value) to a file and runs it in a fresh script engine: that engine.
fn export_and_read_back(globals: &[(String, Vec<u8>)]) -> (Vm, Vec<u8>) {
    let files = crate::host::MemFiles::new();
    let mut vm = Vm::new();
    vm.set_file_host(files.clone());
    for (name, value) in globals {
        vm.set_global_bytes(name, value);
    }
    vm.export_variables(b"$Pref::*", Some(b"prefs.cs"), false)
        .expect("exported");
    let written = files.get("prefs.cs").expect("the file was written");
    let mut back = Vm::new();
    back.set_file_host(files);
    back.exec_file(b"prefs.cs", false).expect("it runs");
    (back, written)
}

#[test]
fn export_never_writes_an_unterminated_statement() {
    // Escape-heavy bytes: quotes, a backslash and control bytes, each escaped to two to four bytes. TGE's color bytes
    // (1 to 7, 11, 12, 14, 15) escape to `\cN`, which TGE's lexer reads back as other bytes: they are swept below for
    // the statements only.
    let exact: Vec<u8> = (0x10u8..=0x1f).chain(*b"\"\\'\r\n\ta").collect();
    let colors: Vec<u8> = (1u8..=0x1f).collect();
    for len in (0..=2100).step_by(37) {
        let value: Vec<u8> = colors.iter().copied().cycle().take(len).collect();
        let (back, _) = export_and_read_back(&[
            ("$Pref::A".to_string(), value),
            ("$Pref::B".to_string(), b"after".to_vec()),
        ]);
        assert_eq!(
            back.global("$Pref::B").as_deref(),
            Some(&b"after"[..]),
            "{len}"
        );
        let value: Vec<u8> = exact.iter().copied().cycle().take(len).collect();
        let long_name = format!("$Pref::{}", "N".repeat(400));
        let globals = [
            ("$Pref::A".to_string(), value.clone()),
            (long_name.clone(), value.clone()),
            ("$Pref::B".to_string(), b"after".to_vec()),
        ];
        let (back, written) = export_and_read_back(&globals);
        for line in written.split(|&b| b == b'\n') {
            assert!(line.len() < MAX_LINE, "a line fits TGE's buffer");
        }
        for (name, want) in &globals {
            assert_eq!(
                back.global(name).unwrap_or_default(),
                *want,
                "{len}: {}",
                &name[..name.len().min(16)]
            );
        }
    }
    // A name too long for any statement is left out (with a warning), and the rest still read back.
    let huge = format!("$Pref::{}", "N".repeat(MAX_LINE));
    let (back, _) = export_and_read_back(&[
        (huge.clone(), b"x".to_vec()),
        ("$Pref::B".into(), b"y".to_vec()),
    ]);
    assert_eq!(back.global(&huge), None);
    assert_eq!(back.global("$Pref::B").as_deref(), Some(&b"y"[..]));
}

#[test]
fn two_long_prefs_never_run_as_script_when_read_back() {
    // The first value is longer than a line and holds a statement as plain text; the second needs no escape and, once
    // the first's line was cut, landed in code position and ran it.
    let payload = format!(
        "\"; $Injected = 1; echo(\"INJECTED\"); //{}",
        "x".repeat(1100)
    );
    let globals = [
        ("$Pref::Zz1".to_string(), payload.into_bytes()),
        (
            "$Pref::Zz2".to_string(),
            b";eval(getSubStr($Pref::Zz1,0,70));//".to_vec(),
        ),
    ];
    let (back, _) = export_and_read_back(&globals);
    assert_eq!(back.global("$Injected"), None, "nothing ran");
    for (name, want) in &globals {
        assert_eq!(back.global(name).as_deref(), Some(want.as_slice()));
    }
}
