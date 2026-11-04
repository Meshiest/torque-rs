use super::*;

fn split(text: &str) -> (String, Option<String>) {
    let (file, name) = split_sequence(text.as_bytes());
    (
        String::from_utf8_lossy(file).into_owned(),
        name.map(|n| String::from_utf8_lossy(n).into_owned()),
    )
}

#[test]
fn a_sequence_text_is_cut_as_tge_cuts_it() {
    assert_eq!(split("a.dsq"), ("a.dsq".into(), None));
    assert_eq!(split("a.dsq run"), ("a.dsq".into(), Some("run".into())));
    assert_eq!(split("a.dsq\trun"), ("a.dsq".into(), Some("run".into())));
    // Spaces and tabs after the first are skipped.
    assert_eq!(split("a.dsq \t  run"), ("a.dsq".into(), Some("run".into())));
    // With both a space and a tab, the name starts after the later of the two first ones, past the blanks that follow:
    // "x a<tab>b" is the file "x" and the name "b" (TGE writes its NUL at the earlier and reads on from the later).
    assert_eq!(split("x a\tb"), ("x".into(), Some("b".into())));
    assert_eq!(split("x\ta b c"), ("x".into(), Some("b c".into())));
    // A text that ends in blanks names the sequence nothing.
    assert_eq!(split("a.dsq "), ("a.dsq".into(), Some(String::new())));
    assert_eq!(split(" run"), (String::new(), Some("run".into())));
}

#[test]
fn a_sequence_text_is_copied_into_255_bytes() {
    let long = format!("{} n", "f".repeat(300));
    let (file, name) = split_sequence(long.as_bytes());
    assert_eq!(file.len(), MAX_SEQUENCE_TEXT);
    assert_eq!(name, None);
    let named = format!("f {}", "n".repeat(400));
    let (file, name) = split_sequence(named.as_bytes());
    assert_eq!(file, b"f");
    assert_eq!(name.map(<[u8]>::len), Some(MAX_SEQUENCE_TEXT - 2));
}

#[test]
fn a_loaded_result_is_present_unless_it_failed() {
    assert!(Loaded::Nothing.is_present() && Loaded::Unread.is_present());
    assert!(!Loaded::Failed.is_present());
}

#[test]
fn a_shape_with_one_collision_detail_reports_its_one_entry() {
    let facts = Facts {
        slots: shapes::preload::Slots {
            collision: vec![1],
            los: vec![2],
        },
        bounds: vec![[0.0; 6]],
        findings: vec![Some(Finding::Invalid)],
        debris_detail: -1,
        eye_node: -1,
        camera_node: -1,
        mount_nodes: [-1; shapes::preload::MOUNT_POINTS],
        ai_repair_node: -1,
    };
    let mut vm = Vm::new();
    report_findings(&mut vm, b"box.dts", &facts);
    let lines: Vec<_> = vm.drain_log().into_iter().map(|l| l.text).collect();
    assert_eq!(lines, [invalid_line(b"box.dts", 0, 1).into_bytes()]);
}
