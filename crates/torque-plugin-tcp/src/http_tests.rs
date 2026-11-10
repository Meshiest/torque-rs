use super::*;

/// The body lines `reads` give, fed as TGE's receive loop feeds `onReceive`.
fn body(reads: &[&[u8]]) -> Vec<String> {
    let mut http = Http::default();
    http.begin();
    let mut lines = Lines::default();
    let mut out = Vec::new();
    for read in reads {
        let mut at = 0;
        let mut calls = 0;
        while at < read.len() {
            let got = http
                .receive(&mut lines, &read[at..], 1 << 16)
                .expect("within the cap");
            at += got.used;
            calls += 1;
            assert!(calls < 10_000, "the reading moves on");
            out.extend(got.line.map(|l| String::from_utf8(l).expect("text")));
        }
    }
    if let Some(last) = lines.finish_last_line() {
        out.extend(
            http.process_line(&mut lines, last)
                .map(|l| String::from_utf8(l).expect("text")),
        );
    }
    out
}

#[test]
fn the_status_line_and_headers_are_taken_in_and_the_body_is_given_line_by_line() {
    let got = body(&[b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nfirst\r\nsecond\nlast"]);
    assert_eq!(got, ["first", "second", "last"]);
}

#[test]
fn a_chunked_body_joins_a_line_across_chunks_and_stops_at_the_last() {
    let answer: &[u8] =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nab\ncde\r\n4\r\nfg\nh\r\n0\r\n\r\nignored\n";
    assert_eq!(body(&[answer]), ["ab", "cdefg", "h"]);
    // The same answer read a byte at a time.
    let bytes: Vec<&[u8]> = answer.chunks(1).collect();
    assert_eq!(body(&bytes), ["ab", "cdefg", "h"]);
}

#[test]
fn only_the_exact_header_turns_chunking_on() {
    let got = body(&[b"HTTP/1.1 200 OK\r\nTransfer-Encoding:chunked\r\n\r\n3\r\nabc\r\n"]);
    assert_eq!(got, ["3", "abc"]);
}

#[test]
fn a_get_and_a_post_are_written_as_tge_writes_them() {
    let mut http = Http {
        request: Some(Request {
            host: "example.com".into(),
            path: b"/a b/c_d".to_vec(),
            query: Some(b"x=1&y=~".to_vec()),
            post: None,
        }),
        ..Http::default()
    };
    assert_eq!(
        http.begin(),
        b"GET /a%20b/c%5Fd?x=1%26y=%7E HTTP/1.1\r\nHost: example.com\r\n\r\n"
    );
    http.request = Some(Request {
        host: "example.com".into(),
        path: b"/form".to_vec(),
        query: None,
        post: Some(b"name=ada".to_vec()),
    });
    assert_eq!(
        http.begin(),
        &b"POST /form HTTP/1.1\r\nHost: example.com\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 8\r\n\r\nname=ada"[..]
    );
}

#[test]
fn a_long_path_is_cut_where_tge_cuts_it() {
    assert_eq!(expand_path(&[b'a'; 10_000]).len(), REQUEST_BUFFER - 3);
    assert_eq!(expand_path(&[b' '; 10_000]).len(), REQUEST_BUFFER - 2);
    assert_eq!(expand_path(&[0xE9]), b"%E9");
}

#[test]
fn a_hostile_chunk_size_wraps_and_never_stalls() {
    let got =
        body(&[b"HTTP/1.1 200 OK\r\ntransfer-encoding: CHUNKED\r\n\r\nzzzzzzzzzzzzzzzz\r\nabc\n"]);
    assert_eq!(got, ["abc"]);
}
