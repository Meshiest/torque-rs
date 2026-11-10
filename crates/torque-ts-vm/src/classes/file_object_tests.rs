use super::*;

#[test]
fn lines_end_at_cr_lf_crlf_and_the_end_and_a_nul_cuts_the_string() {
    let text = b"a\r\nb\nc\rd\x00e\n\nlast";
    let mut at = 0;
    let mut lines = Vec::new();
    while at < text.len() {
        let (line, next) = next_line(text, at);
        assert!(next > at, "a line always moves on");
        lines.push(String::from_utf8(line).unwrap());
        at = next;
    }
    assert_eq!(lines, ["a", "b", "c", "d", "", "last"]);
    assert_eq!(next_line(b"x", 5), (Vec::new(), 5));
    assert_eq!(next_line(b"", 0), (Vec::new(), 0));
    assert_eq!(next_line(b"\r", 0), (Vec::new(), 1));
    assert_eq!(next_line(b"\r\n", 0), (Vec::new(), 2));
    assert_eq!(next_line(b"\n\r", 0), (Vec::new(), 1));
    assert_eq!(next_line(b"ab", usize::MAX), (Vec::new(), usize::MAX));
}

#[test]
fn the_open_files_hold_at_most_their_limit_exactly() {
    let mut files = FileObjects::with_limit(10);
    assert_eq!(files.open_read(1, vec![0; 6]), Ok(()));
    assert_eq!(files.open_read(2, vec![0; 4]), Ok(()));
    assert_eq!(files.held(), 10);
    // One more byte does not fit, and the refused open leaves its object closed.
    assert_eq!(files.open_read(3, vec![0; 1]), Err(1));
    assert!(files.is_eof(3));
    // An object that opens again gives its file back first (the caller closes): 6 bytes in 1's place fit,
    // 7 do not.
    assert!(files.close(1).is_none());
    assert_eq!(files.open_read(1, vec![0; 6]), Ok(()));
    assert!(files.close(1).is_none());
    assert_eq!(files.open_read(1, vec![0; 7]), Err(7));
    assert_eq!(files.held(), 4, "the refused open closed what 1 had");
    assert!(files.close(2).is_none());
    assert_eq!(files.held(), 0);
    // A write holds no bytes, and neither does closing nothing.
    assert!(files.open_write(4, b"config/x".to_vec(), 0));
    assert!(files.close(4).is_none());
    assert!(files.close(99).is_none());
    assert_eq!(files.held(), 0);
    // Lengths past the limit never overflow the sum.
    let mut files = FileObjects::with_limit(usize::MAX);
    assert_eq!(files.open_read(1, vec![0; 3]), Ok(()));
    files.read_bytes = usize::MAX - 2;
    assert_eq!(files.open_read(2, vec![0; 3]), Err(3));
}

#[test]
fn what_is_read_and_what_is_written_are_told_apart() {
    let mut files = FileObjects::with_limit(100);
    files.open_read(1, b"a\r\nb".to_vec()).unwrap();
    assert!(files.open_write(2, b"config/w.txt".to_vec(), 0));
    assert!(!files.is_eof(1) && files.is_eof(2) && files.is_eof(3));
    assert_eq!(files.read_line(1), Some((b"a".to_vec(), 3)));
    assert_eq!(files.read_line(1), Some((b"b".to_vec(), 1)));
    assert!(files.is_eof(1));
    assert_eq!(files.read_line(1), Some((Vec::new(), 0)));
    assert_eq!(files.read_line(2), None);
    assert_eq!(files.read_line(3), None);
    assert_eq!(files.write_target(1), None);
    assert_eq!(files.write_target(2).map(|(path, _)| path.len()), Some(12));
    // Closing a read gives its bytes back.
    assert!(files.close(1).is_none());
    assert_eq!(files.held(), 0);
    assert!(files.is_eof(1));
}

#[test]
fn a_file_is_open_for_writing_by_one_object_at_a_time_ignoring_case() {
    let mut files = FileObjects::with_limit(100);
    assert!(files.can_open_write(b"config/a.txt"));
    assert!(files.open_write(1, b"config/a.txt".to_vec(), 0));
    assert!(files.is_locked(b"CONFIG/A.TXT"));
    assert!(!files.can_open_write(b"Config/A.txt"));
    assert!(!files.open_write(2, b"Config/A.txt".to_vec(), 0));
    assert!(!files.is_locked(b"config/b.txt"));
    // The lines wait in the stream and come back at close, with the path; the lock goes.
    assert!(files.buffer_line(1, b"one").is_none());
    assert!(files.buffer_line(1, b"two").is_none());
    let flush = files.close(1).unwrap();
    assert_eq!(
        (&*flush.path, flush.bytes.as_slice()),
        (&b"config/a.txt"[..], &b"one\r\ntwo\r\n"[..])
    );
    assert!(!files.is_locked(b"config/a.txt"));
    assert!(files.close(1).is_none());
    // Nothing is kept for an object that is not writing.
    assert!(files.buffer_line(3, b"x").is_none());
}

/// What a stream gives the host for each write of `sizes` bytes (None: nothing), and at the end.
fn given(start: u64, sizes: &[usize]) -> (Vec<Option<usize>>, usize) {
    let mut stream = Stream::new(start);
    let mut each = Vec::new();
    for &n in sizes {
        let mut out = Vec::new();
        stream.write(&vec![b'x'; n], &mut out);
        each.push((!out.is_empty()).then_some(out.len()));
    }
    let mut rest = Vec::new();
    stream.flush(&mut rest);
    (each, rest.len())
}

#[test]
fn a_stream_is_an_8_kib_block_buffer() {
    assert_eq!(WRITE_BUFFER_BYTES, 8192);
    // A 10000-byte line at offset 0 goes whole (it spans past the block's end into an empty buffer), its CR LF
    // stays in the buffer.
    assert_eq!(given(0, &[10_000, 2]), (vec![Some(10_000), None], 2));
    // 10002 bytes, then 2 + 10000: the buffer takes the CR LF's block-mate (6382 bytes of the line), flushes
    // (16384 on disk in all), and the rest of the line starts a new buffer: 16384 until close, 20004 at it.
    assert_eq!(
        given(0, &[10_000, 2, 10_000, 2]),
        (vec![Some(10_000), None, Some(6_384), None], 3_620)
    );
    // 9000 + CR LF: 9000 on disk; "second" + CR LF: nothing; 9000 more: 16384 in all; close: all 18012.
    assert_eq!(
        given(0, &[9_000, 2, 6, 2, 9_000, 2]),
        (
            vec![Some(9_000), None, None, None, Some(7_384), None],
            1_628
        )
    );
    // 100 lines of 81 bytes: the buffer holds all 8100 and 3 bytes more.
    let mut lines = Vec::new();
    for _ in 0..100 {
        lines.extend_from_slice(&[79, 2]);
    }
    lines.extend_from_slice(&[1, 2]);
    let (each, rest) = given(0, &lines);
    assert!(each.iter().all(Option::is_none));
    assert_eq!(rest, 8_103);
    // A write that fits the block waits, a full block too; the next byte flushes it. (TGE tests `pos + n <=
    // block tail`, the last index: a write of the whole 8192 bytes of an empty buffer goes whole, too.)
    assert_eq!(given(0, &[8_191, 1, 1]), (vec![None, None, Some(8_192)], 1));
    assert_eq!(given(0, &[8_192, 1]), (vec![Some(8_192), None], 1));
    // Blocks are aligned to the file's offset: an appender at 100 bytes has 8092 bytes of its first block.
    assert_eq!(given(100, &[8_000, 2]), (vec![None, None], 8_002));
    assert_eq!(
        given(100, &[8_000, 2, 8_000]),
        (vec![None, None, Some(8_092)], 7_910)
    );
    // A write that spans past the end of a block into an empty buffer goes whole, anywhere in the file.
    assert_eq!(given(8_000, &[5_000]), (vec![Some(5_000)], 0));
    assert_eq!(given(8_000, &[100, 1_000]), (vec![None, Some(192)], 908));
    assert_eq!(given(0, &[]), (vec![], 0));
    // Offsets at the end of the range saturate: no panic, nothing lost.
    let (each, rest) = given(u64::MAX - 5, &[100, 100]);
    let total: usize = each.iter().flatten().sum::<usize>() + rest;
    assert_eq!(total, 200);
}

#[test]
fn a_deleted_writer_leaves_its_lock_and_drops_its_stream_up_to_the_cap() {
    let mut files = FileObjects::with_limit(100);
    assert!(files.open_write(1, b"config/a.txt".to_vec(), 0));
    assert!(files.buffer_line(1, b"lost").is_none());
    assert!(files.abandon(1).is_none());
    // The stream is gone, the lock stays, and nobody else gets the file.
    assert!(files.is_locked(b"config/a.txt"));
    assert!(!files.can_open_write(b"config/a.txt"));
    assert!(files.close(1).is_none());
    assert!(files.is_locked(b"config/a.txt"));
    assert!(!files.open_write(2, b"config/a.txt".to_vec(), 0));
    assert!(files.buffer_line(1, b"x").is_none());
    // A read is given up as by `close`.
    files.open_read(5, vec![0; 3]).unwrap();
    assert!(files.abandon(5).is_none());
    assert_eq!(files.held(), 0);
    // Past the cap a deleted writer closes as a live one does.
    for i in 0..MAX_LEAKED_LOCKS {
        let id = 100 + i as ObjId;
        assert!(files.open_write(id, format!("config/leak{i}").into_bytes(), 0));
        assert!(files.abandon(id).is_none());
    }
    assert!(files.open_write(9, b"config/past".to_vec(), 0));
    assert!(files.buffer_line(9, b"kept").is_none());
    let flush = files.abandon(9).unwrap();
    assert_eq!(flush.bytes, b"kept\r\n");
    assert!(!files.is_locked(b"config/past"));
    assert!(files.is_locked(b"config/leak0"));
}

#[test]
fn what_a_reader_has_not_read_is_unread() {
    let mut files = FileObjects::with_limit(100);
    assert!(!files.has_unread(1));
    files.open_read(1, b"a\r\nb".to_vec()).unwrap();
    assert!(files.has_unread(1));
    files.read_line(1);
    assert!(files.has_unread(1));
    files.read_line(1);
    assert!(!files.has_unread(1));
    assert!(files.open_write(2, b"config/w".to_vec(), 0));
    assert!(!files.has_unread(2));
}

#[test]
fn at_most_max_open_writers_files_are_open_for_writing() {
    let mut files = FileObjects::with_limit(100);
    for i in 0..MAX_OPEN_WRITERS {
        assert!(files.open_write(i as ObjId, format!("config/f{i}").into_bytes(), 0));
    }
    assert!(!files.can_open_write(b"config/one-more"));
    assert!(!files.open_write(MAX_OPEN_WRITERS as ObjId, b"config/one-more".to_vec(), 0));
    assert!(files.close(7).is_none());
    assert!(files.can_open_write(b"config/one-more"));
}
