//! The zip reader against ordinary and hostile archives. Every archive here is made by the
//! test: the `zip` crate's writer for honest ones, [`build`] (a hand-made zip32 writer that lets a test lie
//! in any field) and byte patches for what neither would produce.

use std::io::Write;

use torque_resource::zips::{
    MAX_DIRECTORY_BYTES, MAX_END_RECORDS, MAX_ENTRIES, MAX_ENTRY_BYTES, MAX_WASTED_INFLATE,
    MAX_ZIP_BYTES, MountError, Skip, ZipMount, entry_name_ok, served_name,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// A zip of `(name, contents)` entries, deflated.
fn zip_of(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, contents) in entries {
        // Raw names: the writer takes a `String`, so non-UTF-8 names are patched in afterwards.
        w.start_file(String::from_utf8_lossy(name).into_owned(), options)
            .unwrap();
        w.write_all(contents).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// Every offset of `sig` in `bytes`.
fn find_all(bytes: &[u8], sig: &[u8]) -> Vec<usize> {
    bytes
        .windows(sig.len())
        .enumerate()
        .filter(|(_, w)| *w == sig)
        .map(|(i, _)| i)
        .collect()
}

const LOCAL: &[u8] = b"PK\x03\x04";
const CENTRAL: &[u8] = b"PK\x01\x02";
const END: &[u8] = b"PK\x05\x06";

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// One entry of a hand-made archive; every header field a test may want to lie about is a field.
struct Raw {
    name: Vec<u8>,
    method: u16,
    flags: u16,
    crc: u32,
    /// What the headers say the stored (compressed) size and the uncompressed size are.
    comp_size: u32,
    size: u32,
    /// What is really there after the local header.
    data: Vec<u8>,
    /// The extra field, in the local and in the central header.
    extra: Vec<u8>,
    /// The entry's comment, in the central header.
    comment: Vec<u8>,
}

/// A stored entry whose headers tell the truth.
fn raw(name: &[u8], contents: &[u8]) -> Raw {
    Raw {
        name: name.to_vec(),
        method: 0,
        flags: 0,
        crc: crc32(contents),
        comp_size: contents.len() as u32,
        size: contents.len() as u32,
        data: contents.to_vec(),
        extra: Vec::new(),
        comment: Vec::new(),
    }
}

/// A zip32 archive: local headers and data, the central directory, the end record.
fn build(entries: &[Raw]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offsets = Vec::new();
    for e in entries {
        offsets.push(out.len() as u32);
        out.extend_from_slice(LOCAL);
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&e.flags.to_le_bytes());
        out.extend_from_slice(&e.method.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&e.crc.to_le_bytes());
        out.extend_from_slice(&e.comp_size.to_le_bytes());
        out.extend_from_slice(&e.size.to_le_bytes());
        out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&(e.extra.len() as u16).to_le_bytes());
        out.extend_from_slice(&e.name);
        out.extend_from_slice(&e.extra);
        out.extend_from_slice(&e.data);
    }
    let directory = out.len();
    for (e, offset) in entries.iter().zip(&offsets) {
        out.extend_from_slice(CENTRAL);
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&e.flags.to_le_bytes());
        out.extend_from_slice(&e.method.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&e.crc.to_le_bytes());
        out.extend_from_slice(&e.comp_size.to_le_bytes());
        out.extend_from_slice(&e.size.to_le_bytes());
        out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&(e.extra.len() as u16).to_le_bytes());
        out.extend_from_slice(&(e.comment.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&e.name);
        out.extend_from_slice(&e.extra);
        out.extend_from_slice(&e.comment);
    }
    let size = out.len() - directory;
    out.extend_from_slice(END);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&(directory as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A folder-named entry that declares a deflate stream (an empty one), where a stored
/// entry of no bytes is a directory record.
fn deflated_folder(name: &[u8]) -> Raw {
    Raw {
        method: 8,
        comp_size: 2,
        data: vec![3, 0],
        ..raw(name, b"")
    }
}

fn skips(mount: &ZipMount) -> Vec<Skip> {
    mount.skipped().iter().map(|(_, s)| *s).collect()
}

fn mounted_names(mount: &ZipMount) -> Vec<String> {
    mount
        .entries()
        .iter()
        .map(|e| String::from_utf8_lossy(&e.name).into_owned())
        .collect()
}

#[test]
fn the_caps_are_the_documented_ones() {
    // 64 MiB per archive, 64 MiB per entry, 65,536 entries, and the others the module docs of
    // `zips` give.
    assert_eq!(MAX_ZIP_BYTES, 64 << 20);
    assert_eq!(MAX_ENTRY_BYTES, 64 << 20);
    assert_eq!(MAX_ENTRIES, 65_536);
    assert_eq!(MAX_END_RECORDS, 8);
    assert_eq!(MAX_DIRECTORY_BYTES, 16 << 20);
    assert_eq!(MAX_WASTED_INFLATE, 256 << 20);
}

// ---------------------------------------------------------------- ordinary archives

#[test]
fn entries_mount_in_archive_order_and_read_ignoring_case() {
    let bytes = zip_of(&[
        (b"description.txt", b"Title: x"),
        (b"server.cs", b"echo(1);"),
        (b"sub/data.txt", b"line\r\n"),
    ]);
    let mount = ZipMount::open(bytes).unwrap();
    let names: Vec<&[u8]> = mount.entries().iter().map(|e| e.name.as_slice()).collect();
    assert_eq!(
        names,
        [&b"description.txt"[..], b"server.cs", b"sub/data.txt"]
    );
    assert_eq!(mount.read(b"SERVER.CS"), Some(b"echo(1);".to_vec()));
    assert_eq!(mount.read(b"Sub/Data.TXT"), Some(b"line\r\n".to_vec()));
    assert_eq!(mount.entry(b"sub/data.txt").unwrap().size, 6);
    assert_eq!(mount.read(b"missing.cs"), None);
    assert!(mount.skipped().is_empty());
    assert_eq!(mount.inflated(), 8 + 6);
}

#[test]
fn stored_and_deflated_entries_both_read() {
    let mount = ZipMount::open(build(&[raw(b"a.cs", b"stored"), raw(b"empty.txt", b"")])).unwrap();
    assert_eq!(mount.read(b"a.cs"), Some(b"stored".to_vec()));
    assert_eq!(mount.read(b"empty.txt"), Some(Vec::new()));
    let mount = ZipMount::open(zip_of(&[(b"d.cs", &b"x".repeat(5000))])).unwrap();
    assert_eq!(mount.read(b"d.cs"), Some(b"x".repeat(5000)));
}

#[test]
fn an_honest_read_holds_no_more_than_the_entry() {
    // A doubling vector would hold up to twice a 64 MiB entry.
    let contents = b"0123456789".repeat(300_000);
    let mount = ZipMount::open(zip_of(&[(b"big.txt", &contents)])).unwrap();
    let read = mount.read(b"big.txt").unwrap();
    assert_eq!(read, contents);
    assert!(read.capacity() <= contents.len() + 1, "{}", read.capacity());
    let mount = ZipMount::open(build(&[raw(b"s.txt", &contents)])).unwrap();
    let read = mount.read(b"s.txt").unwrap();
    assert!(read.capacity() <= contents.len() + 1, "{}", read.capacity());
}

#[test]
fn a_zip_with_junk_before_it_still_mounts() {
    let mut bytes = b"MZ not part of the archive".repeat(3);
    bytes.extend_from_slice(&zip_of(&[(b"ok.cs", b"ok")]));
    let mount = ZipMount::open(bytes).unwrap();
    assert_eq!(mount.read(b"ok.cs"), Some(b"ok".to_vec()));
}

// ---------------------------------------------------------------- names

#[test]
fn names_a_host_may_not_serve_are_left_out() {
    for bad in [
        &b"../evil.cs"[..],
        b"a/../../b.cs",
        b"/abs.cs",
        b"C:/x.cs",
        b"a\\b.cs",
        b"con.cs",
        b"mods/NUL",
        b"a/./b.cs",
        b"a//b.cs",
        b"trailing.",
        b"trailing ",
        b"x\x01y.cs",
        b"nul\0byte.cs",
        b"q?.cs",
        b"",
    ] {
        assert!(!entry_name_ok(bad), "{:?}", String::from_utf8_lossy(bad));
    }
    assert!(entry_name_ok(b"scripts/door house.cs"));
    assert!(entry_name_ok("caf\u{e9}/\u{3b1}.cs".as_bytes()));
    let bytes = zip_of(&[
        (b"../evil.cs", b"x"),
        (b"ok.cs", b"ok"),
        (b"con.cs", b"x"),
        (b"dir/", b""),
    ]);
    let mount = ZipMount::open(bytes).unwrap();
    assert_eq!(mounted_names(&mount), ["ok.cs", "dir/"]);
    assert_eq!(skips(&mount), [Skip::BadName, Skip::BadName]);
    assert_eq!(mount.read(b"../evil.cs"), None);
    assert!(mount.entry(b"../evil.cs").is_none());
}

#[test]
fn every_refused_name_is_left_out_of_a_real_archive() {
    let bad: Vec<&[u8]> = vec![
        b"../evil.cs",
        b"a/../b.cs",
        b"..",
        b".",
        b"/abs.cs",
        b"C:/x.cs",
        b"C:x.cs",
        b"..\\b.cs",
        b"con",
        b"CON.txt",
        b"dir/AUX.cs",
        b"com1.cs",
        b"lpt9",
        b"a/./b.cs",
        b"a//b.cs",
        b"trailing.",
        b"trailing ",
        b"ctl\x01.cs",
        b"del\x7f.cs",
        b"nul\0byte.cs",
        b"star*.cs",
        b"q?.cs",
        b"quote\".cs",
        b"lt<.cs",
        b"pipe|.cs",
        b"\xe9\x01.cs",
    ];
    let mut entries: Vec<Raw> = bad.iter().map(|name| raw(name, b"x")).collect();
    entries.push(raw(b"ok.cs", b"ok"));
    let mount = ZipMount::open(build(&entries)).unwrap();
    assert_eq!(mounted_names(&mount), ["ok.cs"]);
    assert_eq!(skips(&mount), vec![Skip::BadName; bad.len()]);
    for name in bad {
        assert!(mount.entry(name).is_none());
        assert_eq!(mount.read(name), None);
    }
}

#[test]
fn a_name_that_is_not_utf8_mounts_under_its_raw_bytes() {
    // A CP437 name's high byte is kept as it is and the entry is served. The VM's paths are
    // byte strings, and a zip entry is served from memory, so nothing needs the name to be UTF-8.
    let mut bytes = zip_of(&[(b"aXb.cs", b"x"), (b"ok.cs", b"ok")]);
    // Patch the X of the first name, in the local and the central header, to 0xFF.
    for at in find_all(&bytes, b"aXb.cs") {
        bytes[at + 1] = 0xff;
    }
    let mount = ZipMount::open(bytes).unwrap();
    assert!(mount.skipped().is_empty());
    assert_eq!(mount.entries().len(), 2);
    assert_eq!(mount.entries()[0].name, b"a\xffb.cs");
    assert_eq!(mount.read(b"a\xffb.cs"), Some(b"x".to_vec()));
    assert_eq!(
        mount.read(b"A\xffB.CS"),
        Some(b"x".to_vec()),
        "ASCII case still folds"
    );
    // The shape: `hi<0xE9>name.txt`, and a high byte in a folder name.
    let mount = ZipMount::open(build(&[
        raw(b"hi\xe9name.txt", b"cp437"),
        raw(b"d\xc4r/f\x80.txt", b"deep"),
        raw("caf\u{e9}/\u{3b1}.cs".as_bytes(), b"utf8"),
    ]))
    .unwrap();
    assert!(mount.skipped().is_empty());
    assert_eq!(mount.read(b"hi\xe9name.txt"), Some(b"cp437".to_vec()));
    assert_eq!(mount.read(b"d\xc4r/f\x80.txt"), Some(b"deep".to_vec()));
    assert_eq!(
        mount.read("caf\u{e9}/\u{3b1}.cs".as_bytes()),
        Some(b"utf8".to_vec())
    );
    assert!(entry_name_ok(b"\xff\xfe.cs"));
    // A control byte or a NUL next to a high byte is still refused.
    assert!(!entry_name_ok(b"\xe9\x01.cs"));
    assert!(!entry_name_ok(b"\xe9\0.cs"));
}

#[test]
fn backslashes_become_slashes_and_the_entry_mounts() {
    // `bs\slash.txt` is mounted as `bs/slash.txt` and read.
    let mount = ZipMount::open(build(&[
        raw(b"bs\\slash.txt", b"BACKSLASH_DATA"),
        raw(b"deep\\er\\f.cs", b"deeper"),
        raw(b"mixed/in\\one.cs", b"mixed"),
        raw(b"ok.cs", b"ok"),
    ]))
    .unwrap();
    assert!(mount.skipped().is_empty());
    assert_eq!(
        mounted_names(&mount),
        ["bs/slash.txt", "deep/er/f.cs", "mixed/in/one.cs", "ok.cs"]
    );
    assert_eq!(
        mount.read(b"bs/slash.txt"),
        Some(b"BACKSLASH_DATA".to_vec())
    );
    assert_eq!(
        mount.read(b"BS/Slash.TXT"),
        Some(b"BACKSLASH_DATA".to_vec())
    );
    assert_eq!(mount.entry(b"deep/er/f.cs").unwrap().size, 6);
    assert_eq!(mount.read(b"mixed/in/one.cs"), Some(b"mixed".to_vec()));
    // The same through the deflating writer (the name is written as given).
    let mount = ZipMount::open(zip_of(&[(b"w\\x.cs", &b"x".repeat(500))])).unwrap();
    assert_eq!(mount.read(b"w/x.cs"), Some(b"x".repeat(500)));
    assert_eq!(served_name(b"a\\b/c\\d"), Some(b"a/b/c/d".to_vec()));
    assert_eq!(
        served_name(b"plain/name.cs"),
        Some(b"plain/name.cs".to_vec())
    );
}

#[test]
fn a_name_that_is_unsafe_once_its_backslashes_are_slashes_is_still_refused() {
    let bad: Vec<&[u8]> = vec![
        b"..\\x.cs",
        b"a\\..\\b.cs",
        b"a\\..",
        b"C:\\x.cs",
        b"C:x.cs",
        b"\\abs.cs",
        b"a\\\\b.cs",
        b"a\\/b.cs",
        b"a/\\b.cs",
        b"con\\x.txt",
        b"dir\\NUL",
        b"dir\\aux.cs",
        b"a\\.\\b.cs",
        b"trailing.\\x.cs",
        b"x\\trailing ",
        b"ctl\\\x01.cs",
        b"a\\q?.cs",
    ];
    for name in &bad {
        assert_eq!(
            served_name(name),
            None,
            "{:?}",
            String::from_utf8_lossy(name)
        );
    }
    let mut entries: Vec<Raw> = bad.iter().map(|name| raw(name, b"x")).collect();
    entries.push(raw(b"ok.cs", b"ok"));
    let mount = ZipMount::open(build(&entries)).unwrap();
    assert_eq!(mounted_names(&mount), ["ok.cs"]);
    assert_eq!(skips(&mount), vec![Skip::BadName; bad.len()]);
    // The skip list keeps the name as the archive spells it.
    assert_eq!(mount.skipped()[0].0, b"..\\x.cs");
}

#[test]
fn a_converted_name_that_collides_with_another_entry_follows_the_duplicate_rule() {
    // Later data wins, in the first place and under the first spelling.
    let mount = ZipMount::open(build(&[
        raw(b"a\\b.txt", b"first"),
        raw(b"other.cs", b"o"),
        raw(b"a/b.txt", b"second"),
    ]))
    .unwrap();
    assert_eq!(mounted_names(&mount), ["a/b.txt", "other.cs"]);
    assert_eq!(mount.read(b"a/b.txt"), Some(b"second".to_vec()));
    assert_eq!(
        mount.skipped(),
        [(b"a/b.txt".to_vec(), Skip::Duplicate)].as_slice()
    );
    // The other way round, and with case as well.
    let mount = ZipMount::open(build(&[
        raw(b"A/B.txt", b"first"),
        raw(b"a\\b.TXT", b"second"),
    ]))
    .unwrap();
    assert_eq!(mounted_names(&mount), ["A/B.txt"]);
    assert_eq!(mount.read(b"a/b.txt"), Some(b"second".to_vec()));
    // Two spellings that both convert: one entry.
    let mount = ZipMount::open(build(&[raw(b"x\\y", b"1"), raw(b"x\\y", b"2")])).unwrap();
    assert_eq!(mounted_names(&mount), ["x/y"]);
    assert_eq!(mount.read(b"x/y"), Some(b"2".to_vec()));
}

#[test]
fn folder_entries_are_zero_byte_files_that_keep_their_slash() {
    // A zip's folder entries are resources: `isFile("<folder>/X/d/")` is 1 (without the
    // slash 0), their length 0, `openForRead` 1 with `isEOF` 1, and the searches list them among the files.
    // Whatever method the header declares, an entry that declares no bytes is empty and is never inflated (one that
    // declares data is a file: `a_folder_named_entry_that_declares_data_is_a_file_with_its_data`).
    let odd_folder = Raw {
        method: 12,
        flags: 1,
        ..raw(b"odd/", b"")
    };
    let mount = ZipMount::open(build(&[
        deflated_folder(b"d/"),
        raw(b"d/f.txt", b"f"),
        deflated_folder(b"e/"),
        deflated_folder(b"back\\"),
        odd_folder,
        raw(b"ok.cs", b"ok"),
    ]))
    .unwrap();
    assert_eq!(
        mounted_names(&mount),
        ["d/", "d/f.txt", "e/", "back/", "odd/", "ok.cs"]
    );
    assert!(mount.skipped().is_empty(), "{:?}", mount.skipped());
    for name in [&b"d/"[..], b"E/", b"back/", b"odd/"] {
        let entry = mount.entry(name).unwrap();
        assert!(entry.folder, "{}", String::from_utf8_lossy(name));
        assert_eq!(entry.size, 0);
        assert_eq!(mount.read(name), Some(Vec::new()));
    }
    assert!(!mount.entry(b"d/f.txt").unwrap().folder);
    assert!(!mount.entry(b"ok.cs").unwrap().folder);
    // The folder without its slash is not a file; and a folder entry is nothing to inflate.
    assert!(mount.entry(b"d").is_none());
    assert_eq!(mount.read(b"d"), None);
    assert_eq!(mount.inflated(), 0);
    assert_eq!(mount.wasted(), 0);
}

#[test]
fn a_folder_named_entry_that_declares_data_is_a_file_with_its_data() {
    // `e/`, a deflated entry of five bytes with a name that ends in a slash, is a file:
    // `isFile` 1, length 5, its CRC, `openForRead` 1 and the line read back. Stored or deflated, it keeps its slash and
    // is read like any entry (its CRC and length are checked); the same name without data stays a resource of no bytes.
    let deflated = Raw {
        method: 8,
        comp_size: 7,
        data: vec![0x4b, 0x49, 0x2c, 0x49, 0xe4, 0x02, 0x00],
        size: 5,
        crc: crc32(
            b"data
",
        ),
        ..raw(
            b"e/", b"data
",
        )
    };
    let mount = ZipMount::open(build(&[
        deflated,
        raw(
            b"s/", b"stored
",
        ),
        deflated_folder(b"empty/"),
    ]))
    .unwrap();
    assert_eq!(mounted_names(&mount), ["e/", "s/", "empty/"]);
    assert!(mount.skipped().is_empty(), "{:?}", mount.skipped());
    for (name, bytes) in [
        (
            &b"e/"[..],
            &b"data
"[..],
        ),
        (
            b"S/", b"stored
",
        ),
    ] {
        let entry = mount.entry(name).unwrap();
        assert!(!entry.folder, "{}", String::from_utf8_lossy(name));
        assert_eq!(entry.size, bytes.len() as u64);
        assert_eq!(mount.read(name), Some(bytes.to_vec()));
    }
    assert!(mount.entry(b"empty/").unwrap().folder);
    assert_eq!(mount.read(b"empty/"), Some(Vec::new()));
    // Without the slash it is still not a name of the archive.
    assert!(mount.entry(b"e").is_none());
    // A lying header does not make it readable: the data is checked as for any entry.
    let liar = Raw {
        crc: 1,
        ..raw(
            b"liar/", b"data
",
        )
    };
    let mount = ZipMount::open(build(&[liar])).unwrap();
    assert_eq!(mount.read(b"liar/"), None);
}

#[test]
fn a_stored_folder_entry_of_no_bytes_is_a_directory_record_and_no_resource() {
    // Zip tools hold their folders as stored entries of no bytes; none of them is listed and `isFile("<zip>/d/")`
    // is 0.
    let mount = ZipMount::open(build(&[
        raw(b"d/", b""),
        raw(b"d/f.txt", b"f"),
        raw(b"e/", b""),
        raw(b"back\\", b""),
        deflated_folder(b"kept/"),
    ]))
    .unwrap();
    assert_eq!(mounted_names(&mount), ["d/f.txt", "kept/"]);
    assert_eq!(skips(&mount), [Skip::Directory; 3]);
    assert!(mount.entry(b"d/").is_none());
    assert!(mount.entry(b"kept/").is_some_and(|e| e.folder));
}

#[test]
fn folder_names_a_host_may_not_serve_are_left_out() {
    // The name before the slash is a name like any other: `..`, a device, a drive, an empty component.
    let bad: Vec<&[u8]> = vec![
        b"../", b"./", b"//", b"a//", b"//a/", b"con/", b"a/nul/", b"x./", b"C:/", b"q?/", b"\\",
        b"..\\", b"a\\\\", b"/abs/",
    ];
    let mut entries: Vec<Raw> = bad.iter().map(|name| raw(name, b"")).collect();
    entries.push(deflated_folder(b"fine/"));
    let mount = ZipMount::open(build(&entries)).unwrap();
    assert_eq!(mounted_names(&mount), ["fine/"]);
    assert_eq!(skips(&mount), vec![Skip::BadName; bad.len()]);
}

#[test]
fn the_last_of_interleaved_case_twins_wins_across_spellings() {
    // Entries T.TXT, t.txt, t.txt, T.TXT (data T1..T4): ONE resource, listed under the first
    // spelling, reading T4: the last entry in central-directory order, whichever spelling it has. (The `zip`
    // crate merges the identical raw names first, so the winner is told from where each header stands.)
    let mount = ZipMount::open(build(&[
        raw(b"T.TXT", b"T1"),
        raw(b"t.txt", b"T2"),
        raw(b"t.txt", b"T3"),
        raw(b"T.TXT", b"T4"),
    ]))
    .unwrap();
    assert_eq!(mounted_names(&mount), ["T.TXT"]);
    assert_eq!(mount.read(b"t.txt"), Some(b"T4".to_vec()));
    assert_eq!(mount.read(b"T.TXT"), Some(b"T4".to_vec()));
    // The mirror: the last of them has the other spelling.
    let mount = ZipMount::open(build(&[
        raw(b"T.TXT", b"T1"),
        raw(b"t.txt", b"T2"),
        raw(b"T.TXT", b"T3"),
        raw(b"t.txt", b"T4"),
    ]))
    .unwrap();
    assert_eq!(mounted_names(&mount), ["T.TXT"]);
    assert_eq!(mount.read(b"T.TXT"), Some(b"T4".to_vec()));
    // The spelling that comes second wins when its last entry does.
    let mount = ZipMount::open(build(&[
        raw(b"b.txt", b"b1"),
        raw(b"B.txt", b"b2"),
        raw(b"b.txt", b"b3"),
        raw(b"other.txt", b"o"),
    ]))
    .unwrap();
    assert_eq!(mounted_names(&mount), ["b.txt", "other.txt"]);
    assert_eq!(mount.read(b"b.txt"), Some(b"b3".to_vec()));
    let mount = ZipMount::open(build(&[
        raw(b"c.txt", b"c1"),
        raw(b"C.txt", b"c2"),
        raw(b"c.txt", b"c3"),
        raw(b"C.txt", b"c4"),
    ]))
    .unwrap();
    assert_eq!(mount.read(b"c.txt"), Some(b"c4".to_vec()));
    assert_eq!(mounted_names(&mount), ["c.txt"]);
    // The loser is reported under the name it was written with (the crate merged the identical raw names).
    assert_eq!(
        mount.skipped(),
        [(b"c.txt".to_vec(), Skip::Duplicate)].as_slice()
    );
    // Three spellings: `Ab` is merged to its last header (4), which beats `ab` (1) and so must also beat `aB`
    // (3) that comes later in the crate's order: the header that won is the one to compare with.
    let mount = ZipMount::open(build(&[
        raw(b"ab", b"D1"),
        raw(b"Ab", b"D2"),
        raw(b"aB", b"D3"),
        raw(b"Ab", b"D4"),
    ]))
    .unwrap();
    assert_eq!(mounted_names(&mount), ["ab"]);
    assert_eq!(mount.read(b"AB"), Some(b"D4".to_vec()));
    assert_eq!(skips(&mount), [Skip::Duplicate, Skip::Duplicate]);
}

// ---------------------------------------------------------------- duplicates

#[test]
fn names_that_differ_in_case_mount_as_one_the_later_data_in_the_earlier_place() {
    let bytes = zip_of(&[
        (b"A.cs", b"first"),
        (b"other.cs", b"o"),
        (b"a.CS", b"second!"),
    ]);
    let mount = ZipMount::open(bytes).unwrap();
    assert_eq!(mounted_names(&mount), ["A.cs", "other.cs"]);
    assert_eq!(mount.read(b"a.cs"), Some(b"second!".to_vec()));
    assert_eq!(mount.entry(b"A.CS").unwrap().size, 7);
    assert_eq!(
        mount.skipped(),
        [(b"A.cs".to_vec(), Skip::Duplicate)].as_slice()
    );
}

#[test]
fn identical_names_mount_as_one_the_later_data_in_the_earlier_place() {
    // The writer refuses two entries of one name, so rename the second after writing.
    let mut bytes = zip_of(&[(b"a.cs", b"one"), (b"b.cs", b"two"), (b"c.cs", b"three")]);
    for at in find_all(&bytes, b"b.cs") {
        bytes[at] = b'a';
    }
    let mount = ZipMount::open(bytes).unwrap();
    assert_eq!(mounted_names(&mount), ["a.cs", "c.cs"]);
    assert_eq!(mount.read(b"a.cs"), Some(b"two".to_vec()));
}

#[test]
fn a_later_entry_that_cannot_be_served_does_not_replace_an_earlier_one() {
    let mut odd = raw(b"A.CS", b"zzz");
    odd.method = 12;
    let mount = ZipMount::open(build(&[raw(b"a.cs", b"good"), odd])).unwrap();
    assert_eq!(mount.read(b"a.cs"), Some(b"good".to_vec()));
    assert_eq!(skips(&mount), [Skip::Unsupported]);
}

// ---------------------------------------------------------------- sizes, CRCs, methods

#[test]
fn a_lying_size_or_a_bad_crc_makes_the_entry_unreadable() {
    let contents = b"0123456789".repeat(100);
    let honest = zip_of(&[(b"x.cs", &contents)]);
    // Declared uncompressed size 10 (central header +24, local header +22) for 1000 bytes of data.
    let mut small = honest.clone();
    for at in find_all(&small, CENTRAL) {
        small[at + 24..at + 28].copy_from_slice(&10u32.to_le_bytes());
    }
    for at in find_all(&small, LOCAL) {
        small[at + 22..at + 26].copy_from_slice(&10u32.to_le_bytes());
    }
    let mount = ZipMount::open(small).unwrap();
    assert_eq!(mount.entry(b"x.cs").unwrap().size, 10);
    assert_eq!(mount.read(b"x.cs"), None);
    // A CRC that does not match (central +16, local +14).
    let mut crc = honest;
    for at in find_all(&crc, CENTRAL) {
        crc[at + 16] ^= 0xff;
    }
    for at in find_all(&crc, LOCAL) {
        crc[at + 14] ^= 0xff;
    }
    let mount = ZipMount::open(crc).unwrap();
    assert_eq!(mount.read(b"x.cs"), None);
}

#[test]
fn a_size_that_is_not_what_the_data_inflates_to_is_refused_in_both_directions() {
    let contents = b"abcdefghij".repeat(10);
    // Stored: the declared size is smaller / larger than the stored bytes.
    for declared in [0u32, 99, 101, 5000] {
        let mut e = raw(b"s.cs", &contents);
        e.size = declared;
        let mount = ZipMount::open(build(&[e])).unwrap();
        assert_eq!(mount.read(b"s.cs"), None, "stored, declared {declared}");
    }
    // Deflated: the same with the writer's own data.
    for declared in [0u32, 99, 101, 5000] {
        let mut bytes = zip_of(&[(b"d.cs", &contents)]);
        for at in find_all(&bytes, CENTRAL) {
            bytes[at + 24..at + 28].copy_from_slice(&declared.to_le_bytes());
        }
        let mount = ZipMount::open(bytes).unwrap();
        assert_eq!(mount.read(b"d.cs"), None, "deflated, declared {declared}");
    }
    // The truth still reads.
    let mount = ZipMount::open(build(&[raw(b"s.cs", &contents)])).unwrap();
    assert_eq!(mount.read(b"s.cs"), Some(contents));
}

#[test]
fn a_stored_entry_longer_than_its_declared_size_is_refused_at_one_byte_over() {
    // data.len() == size + 1: the read stops one byte past the declared size and refuses.
    let mut e = raw(b"s.cs", b"0123456789");
    e.size = 9;
    let mount = ZipMount::open(build(&[e])).unwrap();
    assert_eq!(mount.read(b"s.cs"), None);
    assert_eq!(mount.inflated(), 10);
}

#[test]
fn a_bad_crc_is_refused_for_stored_and_for_empty_entries() {
    let mut stored = raw(b"s.cs", b"payload");
    stored.crc ^= 1;
    let mut empty = raw(b"e.cs", b"");
    empty.crc = 1;
    let mount = ZipMount::open(build(&[stored, empty])).unwrap();
    assert_eq!(mount.read(b"s.cs"), None);
    assert_eq!(mount.read(b"e.cs"), None);
}

#[test]
fn flagged_unsupported_and_oversized_entries() {
    let mut bytes = zip_of(&[
        (b"enc.cs", b"x"),
        (b"odd.cs", b"y"),
        (b"big.cs", b"z"),
        (b"ok.cs", b"ok"),
    ]);
    let centrals = find_all(&bytes, CENTRAL);
    // enc.cs: the encryption flag (general purpose bit 0, central +8) is ignored.
    bytes[centrals[0] + 8] |= 1;
    // odd.cs: compression method 12 (bzip2, not built in; central +10).
    bytes[centrals[1] + 10..centrals[1] + 12].copy_from_slice(&12u16.to_le_bytes());
    // big.cs: declares MAX_ENTRY_BYTES + 1 bytes.
    let big = u32::try_from(MAX_ENTRY_BYTES + 1).unwrap();
    bytes[centrals[2] + 24..centrals[2] + 28].copy_from_slice(&big.to_le_bytes());
    let mount = ZipMount::open(bytes).unwrap();
    assert_eq!(skips(&mount), [Skip::Unsupported, Skip::TooLarge]);
    assert_eq!(mounted_names(&mount), ["enc.cs", "ok.cs"]);
    assert_eq!(mount.read(b"enc.cs"), Some(b"x".to_vec()));
    assert_eq!(mount.read(b"ok.cs"), Some(b"ok".to_vec()));
}

#[test]
fn every_method_but_stored_and_deflate_is_unsupported() {
    // 1 shrink, 6 implode, 8 is deflate, 9 deflate64, 12 bzip2, 14 lzma, 93 zstd, 95 xz, 98 ppmd.
    let methods = [1u16, 6, 9, 12, 14, 93, 95, 98, 0x1234];
    let entries: Vec<Raw> = methods
        .iter()
        .enumerate()
        .map(|(i, &method)| Raw {
            method,
            ..raw(format!("m{i}.cs").as_bytes(), b"x")
        })
        .collect();
    let mount = ZipMount::open(build(&entries)).unwrap();
    assert_eq!(mount.entries().len(), 0);
    assert_eq!(skips(&mount), vec![Skip::Unsupported; methods.len()]);
}

/// The AE-x extra field (id 0x9901, size 7, version 2, vendor "AE", strength 3, the real method `inner`).
fn aes_extra(inner: u16) -> Vec<u8> {
    let mut extra = vec![0x01, 0x99, 7, 0, 2, 0, b'A', b'E', 3];
    extra.extend_from_slice(&inner.to_le_bytes());
    extra
}

#[test]
fn a_method_99_entry_without_its_aes_field_drops_only_itself() {
    // The reader drops just the entry and the archive loads (as for every method it cannot read).
    let mut aes = raw(b"aes.cs", b"x");
    aes.method = 99;
    aes.flags = 1;
    let mount =
        ZipMount::open(build(&[raw(b"server.cs", b"s"), aes, raw(b"ok.cs", b"ok")])).unwrap();
    assert_eq!(mounted_names(&mount), ["server.cs", "ok.cs"]);
    assert_eq!(
        mount.skipped(),
        [(b"aes.cs".to_vec(), Skip::Unsupported)].as_slice()
    );
    assert_eq!(mount.read(b"server.cs"), Some(b"s".to_vec()));
    assert_eq!(mount.read(b"ok.cs"), Some(b"ok".to_vec()));
    assert!(mount.entry(b"aes.cs").is_none());
}

#[test]
fn an_entry_with_a_real_aes_field_drops_only_itself() {
    // Method 99 plus the AE-x extra field: the crate would hand it to its reader as the method inside the
    // field and then ask for a password. The reader skips the entry, as it skips every method but 0 and 8.
    for inner in [0u16, 8] {
        let aes = Raw {
            method: 99,
            flags: 1,
            extra: aes_extra(inner),
            ..raw(b"aes.cs", b"ciphertext")
        };
        let mount = ZipMount::open(build(&[raw(b"a.cs", b"a"), aes, raw(b"z.cs", b"z")])).unwrap();
        assert_eq!(mounted_names(&mount), ["a.cs", "z.cs"], "inner {inner}");
        assert_eq!(skips(&mount), [Skip::Unsupported], "inner {inner}");
        assert_eq!(mount.read(b"z.cs"), Some(b"z".to_vec()));
    }
}

#[test]
fn method_99_entries_are_dropped_wherever_they_sit_in_the_directory_and_the_file() {
    let bad = |name: &str| Raw {
        method: 99,
        ..raw(name.as_bytes(), b"x")
    };
    // First, middle, last, and several in a row.
    let entries = [
        bad("first"),
        raw(b"a.cs", b"a"),
        bad("second"),
        bad("third"),
        raw(b"b.cs", b"b"),
        bad("last"),
    ];
    let mount = ZipMount::open(build(&entries)).unwrap();
    assert_eq!(mounted_names(&mount), ["a.cs", "b.cs"]);
    assert_eq!(skips(&mount), vec![Skip::Unsupported; 4]);
    // Behind junk (the central header offsets are then shifted).
    let mut prefixed = b"junk before the archive ".repeat(5);
    prefixed.extend_from_slice(&build(&entries));
    let mount = ZipMount::open(prefixed).unwrap();
    assert_eq!(mounted_names(&mount), ["a.cs", "b.cs"]);
    // A digital signature record (`PK\5\5`) between the last header and the end record.
    let mut signed = build(&entries);
    let end = *find_all(&signed, END).last().unwrap();
    signed.splice(end..end, b"PK\x05\x05\0\0".iter().copied());
    let mount = ZipMount::open(signed).unwrap();
    assert_eq!(mounted_names(&mount), ["a.cs", "b.cs"]);
    assert_eq!(skips(&mount), vec![Skip::Unsupported; 4]);
    // The last central record of a zip64 archive is followed by the zip64 end record, not by an
    // ordinary header: patch that record's method to 99.
    let mut bytes = many_entries(MAX_ENTRIES);
    let last = *find_all(&bytes, CENTRAL).last().unwrap();
    bytes[last + 10..last + 12].copy_from_slice(&99u16.to_le_bytes());
    let mount = ZipMount::open(bytes).unwrap();
    assert_eq!(mount.entries().len(), MAX_ENTRIES - 1);
    assert_eq!(skips(&mount), [Skip::Unsupported]);
}

#[test]
fn a_method_99_header_with_its_own_extra_field_and_comment_is_still_found_in_the_chain() {
    // The next header starts after this one's name, extra field and comment: all three count.
    let unknown_extra = vec![0xaa, 0xaa, 2, 0, 1, 2];
    for (extra, comment) in [
        (unknown_extra.clone(), Vec::new()),
        (Vec::new(), b"a comment".to_vec()),
        (unknown_extra, b"and an extra field too".to_vec()),
    ] {
        let bad = Raw {
            method: 99,
            extra,
            comment,
            ..raw(b"aes.cs", b"x")
        };
        let mount = ZipMount::open(build(&[raw(b"a.cs", b"a"), bad, raw(b"b.cs", b"b")])).unwrap();
        assert_eq!(mounted_names(&mount), ["a.cs", "b.cs"]);
        assert_eq!(skips(&mount), [Skip::Unsupported]);
    }
}

#[test]
fn bytes_that_only_look_like_a_method_99_header_are_left_alone() {
    // A stored file whose data holds a central-header signature with method 99 in the right place, but no
    // directory around it, must come back byte for byte.
    let mut lookalike = CENTRAL.to_vec();
    lookalike.extend_from_slice(&[20, 0, 20, 0, 0, 0, 99, 0]);
    lookalike.extend_from_slice(&[0; 48]);
    let mount =
        ZipMount::open(build(&[raw(b"data.bin", &lookalike), raw(b"ok.cs", b"ok")])).unwrap();
    assert_eq!(mount.read(b"data.bin"), Some(lookalike));
    assert!(mount.skipped().is_empty());
}

#[test]
fn the_encryption_flag_is_ignored_and_the_entry_is_served() {
    // General purpose bit 0 is never looked at; the entry reads as its data.
    let mut stored = raw(b"s.cs", b"plain stored");
    stored.flags = 1;
    let mut deflated = zip_of(&[(b"d.cs", &b"plain deflated ".repeat(40))]);
    for at in find_all(&deflated, CENTRAL) {
        deflated[at + 8] |= 1;
    }
    for at in find_all(&deflated, LOCAL) {
        deflated[at + 6] |= 1;
    }
    let mount = ZipMount::open(build(&[stored])).unwrap();
    assert!(mount.skipped().is_empty());
    assert_eq!(mount.read(b"s.cs"), Some(b"plain stored".to_vec()));
    let mount = ZipMount::open(deflated).unwrap();
    assert!(mount.skipped().is_empty());
    assert_eq!(mount.read(b"d.cs"), Some(b"plain deflated ".repeat(40)));
    // Data that really is encrypted does not pass the CRC: the entry is listed and reads as missing.
    let mut garbage = raw(b"g.cs", b"0123456789ab\x9f\x01\x7f\x80 not the plaintext");
    garbage.flags = 1;
    garbage.crc = crc32(b"the plaintext");
    garbage.size = 13;
    let mount = ZipMount::open(build(&[garbage, raw(b"ok.cs", b"ok")])).unwrap();
    assert_eq!(mount.entry(b"g.cs").unwrap().size, 13);
    assert_eq!(mount.read(b"g.cs"), None);
    assert_eq!(mount.read(b"ok.cs"), Some(b"ok".to_vec()));
}

#[test]
fn the_entry_size_cap_is_exact_and_a_zip64_size_cannot_dodge_it() {
    // Exactly MAX_ENTRY_BYTES declared: mounted (and unreadable here, there is no such data).
    let mut at_cap = raw(b"cap.cs", b"x");
    at_cap.size = u32::try_from(MAX_ENTRY_BYTES).unwrap();
    let mut over = raw(b"over.cs", b"x");
    over.size = u32::try_from(MAX_ENTRY_BYTES + 1).unwrap();
    let mut huge = raw(b"huge.cs", b"x");
    huge.size = u32::MAX;
    let mount = ZipMount::open(build(&[at_cap, over, huge])).unwrap();
    assert_eq!(mounted_names(&mount), ["cap.cs"]);
    assert_eq!(mount.entry(b"cap.cs").unwrap().size, MAX_ENTRY_BYTES);
    assert_eq!(skips(&mount), [Skip::TooLarge, Skip::TooLarge]);
    assert_eq!(mount.read(b"cap.cs"), None);

    // A zip64 extra field in the central header can declare more than 4 GiB: made by the writer
    // (`large_file`), then the 64-bit uncompressed size is patched to 5 GiB.
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true);
    w.start_file("z64.cs", options).unwrap();
    w.write_all(b"data").unwrap();
    w.start_file("ok.cs", SimpleFileOptions::default()).unwrap();
    w.write_all(b"ok").unwrap();
    let mut bytes = w.finish().unwrap().into_inner();
    let central = find_all(&bytes, CENTRAL)[0];
    let name_len = usize::from(u16::from_le_bytes([
        bytes[central + 28],
        bytes[central + 29],
    ]));
    // The extra field follows the name: id 0x0001, length, then the uncompressed size as a u64.
    let extra = central + 46 + name_len;
    assert_eq!(&bytes[extra..extra + 2], &[1, 0], "a zip64 extra field");
    bytes[extra + 4..extra + 12].copy_from_slice(&(5u64 << 30).to_le_bytes());
    let mount = ZipMount::open(bytes).unwrap();
    assert_eq!(mounted_names(&mount), ["ok.cs"]);
    assert_eq!(skips(&mount), [Skip::TooLarge]);
}

#[test]
fn a_local_header_that_is_not_where_the_directory_says_leaves_the_entry_unreadable() {
    let mut bytes = build(&[raw(b"gone.cs", b"x"), raw(b"ok.cs", b"ok")]);
    // Break the first local header's signature.
    let first = find_all(&bytes, LOCAL)[0];
    bytes[first + 3] = 0;
    let mount = ZipMount::open(bytes).unwrap();
    assert_eq!(mounted_names(&mount), ["ok.cs"]);
    assert_eq!(
        mount.skipped(),
        [(b"gone.cs".to_vec(), Skip::Unreadable)].as_slice()
    );
}

// ---------------------------------------------------------------- the read bound

#[test]
fn a_zip_bomb_inflates_no_more_than_its_declared_size() {
    // 16 MiB of zeros deflate to about 16 KiB; declared 1 KiB, the read stops at 1 KiB + 1 and refuses.
    let zeros = vec![0u8; 16 << 20];
    let mut bytes = zip_of(&[(b"bomb.bin", &zeros)]);
    for at in find_all(&bytes, CENTRAL) {
        bytes[at + 24..at + 28].copy_from_slice(&1024u32.to_le_bytes());
    }
    let mount = ZipMount::open(bytes).unwrap();
    assert_eq!(mount.read(b"bomb.bin"), None);
    assert_eq!(mount.inflated(), 1025);
}

#[test]
fn an_entry_that_failed_is_not_inflated_again() {
    let zeros = vec![0u8; 1 << 20];
    let mut bytes = zip_of(&[(b"bomb.bin", &zeros), (b"ok.cs", b"ok")]);
    for at in find_all(&bytes, CENTRAL).into_iter().take(1) {
        bytes[at + 24..at + 28].copy_from_slice(&1000u32.to_le_bytes());
    }
    let mount = ZipMount::open(bytes).unwrap();
    for _ in 0..10 {
        assert_eq!(mount.read(b"bomb.bin"), None);
    }
    assert_eq!(mount.inflated(), 1001, "one inflation, not ten");
    // Its neighbour is unharmed, and the failed entry still exists for a stat.
    assert_eq!(mount.read(b"ok.cs"), Some(b"ok".to_vec()));
    assert_eq!(mount.entry(b"bomb.bin").unwrap().size, 1000);
}

#[test]
fn failed_reads_may_inflate_only_so_much_in_all() {
    // 64 MiB of zeros deflated once; eight entries share that stream, each declaring 64 MiB with a wrong CRC.
    // Every read inflates the whole stream before the CRC refuses it. Four of them spend the budget.
    let zeros = vec![0u8; usize::try_from(MAX_ENTRY_BYTES).unwrap()];
    let made = zip_of(&[(b"z.bin", &zeros)]);
    let central = find_all(&made, CENTRAL)[0];
    let field =
        |at: usize| u32::from_le_bytes(made[central + at..central + at + 4].try_into().unwrap());
    let (comp_size, data_start) = (field(20), 30 + "z.bin".len());
    let stream = made[data_start..data_start + comp_size as usize].to_vec();
    let entries: Vec<Raw> = (0..8)
        .map(|i| Raw {
            method: 8,
            crc: field(16) ^ 1,
            comp_size,
            size: u32::try_from(MAX_ENTRY_BYTES).unwrap(),
            data: stream.clone(),
            ..raw(format!("e{i}.bin").as_bytes(), b"")
        })
        .chain([raw(b"honest.txt", b"fine")])
        .collect();
    let mount = ZipMount::open(build(&entries)).unwrap();
    assert_eq!(mount.read(b"honest.txt"), Some(b"fine".to_vec()));
    let inflated_by_honest = mount.inflated();
    for i in 0..8 {
        assert_eq!(mount.read(format!("e{i}.bin").as_bytes()), None);
    }
    assert_eq!(
        mount.inflated() - inflated_by_honest,
        MAX_WASTED_INFLATE,
        "four failures of 64 MiB, then nothing more is inflated"
    );
    // The budget is spent: the mount counts as corrupt and every read of it fails without inflating,
    // the honest entry's too (an honest archive never gets here).
    let spent = mount.inflated();
    assert_eq!(mount.read(b"e7.bin"), None);
    assert_eq!(mount.read(b"honest.txt"), None);
    assert_eq!(mount.inflated(), spent);
}

#[test]
fn the_largest_entry_reads_whole() {
    let zeros = vec![0u8; usize::try_from(MAX_ENTRY_BYTES).unwrap()];
    let mount = ZipMount::open(zip_of(&[(b"max.bin", &zeros)])).unwrap();
    assert_eq!(mount.entry(b"max.bin").unwrap().size, MAX_ENTRY_BYTES);
    let read = mount.read(b"max.bin").unwrap();
    assert_eq!(read.len() as u64, MAX_ENTRY_BYTES);
    assert!(read.iter().all(|&b| b == 0));
    assert_eq!(mount.inflated(), MAX_ENTRY_BYTES);
}

// ---------------------------------------------------------------- archive-level refusals

#[test]
fn archives_that_are_not_zips_too_large_or_claim_too_many_entries_are_refused() {
    assert!(matches!(
        ZipMount::open(Vec::new()),
        Err(MountError::NotAZip(_))
    ));
    assert!(matches!(
        ZipMount::open(b"PK\x03\x04 not really".to_vec()),
        Err(MountError::NotAZip(_))
    ));
    assert!(matches!(
        ZipMount::open(vec![0; 1000]),
        Err(MountError::NotAZip(_))
    ));
    // A zip64 end record claiming a billion entries, wherever it is.
    let mut bytes = zip_of(&[(b"ok.cs", b"ok")]);
    let mut record = b"PK\x06\x06".to_vec();
    record.extend_from_slice(&44u64.to_le_bytes());
    record.extend_from_slice(&[0; 20]);
    record.extend_from_slice(&1_000_000_000u64.to_le_bytes());
    record.extend_from_slice(&[0; 16]);
    bytes.splice(0..0, record);
    assert_eq!(
        ZipMount::open(bytes).err(),
        Some(MountError::TooManyEntries(1_000_000_000))
    );
}

#[test]
fn the_archive_size_cap_is_exact() {
    assert!(matches!(
        ZipMount::open(vec![0; MAX_ZIP_BYTES]),
        Err(MountError::NotAZip(_))
    ));
    assert_eq!(
        ZipMount::open(vec![0; MAX_ZIP_BYTES + 1]).err(),
        Some(MountError::TooLarge(MAX_ZIP_BYTES + 1))
    );
}

/// A zip64 archive of `n` empty stored entries (the writer writes the zip64 end records from 65 536 on).
fn many_entries(n: usize) -> Vec<u8> {
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for i in 0..n {
        w.start_file(format!("{i}"), options).unwrap();
    }
    w.finish().unwrap().into_inner()
}

#[test]
fn the_entry_count_cap_is_exact() {
    let at_cap = ZipMount::open(many_entries(MAX_ENTRIES)).unwrap();
    assert_eq!(at_cap.entries().len(), MAX_ENTRIES);
    assert!(at_cap.skipped().is_empty());
    assert_eq!(at_cap.read(b"65535"), Some(Vec::new()));
    assert_eq!(
        ZipMount::open(many_entries(MAX_ENTRIES + 1)).err(),
        Some(MountError::TooManyEntries(MAX_ENTRIES as u64 + 1))
    );
}

/// A zip64 end record (sig, `size` as its record size, 20 bytes, `claim` entries, 16 bytes) as stored data.
fn zip64_record(size: u64, claim: u64) -> Vec<u8> {
    let mut record = b"PK\x06\x06".to_vec();
    record.extend_from_slice(&size.to_le_bytes());
    record.extend_from_slice(&[0; 20]);
    record.extend_from_slice(&claim.to_le_bytes());
    record.extend_from_slice(&[0; 16]);
    record
}

#[test]
fn four_bytes_that_look_like_a_zip64_record_in_data_are_not_a_claim() {
    // The signature followed by a record size the `zip` crate rejects (below 40, or beyond the rest of the
    // file) can only be data: an honest archive must not be refused for holding it, whatever it "claims".
    for size in [u64::MAX, 0, 1, 39, 1 << 40] {
        let stored = zip64_record(size, 1_000_000_000);
        let mount = ZipMount::open(build(&[raw(b"data.bin", &stored), raw(b"ok.cs", b"ok")]))
            .unwrap_or_else(|e| panic!("record size {size}: {e:?}"));
        assert_eq!(mount.read(b"data.bin"), Some(stored));
    }
    // A record of size 40 or more that fits in the file is the crate's to accept, and the claim counts.
    for size in [40, 44] {
        let record = zip64_record(size, 1_000_000_000);
        assert_eq!(
            ZipMount::open(build(&[raw(b"data.bin", &record)])).err(),
            Some(MountError::TooManyEntries(1_000_000_000))
        );
    }
}

#[test]
fn a_zip64_record_counts_exactly_when_it_and_its_12_header_bytes_fit_in_the_rest_of_the_file() {
    let archive = |extra: u64| {
        let mut bytes = build(&[raw(b"data.bin", &zip64_record(0, 1_000_000_000))]);
        let at = find_all(&bytes, b"PK\x06\x06")[0];
        let rest = (bytes.len() - at) as u64;
        // The record size is relative to the room there is: patch it in (the length does not change).
        bytes[at + 4..at + 12].copy_from_slice(&(rest - 12 + extra).to_le_bytes());
        bytes
    };
    // Fits exactly: counted (and refused as a claim); one byte more: only data.
    assert_eq!(
        ZipMount::open(archive(0)).err(),
        Some(MountError::TooManyEntries(1_000_000_000))
    );
    assert!(ZipMount::open(archive(1)).is_ok());
}

/// A zip32 archive of empty entries with long names whose central directory, with the 22-byte end record,
/// spans exactly `span` bytes.
fn archive_with_directory_span(span: usize) -> Vec<u8> {
    let mut left = span - 22;
    let mut entries = Vec::new();
    while left > 0 {
        // A central record is 46 bytes and the name; keep what is left buildable (at least one name byte).
        let mut record = left.min(46 + 60_000);
        if left != record && left - record < 47 {
            record = left - 47;
        }
        let mut name = format!("n{:05}", entries.len()).into_bytes();
        name.resize(record - 46, b'a');
        entries.push(raw(&name, b""));
        left -= record;
    }
    build(&entries)
}

#[test]
fn the_directory_size_cap_is_exact() {
    let at_cap = archive_with_directory_span(MAX_DIRECTORY_BYTES);
    let mount = ZipMount::open(at_cap).unwrap();
    assert!(mount.entries().len() > 100);
    assert_eq!(mount.directory_bytes(), MAX_DIRECTORY_BYTES);
    let over = archive_with_directory_span(MAX_DIRECTORY_BYTES + 1);
    assert_eq!(
        ZipMount::open(over).err(),
        Some(MountError::DirectoryTooLarge(MAX_DIRECTORY_BYTES + 1))
    );
}

#[test]
fn a_flood_of_look_alike_end_records_is_refused_before_the_crate_searches_for_them() {
    // 1 MiB of zeros then 1000 end records that point at a directory nobody has: the crate would try each
    // and look through the whole file for a directory every time (3.7 s in a debug build, and the cost
    // grows with records times length).
    let mut record = END.to_vec();
    record.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0]);
    record.extend_from_slice(&46u32.to_le_bytes());
    record.extend_from_slice(&0u32.to_le_bytes());
    record.extend_from_slice(&0u16.to_le_bytes());
    let mut bytes = vec![0u8; 1 << 20];
    for _ in 0..1000 {
        bytes.extend_from_slice(&record);
    }
    assert_eq!(
        ZipMount::open(bytes).err(),
        Some(MountError::TooManyEndRecords(1000))
    );
}

#[test]
fn look_alike_end_records_that_overlap_a_p_byte_are_counted_too() {
    // `PPK\5\6` units: the real signature starts one byte into each unit, so the scan must advance one
    // byte after a `P` that is not a signature. A scan that skips two or four bytes counts none of them and
    // hands the `zip` crate the multi-day search the cap exists to prevent.
    let units = 1000;
    for unit in [&b"PPK\x05\x06"[..], b"PPPK\x05\x06", b"PK\x05\x06"] {
        let mut bytes = vec![0u8; 1 << 20];
        for _ in 0..units {
            bytes.extend_from_slice(unit);
        }
        assert_eq!(
            ZipMount::open(bytes).err(),
            Some(MountError::TooManyEndRecords(units)),
            "{:?}",
            String::from_utf8_lossy(unit)
        );
    }
}

#[test]
fn the_largest_claim_of_several_zip64_records_counts() {
    // Two plausible records, the big claim first: refused. A claim taken from the last record only would let
    // the small one hide the big one.
    let big = zip64_record(44, 1_000_000_000);
    let small = zip64_record(44, 3);
    let mut blob = big.clone();
    blob.extend_from_slice(&small);
    assert_eq!(
        ZipMount::open(build(&[raw(b"blob.bin", &blob)])).err(),
        Some(MountError::TooManyEntries(1_000_000_000))
    );
    let mut blob = small;
    blob.extend_from_slice(&big);
    assert_eq!(
        ZipMount::open(build(&[raw(b"blob.bin", &blob)])).err(),
        Some(MountError::TooManyEntries(1_000_000_000))
    );
}

#[test]
fn up_to_the_end_record_cap_an_archive_may_hold_more_than_one() {
    // Stored zips inside the archive carry end records of their own: the real one plus
    // MAX_END_RECORDS - 1 inner ones is fine, one more is refused.
    let inner = build(&[raw(b"inner.cs", b"x")]);
    let make = |inner_zips: usize| {
        let entries: Vec<Raw> = (0..inner_zips)
            .map(|i| raw(format!("z{i}.zip").as_bytes(), &inner))
            .collect();
        build(&entries)
    };
    let mount = ZipMount::open(make(MAX_END_RECORDS - 1)).unwrap();
    assert_eq!(mount.entries().len(), MAX_END_RECORDS - 1);
    assert_eq!(mount.read(b"z0.zip"), Some(inner.clone()));
    assert_eq!(
        ZipMount::open(make(MAX_END_RECORDS)).err(),
        Some(MountError::TooManyEndRecords(MAX_END_RECORDS + 1))
    );
}

#[test]
fn plausible_zip64_records_are_counted_apart_from_zip32_ones() {
    // MAX_END_RECORDS + 1 plausible zip64 records (each claiming one entry), no zip32 look-alike beyond the
    // real end record: refused on the zip64 count alone.
    let mut record = b"PK\x06\x06".to_vec();
    record.extend_from_slice(&44u64.to_le_bytes());
    record.extend_from_slice(&[0; 20]);
    record.extend_from_slice(&1u64.to_le_bytes());
    record.extend_from_slice(&[0; 16]);
    let blob: Vec<u8> = record.repeat(MAX_END_RECORDS + 1);
    assert_eq!(
        ZipMount::open(build(&[raw(b"blob.bin", &blob)])).err(),
        Some(MountError::TooManyEndRecords(MAX_END_RECORDS + 1))
    );
    let blob: Vec<u8> = record.repeat(MAX_END_RECORDS);
    assert!(ZipMount::open(build(&[raw(b"blob.bin", &blob)])).is_ok());
}

// ---------------------------------------------------------------- truncation and fuzz

#[test]
fn every_prefix_and_every_suffix_of_an_archive_is_handled() {
    let base = zip_of(&[
        (b"description.txt", b"Title: cut"),
        (b"server.cs", &b"echo(1);\n".repeat(30)),
        (b"sub/data.txt", b""),
    ]);
    for cut in 0..=base.len() {
        for bytes in [base[..cut].to_vec(), base[cut..].to_vec()] {
            if let Ok(mount) = ZipMount::open(bytes) {
                for e in mount.entries().to_vec() {
                    let _ = mount.read(&e.name);
                }
            }
        }
    }
}

/// A deterministic generator (no rand dependency).
struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Values worth writing into a header field: boundaries of the caps, of the integer widths, and zero.
const LIES: [u64; 14] = [
    0,
    1,
    0xff,
    0xffff,
    0x7fff_ffff,
    0xffff_ffff,
    1 << 26,
    (1 << 26) + 1,
    1 << 32,
    1_000_000_000,
    u64::MAX,
    65_535,
    65_536,
    65_537,
];

/// Opens a mutated archive and reads whatever mounts, twice; checks what the reader promises.
fn exercise(bytes: Vec<u8>) {
    let Ok(mount) = ZipMount::open(bytes) else {
        return;
    };
    assert!(mount.entries().len() <= MAX_ENTRIES);
    let mut declared_total = 0u64;
    for e in mount.entries().to_vec() {
        assert!(e.size <= MAX_ENTRY_BYTES);
        // Whatever the archive says, a mounted name is one a host may serve, already in its served form.
        assert!(
            entry_name_ok(&e.name),
            "{:?}",
            String::from_utf8_lossy(&e.name)
        );
        assert_eq!(served_name(&e.name).as_deref(), Some(e.name.as_slice()));
        declared_total += e.size + 1;
        for _ in 0..2 {
            if let Some(data) = mount.read(&e.name) {
                assert_eq!(data.len() as u64, e.size);
            }
        }
    }
    // Each entry inflates at most its declared size plus one byte on a failure (once), and exactly its
    // size on a success: twice at most.
    assert!(mount.inflated() <= declared_total * 2);
}

#[test]
fn mutated_archives_never_panic() {
    let writer_made = zip_of(&[
        (b"description.txt", b"Title: fuzz\r\nAuthor: test"),
        (b"server.cs", &b"echo(\"hello\");\n".repeat(40)),
        (b"sub/data.txt", b""),
    ]);
    let hand_made = build(&[
        raw(b"a.cs", b"echo(1);"),
        raw(b"dir/b.txt", &b"0123456789".repeat(20)),
        raw(b"c", b""),
    ]);
    // A zip64 archive (the writer's `large_file` puts a zip64 extra field in every header).
    let zip64 = {
        let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true);
        w.start_file("big.cs", options).unwrap();
        w.write_all(&b"x".repeat(300)).unwrap();
        w.start_file("small.cs", options).unwrap();
        w.write_all(b"y").unwrap();
        w.finish().unwrap().into_inner()
    };
    // Names the reader rewrites or refuses, a flagged entry, and method-99 headers with and without their
    // AE-x field.
    let hostile_names = build(&[
        raw(b"a\\b.cs", b"x"),
        raw(b"hi\xe9.txt", b"y"),
        Raw {
            method: 99,
            ..raw(b"aes.cs", b"z")
        },
        Raw {
            method: 99,
            flags: 1,
            extra: aes_extra(8),
            ..raw(b"real.aes", b"c")
        },
        Raw {
            flags: 1,
            ..raw(b"flagged.cs", b"f")
        },
        raw(b"..\\evil.cs", b"e"),
        raw(b"ok.cs", b"ok"),
    ]);
    let bases = [writer_made, hand_made, zip64, hostile_names];
    let mut rng = Xorshift(0x2545_f491_4f6c_dd1d);
    let mut archives = 0usize;
    for round in 0..8_000 {
        let base = &bases[round % bases.len()];
        // 1. Random byte changes.
        let mut bytes = base.clone();
        for _ in 0..=rng.below(8) {
            let at = rng.below(bytes.len());
            bytes[at] = rng.next() as u8;
        }
        exercise(bytes);
        // 2. A header field overwritten with a boundary value, 1 to 8 bytes, near a signature.
        let mut bytes = base.clone();
        let sigs: Vec<usize> = [LOCAL, CENTRAL, END, b"PK\x06\x06", b"PK\x06\x07"]
            .iter()
            .flat_map(|sig| find_all(base, sig))
            .collect();
        for _ in 0..=rng.below(3) {
            let sig = sigs.get(rng.below(sigs.len())).copied().unwrap_or(0);
            let at = sig + rng.below(60);
            let lie = LIES[rng.below(LIES.len())].to_le_bytes();
            for (i, b) in lie.iter().take(1 + rng.below(8)).enumerate() {
                if let Some(slot) = bytes.get_mut(at + i) {
                    *slot = *b;
                }
            }
        }
        exercise(bytes);
        // 3. Truncated, with or without junk after the cut.
        let mut bytes = base[..rng.below(base.len() + 1)].to_vec();
        for _ in 0..rng.below(40) {
            bytes.push(rng.next() as u8);
        }
        exercise(bytes);
        // 4. A forged zip64 end record and locator (counts of a billion and worse) after the archive,
        //    or the whole archive behind junk, or its directory repeated.
        let mut bytes = base.clone();
        match rng.below(3) {
            0 => {
                let eocd64_at = bytes.len() as u64;
                let claim = LIES[rng.below(LIES.len())];
                bytes.extend_from_slice(b"PK\x06\x06");
                bytes.extend_from_slice(&44u64.to_le_bytes());
                bytes.extend_from_slice(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                bytes.extend_from_slice(&claim.min(1 << 20).to_le_bytes());
                bytes.extend_from_slice(&claim.to_le_bytes());
                bytes.extend_from_slice(&LIES[rng.below(LIES.len())].to_le_bytes());
                bytes.extend_from_slice(&LIES[rng.below(LIES.len())].to_le_bytes());
                bytes.extend_from_slice(b"PK\x06\x07");
                bytes.extend_from_slice(&0u32.to_le_bytes());
                bytes.extend_from_slice(&eocd64_at.to_le_bytes());
                bytes.extend_from_slice(&1u32.to_le_bytes());
                // And an end record that says "look in zip64".
                bytes.extend_from_slice(END);
                bytes.extend_from_slice(&[0xff; 8]);
                bytes.extend_from_slice(&u32::MAX.to_le_bytes());
                bytes.extend_from_slice(&u32::MAX.to_le_bytes());
                bytes.extend_from_slice(&0u16.to_le_bytes());
            }
            1 => {
                let mut junk = vec![0u8; rng.below(200)];
                junk.iter_mut().for_each(|b| *b = rng.next() as u8);
                junk.extend_from_slice(&bytes);
                bytes = junk;
            }
            _ => {
                let from = bytes.len() / 2;
                let tail = bytes[from..].to_vec();
                bytes.splice(from..from, tail);
            }
        }
        exercise(bytes);
        archives += 4;
    }
    assert!(archives >= 32_000);
}
