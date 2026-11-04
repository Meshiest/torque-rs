//! The text form of a DSO (`torque_ts_bytecode::dsotext`): lossless on odd tables, and bad text is an error, never
//! a panic or an allocation the text's headers size.

use torque_ts_bytecode::dsotext::{parse, render};
use torque_ts_bytecode::opcodes::Op;
use torque_ts_bytecode::{Dialect, DsoFile, IdentEntry, LocalVariables};

fn t3d() -> Dialect {
    Dialect::torque3d()
}

/// The code word of `op` in Torque3D's numbering.
fn c(op: Op) -> u32 {
    t3d().opcodes.code(op)
}

fn sample() -> DsoFile {
    DsoFile {
        version: 50,
        // A quote, a backslash and a high byte; a tagged string's NUL padding; a final piece with no NUL.
        global_strings: b"a\"b\\c\xff\0tg\0\0\0\0\0\0\0\0unterminated".to_vec(),
        global_floats: vec![1.5, -0.0, f64::NAN, f64::INFINITY],
        function_strings: b"fn\0".to_vec(),
        function_floats: vec![0.1],
        local_variables: vec![
            LocalVariables {
                function: b"(null)::f".to_vec(),
                names: vec![b"%a".to_vec(), b"%\"q\"".to_vec()],
            },
            LocalVariables {
                function: b"Ns::g".to_vec(),
                names: vec![],
            },
        ],
        code: vec![
            c(Op::PushFrame),
            c(Op::LoadImmedStr),
            0,
            c(Op::Jmp),
            9999,
            0xdead_beef,
            c(Op::Return),
        ],
        line_breaks: vec![3, 1, 4, 3],
        identifiers: vec![IdentEntry {
            string_index: 0,
            positions: vec![2, 1],
        }],
    }
}

#[test]
fn text_reads_back_to_the_same_dso_even_when_the_code_does_not_decode_dso() {
    let dso = sample();
    let text = render(&dso, &t3d());
    // (A NaN is not equal to itself, so the bytes are compared.)
    // The quote and the backslash are escaped, the high byte is hex.
    assert!(text.contains(r#""a\"b\\c\xFF""#), "{text}");
    assert!(
        text.contains("local-variables 2\n  \"(null)::f\" \"%a\" \"%\\\"q\\\"\"\n  \"Ns::g\"\n"),
        "{text}"
    );
    let back = parse(&text).unwrap();
    assert_eq!(back.to_bytes(&t3d()), dso.to_bytes(&t3d()), "{text}");
    assert_eq!(render(&back, &t3d()), text);
    // The same after the commentary a person may add.
    let noisy = format!("# note\n\n{}", text.replace("dso 50", "dso 50   "));
    assert_eq!(
        parse(&noisy).unwrap().to_bytes(&t3d()),
        dso.to_bytes(&t3d())
    );
    // And a DSO with nothing in it.
    let empty = DsoFile {
        version: 50,
        global_strings: vec![],
        global_floats: vec![],
        function_strings: vec![],
        function_floats: vec![],
        local_variables: vec![],
        code: vec![],
        line_breaks: vec![],
        identifiers: vec![],
    };
    assert_eq!(parse(&render(&empty, &t3d())).unwrap(), empty);
}

#[test]
fn bad_text_is_an_error() {
    let good = render(&sample(), &t3d());
    assert!(
        good.contains("function-strings 3\n") && good.contains("\"fn\""),
        "{good}"
    );
    for (what, bad) in [
        ("empty", String::new()),
        ("no header", "global-strings 0\n".to_string()),
        (
            "a version that is not a number",
            good.replace("dso 50", "dso x"),
        ),
        (
            "a version beyond 32 bits",
            good.replace("dso 50", "dso 4294967296"),
        ),
        (
            "a table length that lies",
            good.replace("function-strings 3", "function-strings 4"),
        ),
        (
            "a float count that lies",
            good.replace("global-floats 4", "global-floats 5"),
        ),
        ("a code count that lies", good.replace("code 7", "code 8")),
        (
            "a pair count that lies",
            good.replace("line-breaks 2", "line-breaks 3"),
        ),
        (
            "an identifier count that lies",
            good.replace("identifiers 1", "identifiers 2"),
        ),
        (
            "a local-variable count that lies",
            good.replace("local-variables 2", "local-variables 3"),
        ),
        ("an unquoted local variable", good.replace("\"%a\"", "%a")),
        (
            "two local variables without a space",
            good.replace("\"%a\" ", "\"%a\""),
        ),
        ("an unquoted string", good.replace("\"fn\"", "fn")),
        ("a bad escape", good.replace("\"fn\"", "\"f\\q\"")),
        ("a short hex escape", good.replace("\"fn\"", "\"f\\x4\"")),
        ("a signed hex escape", good.replace("\"fn\"", "\"f\\x+1\"")),
        (
            "a hex escape cut in a character",
            good.replace("\"fn\"", "\"f\\x\u{e9}\""),
        ),
        (
            "bits that are not hex",
            good.replace("7ff0000000000000", "zzzz"),
        ),
        (
            "a float line without its bits",
            good.lines()
                .map(|l| {
                    if l.contains("7ff0000000000000") {
                        l.split_whitespace().next().unwrap_or("").to_string()
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        ),
        (
            "a code word that is not hex",
            good.replace("000000 52 49", "000000 52 zz"),
        ),
        ("a negative line", good.replace("  3 1\n", "  -3 1\n")),
        (
            "a missing table",
            good.replace("line-breaks 2\n  3 1\n  4 3\n", ""),
        ),
    ] {
        assert_ne!(bad, good, "{what}: the replacement found nothing");
        assert!(parse(&bad).is_err(), "{what} was accepted:\n{bad}");
    }
}

#[test]
fn headers_do_not_size_allocations_and_nothing_panics() {
    let head = "dso 50\nglobal-strings 0\nglobal-floats 0\nfunction-strings 0\nfunction-floats 0\nlocal-variables 0\n";
    // Counts far beyond the text itself: an error, quickly, without reserving them.
    let huge = [
        "dso 50\nglobal-strings 18446744073709551615\n".to_string(),
        "dso 50\nglobal-strings 0\nglobal-floats 99999999999999999\n".to_string(),
        "dso 50\nglobal-strings 0\nglobal-floats 0\nfunction-strings 0\nfunction-floats 0\nlocal-variables 18446744073709551615\n"
            .to_string(),
        format!("{head}code 4611686018427387904\n"),
        format!("{head}code 0\nline-breaks 9223372036854775808\nidentifiers 0\n"),
        format!("{head}code 0\nline-breaks 0\nidentifiers 18446744073709551615\n"),
        format!("{head}code 0\nline-breaks 0\nidentifiers 1\n  1 2 3 x\n"),
    ];
    for text in &huge {
        assert!(parse(text).is_err(), "{text}");
    }
    // Random damage to a good text: every outcome is Ok or Err.
    let good = render(&sample(), &t3d());
    let bytes = good.as_bytes();
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let (mut ok, mut err) = (0, 0);
    for _ in 0..3000 {
        let mut damaged = bytes.to_vec();
        for _ in 0..1 + next() % 4 {
            let at = (next() % damaged.len() as u64) as usize;
            match next() % 3 {
                0 => damaged[at] = (next() % 128) as u8,
                1 => {
                    damaged.remove(at);
                }
                _ => damaged.insert(at, b"\"\\x0 \n|#-9"[(next() % 10) as usize]),
            }
        }
        match parse(&String::from_utf8_lossy(&damaged)) {
            Ok(_) => ok += 1,
            Err(_) => err += 1,
        }
    }
    eprintln!("{ok} damaged texts still parse, {err} are refused");
    assert!(err > 1000, "{ok} ok, {err} err");
}
