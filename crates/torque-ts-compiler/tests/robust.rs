//! The compiler on hostile input: it never panics, always answers, and stays within fixed time and memory on
//! the worst inputs we know (a `switch` whose large test expression is compiled once per `or` value, four
//! million tokens). Each compile runs through [`compile`], which has its own worker thread.

mod common;

use std::process::Command;
use std::time::Instant;

use common::Rng;
use torque_ts_bytecode::Dialect;
use torque_ts_compiler::{Output, Purpose, classic, compile};

fn t3d() -> Dialect {
    Dialect::torque3d()
}

/// Sample sources (`tests/fixtures/corpus`).
const CORPUS: &[&[u8]] = &[
    include_bytes!("fixtures/corpus/001_assign.cs"),
    include_bytes!("fixtures/corpus/001_basic.cs"),
    include_bytes!("fixtures/corpus/002_ops.cs"),
    include_bytes!("fixtures/corpus/002_tags.cs"),
    include_bytes!("fixtures/corpus/003_assignops.cs"),
    include_bytes!("fixtures/corpus/004_calls.cs"),
    include_bytes!("fixtures/corpus/005_fields.cs"),
    include_bytes!("fixtures/corpus/006_if.cs"),
    include_bytes!("fixtures/corpus/007_loops.cs"),
    include_bytes!("fixtures/corpus/008_switch.cs"),
    include_bytes!("fixtures/corpus/009_functions.cs"),
    include_bytes!("fixtures/corpus/010_objects.cs"),
    include_bytes!("fixtures/corpus/011_strings.cs"),
    include_bytes!("fixtures/corpus/012_numbers.cs"),
    include_bytes!("fixtures/corpus/013_lines.cs"),
    include_bytes!("fixtures/corpus/014_crlf.cs"),
    include_bytes!("fixtures/corpus/015_case.cs"),
    include_bytes!("fixtures/corpus/016_mismatch.cs"),
    include_bytes!("fixtures/corpus/017_outside.cs"),
    include_bytes!("fixtures/corpus/018_toplevel_return.cs"),
    include_bytes!("fixtures/corpus/019_misc.cs"),
];

/// Compiles `src` both ways, with and without a file name, in both dialects, and checks what any compile must give: a
/// code only without a syntax error or for `exec`, a syntax error reported with a message, code that decodes.
fn hammer(src: &[u8]) {
    for (purpose, file) in [
        (Purpose::Run, Some(&b"t.cs"[..])),
        (Purpose::Check, Some(&b"t.cs"[..])),
        (Purpose::Run, None),
    ] {
        let out = compile(src, file, purpose, &t3d());
        sane(&out, purpose, src);
        classic_sane(&classic_compile(src, file, purpose), src);
    }
}

fn classic_compile(src: &[u8], file: Option<&[u8]>, purpose: Purpose) -> classic::Output {
    classic::compile(src, file, purpose, &classic::Profile::tge14())
}

/// What any compile of the TGE 1.4 grammar and its two-pass code generator must give: an answer that is not the worker's
/// panic, and a syntax error reported with a message.
fn classic_sane(out: &classic::Output, src: &[u8]) {
    let what = || String::from_utf8_lossy(&src[..src.len().min(80)]).into_owned();
    assert!(
        !out.diagnostics
            .iter()
            .any(|d| d.text == b"the compiler could not run"
                || d.text.starts_with(b"internal error")),
        "{}: the classic compiler panicked or could not run: {:?}",
        what(),
        common::texts(&out.diagnostics)
    );
    assert!(
        !out.syntax_error || !out.diagnostics.is_empty(),
        "{}: a classic error without a message",
        what()
    );
}

fn sane(out: &Output, purpose: Purpose, src: &[u8]) {
    let what = || String::from_utf8_lossy(&src[..src.len().min(80)]).into_owned();
    // `compile` turns a panic of its worker thread (or a thread that cannot start) into this message, so a
    // test that only looks at the output would take a panic for a syntax error.
    assert!(
        !out.diagnostics
            .iter()
            .any(|d| d.text == b"the compiler could not run"
                || d.text.starts_with(b"internal error")),
        "{}: the compiler panicked or could not run: {:?}",
        what(),
        common::texts(&out.diagnostics)
    );
    if out.syntax_error {
        assert!(
            !out.diagnostics.is_empty(),
            "{}: an error without a message",
            what()
        );
        if purpose == Purpose::Check {
            assert!(out.dso.is_none(), "{}: code after an error", what());
        }
    }
    if let Some(dso) = &out.dso {
        torque_ts_bytecode::decode::decode_dso(dso, &t3d())
            .unwrap_or_else(|e| panic!("{}: the code does not decode: {e}", what()));
        assert_eq!(
            torque_ts_bytecode::DsoFile::parse(&dso.to_bytes(&t3d()), &t3d()).as_ref(),
            Ok(dso)
        );
    }
    assert!(out.work >= torque_ts_text::until_nul(src).len() as u64);
}

#[test]
fn random_bytes() {
    let mut rng = Rng::new(0xb17e5);
    let weighted: &[u8] = b"$%abcxyz019_.\"'\\(){}[];,:=+-*/<>!&|^~?@#\n\r\t \x00\xff\x80";
    for i in 0..4000 {
        let len = rng.below(if i % 50 == 0 { 5000 } else { 120 });
        let src: Vec<u8> = (0..len)
            .map(|_| {
                if rng.chance(0.5) {
                    *rng.pick(weighted)
                } else {
                    rng.below(256) as u8
                }
            })
            .collect();
        hammer(&src);
    }
}

const PIECES: &[&str] = &[
    "function",
    "new",
    "break",
    "else",
    "continue",
    "if",
    "return",
    "while",
    "default",
    "for",
    "datablock",
    "switch",
    "switch$",
    "case",
    "or",
    "package",
    "true",
    "false",
    "do",
    "namespace",
    "NL",
    "TAB",
    "SPC",
    "==",
    "!=",
    ">=",
    "<=",
    "&&",
    "||",
    "::",
    "--",
    "++",
    "$=",
    "!$=",
    "<<",
    ">>",
    "+=",
    "-=",
    "*=",
    "/=",
    "%=",
    "&=",
    "^=",
    "|=",
    "<<=",
    ">>=",
    "$x",
    "%y",
    "$a::b",
    "foo",
    "Foo",
    "Parent",
    "1",
    "0x1F",
    "1.5",
    "1e5",
    "\"s\"",
    "\"\\x41\\c0\"",
    "\"\\x4\"",
    "'t'",
    "\"open",
    "//c\n",
    "/*",
    "*/",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    ";",
    ",",
    ":",
    "?",
    ".",
    "!",
    "~",
    "=",
    "+",
    "-",
    "*",
    "/",
    "%",
    "<",
    ">",
    "&",
    "|",
    "^",
    "@",
    "#",
    "\n",
    "\r",
    "\0",
    "\u{ff}",
];

#[test]
fn random_token_soups() {
    let mut rng = Rng::new(0x50a9);
    for _ in 0..4000 {
        let mut src = Vec::new();
        for _ in 0..rng.below(60) {
            let p = *rng.pick(PIECES);
            if p == "\u{ff}" {
                src.push(0xFF);
            } else {
                src.extend_from_slice(p.as_bytes());
            }
            if rng.chance(0.6) {
                src.push(b' ');
            }
        }
        hammer(&src);
    }
}

#[test]
fn random_programs_and_their_mutations() {
    let mut rng = Rng::new(0x9a7e);
    for i in 0..1500 {
        let mut src =
            common::program::random_program(&mut rng, 1 + i % 4, 1 + (i % 5) as u32).into_bytes();
        if i % 2 == 1 && !src.is_empty() {
            for _ in 0..1 + rng.below(4) {
                let at = rng.below(src.len());
                match rng.below(3) {
                    0 => src[at] = rng.below(256) as u8,
                    1 => {
                        src.remove(at);
                    }
                    _ => src.insert(at, *rng.pick(b"(){}[];,.\"'=\n\xff")),
                }
            }
        }
        hammer(&src);
    }
}

#[test]
fn every_prefix_and_suffix_of_the_corpus() {
    let mut n = 0;
    for src in CORPUS {
        for cut in 0..=src.len() {
            hammer(&src[..cut]);
            hammer(&src[cut..]);
            n += 2;
        }
        // With a NUL or a 0xFF byte in the middle.
        for cut in (0..src.len()).step_by(7) {
            for b in [0x00, 0xFF] {
                let mut s = src.to_vec();
                s.insert(cut, b);
                hammer(&s);
                n += 1;
            }
        }
    }
    eprintln!("{n} variants of {} corpus files", CORPUS.len());
}

fn repeat(prefix: &str, open: &str, middle: &str, close: &str, suffix: &str, n: usize) -> Vec<u8> {
    format!(
        "{prefix}{}{middle}{}{suffix}",
        open.repeat(n),
        close.repeat(n)
    )
    .into_bytes()
}

#[test]
fn deep_and_long_inputs_end_cleanly() {
    let shapes: &[(&str, &str, &str, &str, &str)] = &[
        ("$x = ", "(", "1", ")", ";"),
        ("$x = ", "- ", "1", "", ";"),
        ("$x = 1", " + 1", "", "", ";"),
        ("$x = 1", " @ \"s\"", "", "", ";"),
        ("$x = %a", ".f", "", "", ";"),
        ("$x = %a", ".f(1)", "", "", ";"),
        ("$x = %a", "[1", "", "]", ";"),
        ("$x = ", "%a[", "1", "]", ";"),
        ("", "if (1) ", "$x = 1;", "", ""),
        ("", "if (1) {", "$x = 1;", "}", ""),
        ("", "if (1) {} else ", "$x = 1;", "", ""),
        ("", "while (1) { ", "break;", "}", ""),
        ("function f() ", "{", "", "}", ""),
        ("", "new X() { ", "", "};", ""),
        ("switch (1) { ", "case 1: ", "", "", "}"),
        ("switch (1) { case 1", " or 1", "", "", ": }"),
        ("", "", "foo(", "1,", "1);"),
        ("", "$x = 1;", "", "", ""),
        ("", "function f() {}", "", "", ""),
        ("new X() { ", "a = 1; ", "", "", "};"),
        ("", "\n", "$x = 1 2;", "", ""),
        ("", "// comment\n", "", "", ""),
        ("", "\"str\" @ ", "1", "", ";"),
    ];
    let mut worst = (0.0f64, "");
    for &(prefix, open, middle, close, suffix) in shapes {
        for n in [10, 300, 345, 346, 4092, 4100, 40_000] {
            let src = repeat(prefix, open, middle, close, suffix, n);
            let start = Instant::now();
            let out = compile(&src, Some(b"t.cs"), Purpose::Run, &t3d());
            let secs = start.elapsed().as_secs_f64();
            sane(&out, Purpose::Run, &src);
            let start = Instant::now();
            classic_sane(&classic_compile(&src, Some(b"t.cs"), Purpose::Run), &src);
            let secs = secs.max(start.elapsed().as_secs_f64());
            if secs > worst.0 {
                worst = (secs, open);
            }
            assert!(secs < 20.0, "{open:?} x{n}: {secs} s");
        }
    }
    eprintln!("slowest: {:.2} s ({:?} x40000)", worst.0, worst.1);
}

#[test]
fn compile_and_parse_work_from_a_thread_with_256_kib_of_stack() {
    let handle = std::thread::Builder::new()
        .stack_size(256 << 10)
        .spawn(|| {
            let deep = format!("function f() {{ $x = 1{}; }}", " + 1".repeat(4000));
            let out = compile(deep.as_bytes(), Some(b"t.cs"), Purpose::Run, &t3d());
            assert!(!out.syntax_error && out.dso.is_some());
            let fields = format!("$x = %a{};", ".f[1]".repeat(4000));
            let out = compile(fields.as_bytes(), None, Purpose::Run, &t3d());
            assert!(!out.syntax_error && out.dso.is_some());
            let too_deep = format!("$x = 1{};", " @ 1".repeat(5000));
            let out = compile(too_deep.as_bytes(), Some(b"t.cs"), Purpose::Check, &t3d());
            assert!(out.syntax_error);
            // The parser runs on the caller's thread: the deepest trees it builds are dropped without
            // recursing, so it works here too.
            for (start, op) in [
                ("$x = 1", " + 1"),
                ("$x = %a", ".f(1)"),
                ("$x = 1", " ? 1 : 1"),
            ] {
                let src = format!("{start}{};", op.repeat(4090));
                let parsed = torque_ts_compiler::parser::parse(src.as_bytes(), Some(b"t.cs"));
                assert!(!parsed.syntax_error);
                drop(parsed);
            }
        })
        .unwrap();
    handle.join().unwrap();
}

/// The peak memory of this process, in bytes, where the platform tells it.
fn peak_memory() -> Option<u64> {
    if cfg!(windows) {
        let out = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id()),
            ])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    } else {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
        let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kib * 1024)
    }
}

/// The pathological inputs: (name, source, what the compile must end with).
fn pathological(name: &str) -> (Vec<u8>, Option<&'static str>) {
    let switch = |arg: &str, args: usize, values: usize| {
        let mut src = b"function f() { switch (foo(".to_vec();
        src.extend(format!("{arg},").repeat(args).bytes());
        src.extend_from_slice(arg.as_bytes());
        src.extend_from_slice(b")) { case 1");
        src.extend(b" or 1".repeat(values));
        src.extend_from_slice(b": $x = 1; } }");
        src
    };
    match name {
        // 4000 values over a 200001-argument call of floats: 800 million visits, which the visit bound stops at
        // 64 million (the generator stops descending there).
        "switch-visits" => (
            switch("1.5", 200_000, 4000),
            Some("the script is too large to compile"),
        ),
        // 400 values over a 20001-argument call: 24 million code words, past the code bound.
        "switch-words" => (
            switch("%x", 20_000, 400),
            Some("the script is too large to compile"),
        ),
        // A switch that fits: 300 values over 300 arguments.
        "switch-fits" => (switch("\"s\"", 300, 300), None),
        // 4.2 million tokens: a parameter list (the leanest tree per token).
        "tokens" => {
            let mut src = b"function f(".to_vec();
            src.extend(b"%a,".repeat(2_100_000));
            src.extend_from_slice(b"%a) {}");
            (src, Some("the file has more than 4194304 tokens"))
        }
        // Just under the token bound, as statements, compiled.
        "statements" => (b"$x = 1;".repeat(1_000_000), None),
        _ => panic!("no case {name}"),
    }
}

const CASES: &[&str] = &[
    "switch-visits",
    "switch-words",
    "switch-fits",
    "tokens",
    "statements",
];

/// Runs one pathological case when `TS_COMPILER_PATHOLOGICAL` names it (the parent test runs this test in a
/// child process per case, so each case's peak memory is its own).
#[test]
fn pathological_child() {
    let Ok(name) = std::env::var("TS_COMPILER_PATHOLOGICAL") else {
        return;
    };
    let (src, ending) = pathological(&name);
    let start = Instant::now();
    let out = compile(&src, Some(b"t.cs"), Purpose::Run, &t3d());
    let secs = start.elapsed().as_secs_f64();
    let texts = common::texts(&out.diagnostics);
    assert_eq!(texts.last().map(String::as_str), ending, "{name}");
    assert_eq!(out.dso.is_some(), ending.is_none(), "{name}");
    let words = out.dso.as_ref().map_or(0, |d| d.code.len());
    println!(
        "PATHOLOGICAL {name} secs={secs:.3} peak={} source={} work={} words={words}",
        peak_memory().unwrap_or(0),
        src.len(),
        out.work
    );
}

#[test]
fn pathological_inputs_stay_within_fixed_time_and_memory() {
    if std::env::var_os("TS_COMPILER_PATHOLOGICAL").is_some() {
        return;
    }
    let test_binary = std::env::current_exe().unwrap();
    for name in CASES {
        let out = Command::new(&test_binary)
            .args([
                "pathological_child",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("TS_COMPILER_PATHOLOGICAL", name)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{name}: {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        // The harness prints the test's name on the same line, before what the test prints.
        let line = stdout
            .lines()
            .find_map(|l| l.find("PATHOLOGICAL ").map(|at| &l[at..]))
            .unwrap_or_else(|| panic!("{name}: no result in {stdout}"));
        eprintln!("{line}");
        let field = |key: &str| -> f64 {
            line.split_whitespace()
                .find_map(|f| f.strip_prefix(key)?.strip_prefix('='))
                .and_then(|v| v.parse().ok())
                .unwrap()
        };
        let (secs, peak) = (field("secs"), field("peak"));
        // Debug-build bounds, several times what the cases need; a release build is about ten times faster.
        assert!(secs < 120.0, "{name}: {secs} s");
        if peak > 0.0 {
            assert!(peak < 1.5e9, "{name}: {peak} bytes");
        }
    }
}

/// The scanner's worst shape: lines of a string opener that never closes, then 2000 escaped
/// quotes. Every scan of one runs to the end of its line and falls back to an illegal one-byte token, so a
/// driver that went on scanning after that token would read about 4 KiB per source byte, as the lexer alone
/// does when drained past it. The parser has no rule that takes the illegal token and no error recovery, so
/// `compile` makes exactly one such scan and stops: its time and its work are those of the bytes it was handed.
fn hostile_lines(opener: u8, total: usize) -> Vec<u8> {
    let mut line = vec![opener];
    for _ in 0..2000 {
        line.extend_from_slice(&[0x5c, opener]);
    }
    line.push(b'\n');
    line.repeat(total / line.len() + 1)
}

#[test]
fn the_scanners_worst_shape_costs_compile_one_scan() {
    let mut worst = 0.0f64;
    for opener in *b"\"'" {
        // After code that compiles (so the error comes late), after a half statement, and from the start.
        for prefix in [&b""[..], b"$x = ", &b"$x = 1;\n".repeat(100_000)[..]] {
            for total in [1 << 20, 16 << 20] {
                let mut src = prefix.to_vec();
                src.extend(hostile_lines(opener, total));
                src.truncate(total.max(prefix.len() + 4100));
                for (purpose, file) in [
                    (Purpose::Check, Some(&b"t.cs"[..])),
                    (Purpose::Run, Some(&b"t.cs"[..])),
                    (Purpose::Run, None),
                ] {
                    let start = Instant::now();
                    let out = compile(&src, file, purpose, &t3d());
                    let secs = start.elapsed().as_secs_f64();
                    worst = worst.max(secs);
                    assert!(
                        out.syntax_error,
                        "{} bytes after {} of prefix",
                        total,
                        prefix.len()
                    );
                    // The report names the first hostile line: one source line per prefix statement.
                    if file.is_some() {
                        let first = common::texts(&out.diagnostics).remove(0);
                        let line = 1 + prefix.iter().filter(|&&b| b == b'\n').count();
                        assert!(
                            first.ends_with(&format!("Line: {line} - Syntax error.")),
                            "{first}"
                        );
                    }
                    // The work is the bytes plus the parser's steps up to the error (a few per statement of
                    // the prefix), and never a multiple of the bytes.
                    let statements = prefix.iter().filter(|&&b| b == b';').count() as u64;
                    assert!(
                        out.work <= src.len() as u64 + 64 + 40 * statements,
                        "{} bytes: work {}",
                        src.len(),
                        out.work
                    );
                    // 16 MiB of the plain scanner is about 6 s in a debug build; the 4 KiB-per-byte reading
                    // of the same input would be minutes.
                    assert!(
                        secs < 60.0,
                        "{} bytes after {} of prefix: {secs} s",
                        src.len(),
                        prefix.len()
                    );
                }
            }
        }
    }
    eprintln!("the worst shape through compile: slowest {worst:.2} s");
}

#[test]
fn long_tokens_compile_in_time_linear_in_the_bytes() {
    // Each token is about 4 KiB (an identifier, number, comment, string or variable) and the whole is a program, so
    // every scan is taken and no one stops the parser early.
    let name = "a".repeat(4095);
    let number = "7".repeat(4095);
    let comment = format!("//{}\n", "c".repeat(4090));
    let string = format!("\"{}\"", "s".repeat(4094));
    let escaped = format!("\"{}\"", "\\\"".repeat(2047));
    let variable = format!("${}", "v".repeat(4094));
    let colons = format!("${}", "a::".repeat(1364) + "a");
    let mut worst = 0.0f64;
    for (what, one) in [
        ("identifier", format!("{name}();\n")),
        ("number", format!("$x = {number};\n")),
        ("comment", format!("{comment}$x = 1;\n")),
        ("string", format!("$x = {string};\n")),
        ("escaped quotes", format!("$x = {escaped};\n")),
        ("variable", format!("{variable} = 1;\n")),
        ("variable with colons", format!("{colons} = 1;\n")),
    ] {
        for total in [1 << 20, 8 << 20] {
            let src = one.repeat(total / one.len()).into_bytes();
            let start = Instant::now();
            let out = compile(&src, Some(b"t.cs"), Purpose::Run, &t3d());
            let secs = start.elapsed().as_secs_f64();
            worst = worst.max(secs);
            sane(&out, Purpose::Run, &src);
            assert!(
                !out.syntax_error && out.dso.is_some(),
                "{what} x{}: {:?}",
                src.len() / one.len(),
                common::texts(&out.diagnostics)
            );
            assert!(secs < 60.0, "{what}, {total} bytes: {secs} s");
        }
    }
    eprintln!("the longest tokens: slowest {worst:.2} s");
}

/// A `switch` over a call of `args` arguments tests its expression once per `case` value (a chain of `or`
/// values may be 4000 long), so each argument adds one word per value; each `return;` after it adds exactly one.
fn words_source(args: usize, values: usize, returns: usize) -> Vec<u8> {
    let mut src = b"switch (foo(".to_vec();
    src.extend(b"*%x,".repeat(args));
    src.extend_from_slice(b"*%x)) { case 1");
    src.extend(b" or 1".repeat(values));
    src.extend_from_slice(b": $x = 1; }");
    src.extend(b"return;".repeat(returns));
    src
}

#[test]
fn the_code_word_bound_is_exact() {
    let max = torque_ts_compiler::codegen::MAX_CODE_WORDS as usize;
    let words = |args: usize, returns: usize| {
        let out = compile(
            &words_source(args, 4000, returns),
            Some(b"t.cs"),
            Purpose::Run,
            &t3d(),
        );
        out.dso.map(|d| d.code.len())
    };
    // Each argument adds a word per value (4001 of them); take as many as fit, then pad with `return;`s up
    // to exactly the bound.
    let (small, per_arg) = (
        words(10, 0).unwrap(),
        words(11, 0).unwrap() - words(10, 0).unwrap(),
    );
    assert_eq!(per_arg, 4001);
    let args = 10 + (max - small) / per_arg;
    let have = words(args, 0).unwrap();
    assert!(have <= max && max - have < per_arg, "{have} words");
    let pad = max - have;
    assert_eq!(words(args, pad), Some(max), "{max} words fit");
    let out = compile(
        &words_source(args, 4000, pad + 1),
        Some(b"t.cs"),
        Purpose::Run,
        &t3d(),
    );
    assert!(out.syntax_error && out.dso.is_none());
    assert_eq!(
        common::texts(&out.diagnostics),
        ["the script is too large to compile"]
    );
}
