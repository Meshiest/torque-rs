use super::*;

#[test]
fn strip_ml_control_chars_charges_every_pass() {
    let vm = Vm::new();
    let nested = format!("{}{}", "<b".repeat(20), "r>".repeat(20));
    let before = vm.objects.peek_work();
    let out = vm.strip_ml_control_chars(nested.as_bytes()).unwrap();
    // Each pass strips one <br> the previous pass made: 21 passes, the first over 80 bytes.
    assert!(out.is_empty(), "{out:?}");
    assert!(vm.objects.peek_work() - before >= 80 + 76, "work charged");
}

#[test]
fn a_scan_that_ends_in_no_tag_is_charged_by_the_bytes_it_looked_at() {
    let vm = Vm::new();
    // Eleven `<font:` that never see a `>` each look at the whole rest of the text.
    let text = format!("{}{}", "<font:".repeat(11), "a".repeat(10_000));
    let before = vm.objects.peek_work();
    let out = vm.strip_ml_control_chars(text.as_bytes()).unwrap();
    assert_eq!(out.len(), text.len(), "nothing is a tag");
    assert!(
        vm.objects.peek_work() - before >= 11 * 10_000,
        "the failed scans are work"
    );
}

#[test]
fn markup_tags_follow_the_prefix_order() {
    let tag = |t: &[u8]| markup_tag(t).0;
    assert_eq!(tag(b"<br>x"), Some(4));
    assert_eq!(tag(b"<COLOR:112233>"), Some(14));
    assert_eq!(tag(b"<color:1122>"), None);
    assert_eq!(tag(b"<a:x:y>z"), Some(3));
    assert_eq!(tag(b"<font:A:12>"), Some(11));
    assert_eq!(tag(b"<bitmap:a:b>"), None);
    assert_eq!(tag(b"<nope>"), None);
    assert_eq!(tag(b"<"), None);
    assert_eq!(tag(b""), None);
}

#[test]
fn replace_stops_writing_at_the_cap_instead_of_building_the_whole_result() {
    // Without the check inside the loop the result (100000 * 2048 bytes) would be built whole and
    // refused only at the end: the charge hook sees every replacement written.
    let hay = vec![b'a'; 100_000];
    let to = vec![b'b'; 2048];
    let mut written = 0usize;
    let r = replace(&hay, b"a", &to, |n| {
        written += n;
        Ok(())
    });
    assert_eq!(r, Err(VmError::StringTooLong));
    assert!(written <= MAX_STRING_BYTES, "{written} bytes written");
}

#[test]
fn a_failed_scan_reports_how_far_it_looked() {
    assert_eq!(markup_tag(b"<font:abc"), (None, 4));
    assert_eq!(markup_tag(b"<br>"), (Some(4), 0));
    assert_eq!(markup_tag(b"<nope"), (None, 0));
}

#[test]
fn strip_chars_and_next_token_are_linear_in_their_arguments() {
    // 4 MiB of text against a 4 MiB set none of whose bytes it holds: a scan of the set per byte would be 16e12 probes.
    let text = vec![b'a'; 4 << 20];
    let set = vec![b'b'; 4 << 20];
    let mut vm = Vm::new();
    vm.set_step_limit(Some(1_000_000));
    let start = std::time::Instant::now();
    let stripped = vm
        .invoke(None, b"stripChars", &[text.clone(), set.clone()])
        .unwrap();
    assert_eq!(stripped, text);
    let rest = vm
        .invoke(None, b"nextToken", &[text.clone(), b"tok".to_vec(), set])
        .unwrap();
    assert!(rest.is_empty());
    assert_eq!(vm.global("$tok").map(|t| t.len()), Some(text.len()));
    assert!(start.elapsed().as_secs() < 20, "{:?}", start.elapsed());
    // The set still works as a set.
    let out = vm
        .invoke(None, b"stripChars", &[b"a-b_c".to_vec(), b"_-".to_vec()])
        .unwrap();
    assert_eq!(out, b"abc");
    let rest = vm
        .invoke(
            None,
            b"nextToken",
            &[b"  x y".to_vec(), b"t".to_vec(), b" ".to_vec()],
        )
        .unwrap();
    assert_eq!(
        (rest.as_slice(), vm.global("$t").as_deref()),
        (&b"y"[..], Some(&b"x"[..]))
    );
}

#[test]
fn strip_ml_scans_stop_at_the_step_limit() {
    // Each `<font:` with no `>` scans the rest of the text, 65,000 steps of 4 MiB: the first scan passes the 100,000-step
    // limit, and the pass stops there instead of making 62 more.
    let text = format!("{}{}", "<font:".repeat(63), "a".repeat(4 << 20));
    let mut vm = Vm::new();
    vm.set_step_limit(Some(100_000));
    let before = vm.objects.peek_work();
    assert_eq!(
        vm.strip_ml_control_chars(text.as_bytes()),
        Err(VmError::StepLimit)
    );
    let work = vm.objects.peek_work() - before;
    assert!(work < 63 * (4 << 20), "the pass stopped early: {work}");
}
