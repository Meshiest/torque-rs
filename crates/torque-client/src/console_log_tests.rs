use super::*;
use ts_vm::vm::Vm;

fn temp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.scratch/test-tmp/console-log")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_log_starts_with_tges_header_and_writes_each_line_with_crlf_at_once() {
    let dir = temp("lines");
    let path = log_path(&dir);
    let mut vm = Vm::new();
    let log = attach(&mut vm, &path).unwrap();
    vm.print(b"Download finished");
    vm.warn("a warning about shapes/a/b.dts".to_string());
    vm.print(b"two\nlines");
    // Written before anything is dropped or flushed by the host: a client that stops here leaves them on disk.
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.split("\r\n").collect();
    assert!(
        lines[0].starts_with("//-------------------------- "),
        "{}",
        lines[0]
    );
    assert!(lines[0].ends_with(" -----"));
    assert_eq!(
        &lines[1..],
        [
            "Download finished",
            "a warning about shapes/a/b.dts",
            "two",
            "lines",
            ""
        ]
    );
    assert!(log.borrow().is_open());
    // A second start writes a new log and keeps the first as console.log.1; a third replaces that with the second.
    drop(vm);
    let mut vm = Vm::new();
    attach(&mut vm, &path).unwrap();
    vm.print(b"again");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 2, "{text}");
    let previous = previous_path(&path);
    assert_eq!(previous.file_name().unwrap(), "console.log.1");
    let kept = std::fs::read_to_string(&previous).unwrap();
    assert!(kept.contains("Download finished"), "{kept}");
    drop(vm);
    let mut vm = Vm::new();
    attach(&mut vm, &path).unwrap();
    let kept = std::fs::read_to_string(&previous).unwrap();
    assert!(
        kept.contains("again") && !kept.contains("Download"),
        "{kept}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn secrets_never_reach_the_file() {
    let ticket = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123";
    let sha = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
    let secrets = vec![b"Zq9-pass-word".to_vec()];
    let cases: [(String, String); 6] = [
        (
            format!("auth: ticket {ticket}"),
            format!("auth: ticket {REDACTED}"),
        ),
        (
            format!("POST /join ticket={ticket}&id=12345 HTTP/1.1"),
            format!("POST /join ticket={REDACTED}&id=12345 HTTP/1.1"),
        ),
        ("token=abc123&x=1".into(), format!("token={REDACTED}&x=1")),
        ("reply Zq9-pass-word".into(), format!("reply {REDACTED}")),
        (
            format!("a hash line (expected={sha}, actual={sha})"),
            format!("a hash line (expected={sha}, actual={sha})"),
        ),
        ("a plain line 92".into(), "a plain line 92".into()),
    ];
    for (line, want) in cases {
        assert_eq!(
            String::from_utf8(redact(line.as_bytes(), &secrets)).unwrap(),
            want
        );
    }
    let dir = temp("secrets");
    let path = log_path(&dir);
    let mut vm = Vm::new();
    let log = attach(&mut vm, &path).unwrap();
    log.borrow_mut().add_secret(b"Zq9-pass-word");
    vm.print(format!("echo ticket {ticket} code Zq9-pass-word").as_bytes());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains(ticket) && !text.contains("Zq9-pass-word"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_registered_name_is_redacted_like_the_built_in_ones() {
    let names = vec![b"sessionKey".to_vec()];
    assert_eq!(
        String::from_utf8(redact_with(b"SESSIONKEY=abc123&x=1", &[], &names)).unwrap(),
        format!("SESSIONKEY={REDACTED}&x=1")
    );
    assert_eq!(redact(b"sessionKey=abc123", &[]), b"sessionKey=abc123");
    let dir = temp("names");
    let path = log_path(&dir);
    let mut vm = Vm::new();
    let log = attach(&mut vm, &path).unwrap();
    log.borrow_mut().add_secret_name(b"sessionkey");
    vm.print(b"echo sessionKey=abc123");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("abc123"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_log_stops_at_its_cap_with_one_closing_line() {
    let dir = temp("cap");
    let path = log_path(&dir);
    let mut log = ConsoleLog::create(&path).unwrap();
    let line = vec![b'x'; 4000];
    let mut n = 0u64;
    while log.is_open() {
        log.line(Level::Normal, &line);
        n += 1;
        assert!(n < MAX_LOG_BYTES / 1000, "never capped");
    }
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size <= MAX_LOG_BYTES + CAP_LINE.len() as u64 + 2, "{size}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.ends_with(&format!("{CAP_LINE}\r\n")));
    log.line(Level::Normal, b"more");
    assert_eq!(std::fs::metadata(&path).unwrap().len(), size);
    let _ = std::fs::remove_dir_all(&dir);
}
