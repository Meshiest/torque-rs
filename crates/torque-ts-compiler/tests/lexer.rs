//! The scanner's line counting, long tokens and its read position.

use torque_ts_compiler::lexer::{Lexed, Lexer, TokenValue, tok};

/// The kinds and lines of every token of `src` (the scan stops at the end or a refusal).
fn kinds(src: &[u8]) -> Vec<(u8, u32)> {
    let mut lexer = Lexer::new(src);
    let mut out = Vec::new();
    while let Lexed::Token(t) = lexer.next_token() {
        out.push((t.kind, t.line));
    }
    out
}

#[test]
fn a_comment_ends_at_a_carriage_return_and_a_lone_cr_is_not_a_line() {
    // A line comment is `//[^\n\r]*` and a `\r` is skipped: the comment stops at the CR, `b` follows, and only
    // newlines count lines.
    assert_eq!(kinds(b"a //c\rb"), [(tok::IDENT, 1), (tok::IDENT, 1)]);
    assert_eq!(kinds(b"a //c\r\nb"), [(tok::IDENT, 1), (tok::IDENT, 2)]);
    assert_eq!(kinds(b"a //c\nb"), [(tok::IDENT, 1), (tok::IDENT, 2)]);
}

#[test]
fn a_token_may_be_any_length() {
    let mut name = vec![b'a'; 1 << 16];
    name.extend_from_slice(b" b");
    assert_eq!(kinds(&name), [(tok::IDENT, 1), (tok::IDENT, 1)]);
    let mut text = b"\"".to_vec();
    text.extend(std::iter::repeat_n(b'x', 1 << 16));
    text.extend_from_slice(b"\";");
    match all(&text).first() {
        Some(Lexed::Token(t)) => {
            assert_eq!(t.kind, tok::STRATOM);
            assert!(matches!(&t.value, TokenValue::Str(s) if s.len() == 1 << 16));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_input_is_read_a_line_at_a_time() {
    // The read position is the end of the line of the furthest byte a scan looked at.
    let src = b"a = 1;\nb = 2;\n\nc";
    let mut lexer = Lexer::new(src);
    assert_eq!(lexer.scan_index(), 0);
    lexer.next_token();
    assert_eq!(lexer.scan_index(), 7);
    for _ in 0..3 {
        lexer.next_token();
    }
    assert_eq!(
        lexer.scan_index(),
        7,
        "the `\\n` that ends `;` is on the first line"
    );
    lexer.next_token();
    assert_eq!(lexer.scan_index(), 14);
    while matches!(lexer.next_token(), Lexed::Token(_)) {}
    assert_eq!(lexer.scan_index(), src.len());
    // A NUL ends the input; a 0xFF byte is text (an illegal token outside strings and comments).
    let mut lexer = Lexer::new(b"a\0b");
    assert!(matches!(lexer.next_token(), Lexed::Token(_)));
    assert_eq!(lexer.next_token(), Lexed::End);
    assert_eq!(lexer.scan_index(), 1);
    assert_eq!(
        kinds(b"a \xff b"),
        [(tok::IDENT, 1), (tok::ILLEGAL, 1), (tok::IDENT, 1)]
    );
}

fn all(src: &[u8]) -> Vec<Lexed> {
    let mut lexer = Lexer::new(src);
    let mut out = Vec::new();
    loop {
        let t = lexer.next_token();
        let done = !matches!(t, Lexed::Token(_));
        out.push(t);
        if done {
            return out;
        }
    }
}

#[test]
fn block_comments_are_skipped_and_count_their_lines() {
    assert_eq!(
        kinds(b"a /* x\n y * / */ b /**/ c /*/ */ d"),
        [
            (tok::IDENT, 1),
            (tok::IDENT, 2),
            (tok::IDENT, 2),
            (tok::IDENT, 2)
        ]
    );
    // A long comment.
    let mut long = b"/*".to_vec();
    long.extend(std::iter::repeat_n(b'x', 3 * 4096));
    long.extend_from_slice(b"*/ a");
    assert_eq!(kinds(&long), [(tok::IDENT, 1)]);
    // No end: the scanner says so.
    assert_eq!(all(b"a /* x").last(), Some(&Lexed::UnterminatedComment));
}

#[test]
fn torque3d_keywords_and_operators() {
    assert_eq!(kinds(b"do dox"), [(tok::DO, 1), (tok::IDENT, 1)]);
    assert_eq!(
        kinds(b"foreach foreach$ foreachx in inx assert singleton"),
        [
            (tok::FOREACH, 1),
            (tok::FOREACHSTR, 1),
            (tok::IDENT, 1),
            (tok::IN, 1),
            (tok::IDENT, 1),
            (tok::ASSERT, 1),
            (tok::SINGLETON, 1)
        ]
    );
    // A keyword spelled in another case is a name.
    assert_eq!(kinds(b"Foreach IN"), [(tok::IDENT, 1), (tok::IDENT, 1)]);
    assert_eq!(
        kinds(b"a->b a-->b a-- >b a- >b"),
        [
            (tok::IDENT, 1),
            (tok::INTNAME, 1),
            (tok::IDENT, 1),
            (tok::IDENT, 1),
            (tok::INTNAMER, 1),
            (tok::IDENT, 1),
            (tok::IDENT, 1),
            (tok::MINUSMINUS, 1),
            (tok::char_token(b'>'), 1),
            (tok::IDENT, 1),
            (tok::IDENT, 1),
            (tok::char_token(b'-'), 1),
            (tok::char_token(b'>'), 1),
            (tok::IDENT, 1)
        ]
    );
}

#[test]
fn floats_are_single_precision_and_may_leave_out_the_integer_part() {
    let values = |src: &[u8]| -> Vec<TokenValue> {
        all(src)
            .into_iter()
            .filter_map(|t| match t {
                Lexed::Token(t) => Some(t.value),
                _ => None,
            })
            .collect()
    };
    assert_eq!(values(b"0.1"), [TokenValue::Float(f64::from(0.1f32))]);
    assert_eq!(
        values(b".5 .25e2 1e3"),
        [
            TokenValue::Float(0.5),
            TokenValue::Float(25.0),
            TokenValue::Float(1000.0)
        ]
    );
    // `.` before a name is the operator; `1.` is the integer and the operator.
    assert_eq!(
        kinds(b"a.b 1.x"),
        [
            (tok::IDENT, 1),
            (tok::char_token(b'.'), 1),
            (tok::IDENT, 1),
            (tok::INTCONST, 1),
            (tok::char_token(b'.'), 1),
            (tok::IDENT, 1)
        ]
    );
}
