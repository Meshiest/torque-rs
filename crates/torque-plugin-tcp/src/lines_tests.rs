use super::*;

/// Every line `reads` make, as TGE's receive loop calls `parseLine` until a read is used up.
fn split(lines: &mut Lines, reads: &[&[u8]]) -> Vec<String> {
    let mut out = Vec::new();
    for read in reads {
        let mut start = 0;
        while start < read.len() {
            let step = lines.parse_line(read, start, 64).expect("within the cap");
            assert!(step.next > start, "every call moves on");
            start = step.next;
            out.extend(step.line.map(|l| String::from_utf8(l).expect("text")));
        }
    }
    out
}

#[test]
fn lines_end_at_a_newline_or_a_nul_and_lose_one_carriage_return() {
    let mut lines = Lines::default();
    let got = split(&mut lines, &[b"one\r\ntwo\nthree\0\r\r\n\n"]);
    assert_eq!(got, ["one", "two", "three", "\r", ""]);
    assert_eq!(lines.finish_last_line(), None);
}

#[test]
fn a_line_split_across_reads_is_joined_and_the_rest_is_the_last_line() {
    let mut lines = Lines::default();
    let got = split(&mut lines, &[b"hel", b"lo\r", b"\nwor", b"ld\r"]);
    assert_eq!(got, ["hello"]);
    // The last line keeps its carriage return, as `finishLastLine` does not drop it.
    assert_eq!(lines.finish_last_line().as_deref(), Some(&b"world\r"[..]));
    assert!(!lines.is_buffered());
}

#[test]
fn a_line_past_the_cap_is_refused_whole_or_in_pieces() {
    let mut lines = Lines::default();
    assert_eq!(lines.parse_line(&[b'x'; 65], 0, 64), Err(TooLong));
    assert!(!lines.is_buffered());
    let mut long = vec![b'y'; 65];
    long.push(b'\n');
    assert_eq!(lines.parse_line(&long, 0, 64), Err(TooLong));
    assert!(lines.parse_line(&[b'z'; 40], 0, 64).is_ok());
    assert_eq!(lines.parse_line(&[b'z'; 40], 0, 64), Err(TooLong));
    assert!(!lines.is_buffered());
}

#[test]
fn an_empty_buffer_still_routes_the_next_piece_through_it() {
    let mut lines = Lines::default();
    lines.restore(Vec::new());
    let step = lines.parse_line(b"ab\r\n", 0, 64).expect("within the cap");
    assert_eq!(
        step,
        Step {
            next: 4,
            line: Some(b"ab".to_vec())
        }
    );
    assert!(!lines.is_buffered());
    // An empty buffer is no last line.
    lines.restore(Vec::new());
    assert_eq!(lines.finish_last_line(), None);
    assert!(lines.is_buffered());
}
