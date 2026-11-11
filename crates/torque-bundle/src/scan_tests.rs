use super::*;

fn needle() -> Needle {
    Needle::new("the workspace", Path::new(r"C:\Work\Repo")).unwrap()
}

fn scanner(needles: Vec<Needle>) -> Scanner {
    Scanner::new(needles, MAX_INFLATED_BYTES).unwrap()
}

/// The scanner of the workspace needle.
fn ws() -> Scanner {
    scanner(vec![needle()])
}

fn labels(s: &Scanner, bytes: &[u8]) -> Vec<String> {
    s.scan(bytes).into_iter().map(|(label, _)| label).collect()
}

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn utf16be(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

#[test]
fn every_spelling_of_a_folder_is_found() {
    let s = ws();
    for hit in [
        r"panicked at C:\Work\Repo\src\lib.rs".as_bytes().to_vec(),
        b"c:/work/repo/src".to_vec(),
        br#"Path("C:\\Work\\Repo\\x")"#.to_vec(),
        b"C://Work//Repo//x".to_vec(),
        br"C:/Work\Repo/mixed".to_vec(),
        utf16le(r"C:\WORK\REPO"),
        utf16be(r"C:\WORK\REPO"),
        utf16be("c:/work/repo"),
        utf16le(r"C:\\Work\\Repo"),
        b"/c/work/repo/src".to_vec(),
        b"/mnt/c/Work/Repo/src".to_vec(),
    ] {
        assert_eq!(labels(&s, &hit), ["the workspace"], "{hit:?}");
    }
    assert!(labels(&s, b"/workspace/src/lib.rs C:\\Work\\Other").is_empty());
}

#[test]
fn percent_encoded_and_short_name_spellings_are_found() {
    let home = scanner(vec![
        Needle::new("the home folder", Path::new(r"C:\Home\My Name Long")).unwrap(),
    ]);
    for hit in [
        "file:///C:/Home/My%20Name%20Long/dev",
        "file:///C%3A/Home/My%20Name%20Long",
        "file:///c:/Home/my%20name%20long",
        r"C:\Home\MYNAME~1\dev",
        r"c:\Home\myname~2",
        "C:/Home/MYNAME~4/x",
    ] {
        assert_eq!(labels(&home, hit.as_bytes()), ["the home folder"], "{hit}");
    }
    // Not this folder: another number, another name.
    assert!(labels(&home, br"C:\Home\MYNAME~5").is_empty());
    assert!(labels(&home, br"C:\Home\OTHER~1").is_empty());
    // A path with no drive has none of the drive spellings.
    let plain = scanner(vec![
        Needle::new("p", Path::new("/home/someone/work")).unwrap(),
    ]);
    assert_eq!(labels(&plain, b"/home/someone/work"), ["p"]);
}

#[test]
fn short_names_follow_the_8_3_rule_and_keep_dashes_and_underscores() {
    assert_eq!(short_name("My Name Long", 1).as_deref(), Some("MYNAME~1"));
    assert_eq!(short_name("torque-rs", 2).as_deref(), Some("TORQUE~2"));
    assert_eq!(short_name("my-long-name", 1).as_deref(), Some("MY-LON~1"));
    assert_eq!(short_name("a_long_name_x", 1).as_deref(), Some("A_LONG~1"));
    assert_eq!(
        short_name("notes.markdown", 1).as_deref(),
        Some("NOTES~1.MAR")
    );
    assert_eq!(short_name("Home", 1), None);
    assert_eq!(short_name("a.txt", 1), None);
    assert_eq!(short_name("my-dir", 1), None);
    assert_eq!(short_name("snake_9", 1), None);
    let dashed = scanner(vec![
        Needle::new("the checkout", Path::new(r"C:\Work\my-long-name")).unwrap(),
    ]);
    assert_eq!(labels(&dashed, br"C:\Work\MY-LON~1\src"), ["the checkout"]);
}

#[test]
fn user_and_host_names_are_soft_and_only_between_separators() {
    assert!(Needle::name("the user name", "tiny").is_none());
    let s = scanner(vec![Needle::name("the user name", "runner").unwrap()]);
    // Bare, as a word in a program or a text: not a hit at all.
    for bare in [
        b"the runner finished".as_slice(),
        b"runner",
        b"/runner",
        b"runner/",
        b"x/runnerx/y",
    ] {
        assert!(s.scan(bare).is_empty(), "{bare:?}");
    }
    // As a path component: a warning, in UTF-8 and UTF-16.
    for hit in [
        b"/home/runner/work".to_vec(),
        br"C:\Home\RUNNER\dev".to_vec(),
        utf16le(r"C:\Home\runner\dev"),
        utf16be("/home/runner/x"),
    ] {
        assert_eq!(
            s.scan(&hit),
            [("the user name".to_string(), true)],
            "{hit:?}"
        );
    }
    let hits = s.scan_file(b"/home/runner/work");
    assert_eq!(hits.len(), 1);
    assert!(!hits[0].is_hard());
    assert_eq!(
        hits[0].describe("bin/game"),
        "bin/game: holds the user name"
    );
    // A folder needle is a hard hit.
    assert!(ws().scan_file(br"C:\Work\Repo")[0].is_hard());
}

#[test]
fn short_folders_are_not_looked_for() {
    assert!(Needle::new("root", Path::new("C:\\")).is_none());
    assert!(Needle::new("verbatim", Path::new(r"\\?\C:\Work\Repo\")).is_some());
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn zip_of(members: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in members {
        w.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn png_with(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut png = PNG_SIGNATURE.to_vec();
    for (kind, data) in [(b"IHDR", &[0u8; 13][..]), (kind, data)] {
        png.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        png.extend_from_slice(&[0; 4]);
    }
    png.extend_from_slice(&0u32.to_be_bytes());
    png.extend_from_slice(b"IEND");
    png.extend_from_slice(&[0; 4]);
    png
}

fn zlib(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn leak(member: &str) -> Hit {
    Hit::Leak {
        member: member.into(),
        label: "the workspace".into(),
        soft: false,
    }
}

fn unscannable(hits: &[Hit], member: &str) -> bool {
    hits.iter()
        .any(|h| matches!(h, Hit::Unscannable { member: m, .. } if m == member))
}

fn members(hits: &[Hit]) -> Vec<&str> {
    hits.iter()
        .map(|h| match h {
            Hit::Leak { member, .. } | Hit::Unscannable { member, .. } => member.as_str(),
        })
        .collect()
}

#[test]
fn what_is_compressed_inside_a_file_is_scanned_and_the_member_named() {
    let s = ws();
    let text = br"built in C:\Work\Repo\target".repeat(8);
    // Compressed, the folder is not in the file's own bytes.
    let inside_zip = zip_of(&[("readme.txt", b"nothing"), ("notes/log.txt", &text)]);
    assert!(s.scan(&inside_zip).is_empty());
    assert_eq!(s.scan_file(&inside_zip), [leak("!notes/log.txt")]);
    assert_eq!(
        s.scan_file(&inside_zip)[0].describe("mods/a.zip"),
        "mods/a.zip!notes/log.txt: holds the workspace"
    );
    // A zip in a zip, and a gzip, and a raw zlib stream.
    let nested = zip_of(&[("inner.zip", &inside_zip)]);
    assert_eq!(members(&s.scan_file(&nested)), ["!inner.zip!notes/log.txt"]);
    assert_eq!(s.scan_file(&gzip(&text)), [leak("!gunzip")]);
    assert_eq!(s.scan_file(&zlib(&text)), [leak("!zlib")]);
    // PNG text chunks: zTXt, iCCP and a compressed iTXt (a plain tEXt is in the file's own bytes).
    let mut ztxt = b"Comment\0\0".to_vec();
    ztxt.extend(zlib(&text));
    assert_eq!(
        members(&s.scan_file(&png_with(b"zTXt", &ztxt))),
        ["!zTXt chunk"]
    );
    let mut iccp = b"profile\0\0".to_vec();
    iccp.extend(zlib(&text));
    assert_eq!(
        members(&s.scan_file(&png_with(b"iCCP", &iccp))),
        ["!iCCP chunk"]
    );
    let mut itxt = b"Comment\0\x01\0en\0Kommentar\0".to_vec();
    itxt.extend(zlib(&text));
    assert_eq!(
        members(&s.scan_file(&png_with(b"iTXt", &itxt))),
        ["!iTXt chunk"]
    );
    let mut plain_itxt = b"Comment\0\0\0en\0Kommentar\0".to_vec();
    plain_itxt.extend(&text);
    assert_eq!(members(&s.scan_file(&png_with(b"iTXt", &plain_itxt))), [""]);
    // Clean files give nothing.
    assert!(s.scan_file(&zip_of(&[("a.txt", b"fine")])).is_empty());
    assert!(s.scan_file(&gzip(b"fine")).is_empty());
    assert!(s.scan_file(&zlib(b"fine")).is_empty());
}

#[test]
fn a_member_the_scan_cannot_read_is_a_hit_and_a_name_that_leaks_is_not_printed() {
    let s = ws();
    // A gzip header with a broken stream behind it.
    let mut broken = gzip(&b"x".repeat(1000));
    let len = broken.len();
    broken[len / 2] ^= 0xff;
    broken.truncate(len - 6);
    assert!(unscannable(&s.scan_file(&broken), "!gunzip"));
    // A member named after the folder is reported by its number (its name is in the zip's own bytes: a hit of the file too).
    let named = zip_of(&[(r"C:/Work/Repo/x.txt", br"C:\Work\Repo")]);
    let hits = s.scan_file(&named);
    assert!(hits.contains(&leak("")), "{hits:?}");
    assert!(hits.contains(&leak("!member 0")), "{hits:?}");
    assert!(
        members(&hits).iter().all(|m| !m.contains("Work")),
        "{hits:?}"
    );
}

#[test]
fn nesting_is_bounded_and_reported() {
    let s = ws();
    let mut bytes = zip_of(&[("leak.txt", br"C:\Work\Repo")]);
    for _ in 0..MAX_DEPTH + 2 {
        bytes = zip_of(&[("z.zip", &bytes)]);
    }
    let hits = s.scan_file(&bytes);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        matches!(&hits[0], Hit::Unscannable { why, member } if why.contains("nested too deep") && member.matches('!').count() == MAX_DEPTH),
        "{hits:?}"
    );
}

#[test]
fn something_that_looks_like_a_zip_and_does_not_open_is_reported() {
    let s = ws();
    // A local header with no directory behind it.
    let mut cut = zip_of(&[("a.txt", b"fine")]);
    cut.truncate(cut.len() - 30);
    assert!(unscannable(&s.scan_file(&cut), "!zip"));
}

#[test]
fn a_zip_with_more_members_than_the_scan_reads_is_reported() {
    let s = ws();
    let names: Vec<String> = (0..=MAX_MEMBERS).map(|i| format!("m{i}")).collect();
    let refs: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
    let hits = s.scan_file(&zip_of(&refs));
    assert!(
        hits.iter()
            .any(|h| matches!(h, Hit::Unscannable { member, why }
            if member.starts_with("!members past") && why.contains("more members"))),
        "{} hits",
        hits.len()
    );
}

#[test]
fn a_zip_behind_other_bytes_is_found_by_its_end_record() {
    let s = ws();
    let text = br"built in C:\Work\Repo\target".repeat(8);
    let zip = zip_of(&[("log.txt", &text)]);
    // A self-extracting file: a program, then the zip; and a polyglot that starts as text.
    for front in [
        [0x4d, 0x5a, 0x90, 0, 3, 0, 0, 0].repeat(64),
        b"#!/bin/sh\nexit\n".to_vec(),
    ] {
        let mut file = front;
        file.extend_from_slice(&zip);
        assert!(s.scan_file(&file).contains(&leak("!log.txt")));
    }
    // Four signature bytes in the middle of a program are not a zip.
    let mut program = vec![0x90u8; 4096];
    program[100..104].copy_from_slice(b"PK\x05\x06");
    assert!(s.scan_file(&program).is_empty());
}

#[test]
fn every_zip_of_a_file_is_scanned_not_only_the_last() {
    let s = ws();
    let text = br"built in C:\Work\Repo\target".repeat(8);
    let leaky = zip_of(&[("log.txt", &text)]);
    let clean = zip_of(&[("a.txt", b"fine")]);
    // The leaky zip first, a clean one after it (a reader opens the last): and the other way round.
    let mut file = leaky.clone();
    file.extend_from_slice(&clean);
    assert!(s.scan_file(&file).contains(&leak("!zip1/log.txt")));
    let mut file = clean.clone();
    file.extend_from_slice(&leaky);
    assert!(s.scan_file(&file).contains(&leak("!zip2/log.txt")));
    // Three zips.
    let mut three = leaky.clone();
    three.extend_from_slice(&clean);
    three.extend_from_slice(&leaky);
    let hits = s.scan_file(&three);
    assert!(hits.contains(&leak("!zip1/log.txt")) && hits.contains(&leak("!zip3/log.txt")));
    // A stored zip inside a zip is found as the member's own file and as a zip of the file: both are reported, once each.
    let outer = zip_with_stored("inner.zip", &leaky);
    let hits = s.scan_file(&outer);
    assert!(hits.contains(&leak("!zip2/inner.zip!log.txt")), "{hits:?}");
    assert!(hits.contains(&leak("!zip1/log.txt")), "{hits:?}");
    assert_eq!(hits.len(), 2, "{hits:?}");
}

/// A zip of one stored member `name` holding `data`.
fn zip_with_stored(name: &str, data: &[u8]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file(
        name,
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    std::io::Write::write_all(&mut w, data).unwrap();
    w.finish().unwrap().into_inner()
}

#[test]
fn a_zip_hidden_in_an_entry_the_scan_does_not_read_is_still_found() {
    let s = ws();
    let text = br"built in C:\Work\Repo	arget".repeat(8);
    let leaky = zip_of(&[("log.txt", &text)]);
    // A "directory" entry (its name ends in a slash) stored over the bytes of a second zip: `zip()` skips directory entries, so
    // only the second zip's own end record leads to the leak.
    let outer = zip_with_stored("folder/", &leaky);
    let hits = s.scan_file(&outer);
    assert!(
        hits.iter()
            .any(|h| matches!(h, Hit::Leak { member, .. } if member.ends_with("log.txt"))),
        "{hits:?}"
    );
}

#[test]
fn compressed_formats_the_scan_does_not_decode_are_unscannable() {
    let s = ws();
    for (magic, name) in [
        (&b"\xfd7zXZ\0rest"[..], "xz"),
        (b"\x28\xb5\x2f\xfdrest", "zstd"),
        (b"BZh91AY&SY", "bzip2"),
        (b"7z\xbc\xaf\x27\x1c\0\x04", "7z"),
        (b"wOFF\0\x01\0\0", "WOFF"),
        (b"wOF2\0\x01\0\0", "WOFF2"),
    ] {
        let hits = s.scan_file(magic);
        assert!(
            matches!(&hits[..], [Hit::Unscannable { why, .. }] if why.contains(name)),
            "{name}: {hits:?}"
        );
    }
    // Raw zlib that does not decode is reported; "x^" text is just text.
    assert!(unscannable(&s.scan_file(b"\x78\x9cnot a stream"), "!zlib"));
    assert!(s.scan_file(b"x^ is an exponent").is_empty());
}

#[test]
fn the_inflate_limit_is_the_callers() {
    let text = br"built in C:\Work\Repo\target".repeat(1000);
    let big = gzip(&text);
    let small = Scanner::new(vec![needle()], 1000).unwrap();
    let hits = small.scan_file(&big);
    assert!(
        matches!(&hits[..], [Hit::Unscannable { why, .. }] if why.contains("limit")),
        "{hits:?}"
    );
    let roomy = Scanner::new(vec![needle()], text.len()).unwrap();
    assert_eq!(roomy.scan_file(&big), [leak("!gunzip")]);
    // The budget is the file's, over everything inside it.
    let two = zip_of(&[("a", &text), ("b", &text)]);
    let hits = Scanner::new(vec![needle()], text.len() + 10)
        .unwrap()
        .scan_file(&two);
    assert!(hits.iter().any(|h| matches!(h, Hit::Unscannable { .. })));
}

#[test]
fn a_truncated_or_malformed_png_is_reported() {
    let s = ws();
    let mut ztxt = b"Comment\0\0".to_vec();
    ztxt.extend(zlib(b"fine"));
    let good = png_with(b"zTXt", &ztxt);
    assert!(s.scan_file(&good).is_empty());
    // Cut inside a chunk, before the IEND, and with a chunk header cut.
    assert!(unscannable(
        &s.scan_file(&good[..good.len() - 20]),
        "!zTXt chunk"
    ));
    assert!(unscannable(&s.scan_file(&good[..good.len() - 12]), "!PNG"));
    assert!(unscannable(&s.scan_file(&good[..good.len() - 5]), "!PNG"));
    // Text chunks whose fields do not parse.
    for (kind, data) in [
        (b"zTXt", b"no nul here".to_vec()),
        (b"iCCP", b"profile".to_vec()),
        (b"iTXt", b"key\0\x01\0no more fields".to_vec()),
        (b"iTXt", b"key\0\x07\0en\0t\0text".to_vec()),
    ] {
        let name = format!("!{} chunk", String::from_utf8_lossy(kind));
        let hits = s.scan_file(&png_with(kind, &data));
        assert!(unscannable(&hits, &name), "{name}: {hits:?}");
    }
    // A stream that does not inflate is reported too.
    let png = png_with(b"zTXt", b"Comment\0\0garbage that is no zlib stream");
    assert!(unscannable(&s.scan_file(&png), "!zTXt chunk"));
}

/// The EOCD of the last zip of `file` with its central-directory offset field set to `offset`.
fn with_directory_offset(mut file: Vec<u8>, offset: u32) -> Vec<u8> {
    let at = file.windows(4).rposition(|w| w == b"PK\x05\x06").unwrap();
    file[at + 16..at + 20].copy_from_slice(&offset.to_le_bytes());
    file
}

#[test]
fn a_crafted_directory_offset_cannot_hide_another_zip() {
    let s = ws();
    let text = br"built in C:\Work\Repo\target".repeat(8);
    let leaky = zip_of(&[("log.txt", &text)]);
    let clean = zip_of(&[("a.txt", b"fine")]);
    // The clean zip's directory offset says it starts at the front of the file, over the leaky zip: its end record used to cover
    // the leaky zip's, which was then skipped as "inside" it.
    let mut file = leaky.clone();
    file.extend_from_slice(&clean);
    let directory = u32::try_from(file.len() - 22 - central_directory_size(&clean)).unwrap();
    let crafted = with_directory_offset(file, directory);
    let hits = s.scan_file(&crafted);
    assert!(hits.contains(&leak("!zip1/log.txt")), "{hits:?}");
    // The leaky zip's own offset crafted instead: its member cannot be read, and that is a failure, not a pass.
    let mut file = clean.clone();
    file.extend_from_slice(&leaky);
    let directory = u32::try_from(file.len() - 22 - central_directory_size(&leaky)).unwrap();
    let hits = s.scan_file(&with_directory_offset(file, directory));
    assert!(hits.iter().any(Hit::is_hard), "{hits:?}");
}

/// The size of the central directory of the zip `zip` (from its end record).
fn central_directory_size(zip: &[u8]) -> usize {
    let at = zip.windows(4).rposition(|w| w == b"PK\x05\x06").unwrap();
    usize::try_from(u32::from_le_bytes(
        zip[at + 12..at + 16].try_into().unwrap(),
    ))
    .unwrap()
}

#[test]
fn the_names_of_this_computer_come_from_the_variables_the_home_folder_and_the_host_files() {
    let var = |name: &str| match name {
        "USERNAME" => Some("alexandra".to_string()),
        "USER" => Some("Alexandra".to_string()),
        "COMPUTERNAME" => Some("BUILD-BOX-7".to_string()),
        _ => None,
    };
    let read = |path: &str| match path {
        "/etc/hostname" => Some("build-box-7\n".to_string()),
        "/proc/sys/kernel/hostname" => Some("kernelhost\n".to_string()),
        _ => None,
    };
    let needles = name_needles(var, read, Some(Path::new("/People/alexander")));
    let s = scanner(needles);
    let found = |text: &str| -> Vec<String> { labels(&s, text.as_bytes()) };
    assert_eq!(found("/home/alexandra/x"), ["the user name"]);
    assert_eq!(found("/people/ALEXANDER/x"), ["the user name"]);
    assert_eq!(found("//build-box-7/share/"), ["the host name"]);
    assert_eq!(found("/kernelhost/"), ["the host name"]);
    // The same name from two sources is one needle; a bare word or a short name is none.
    assert_eq!(s.needles.len(), 4, "{:?}", s.needles);
    assert!(name_needles(|_| None, |_| None, Some(Path::new("/home/me"))).is_empty());
    assert!(name_needles(|_| Some(String::new()), |_| Some("\n".into()), None).is_empty());
}

#[test]
fn short_names_in_edge_cases() {
    // Leading dots, several dots, characters a short name cannot hold, and nothing usable at all.
    assert_eq!(short_name(".config", 1).as_deref(), Some("CONFIG~1"));
    assert_eq!(
        short_name("..hidden-folder", 2).as_deref(),
        Some("HIDDEN~2")
    );
    assert_eq!(
        short_name("my.long.name.txt", 1).as_deref(),
        Some("MYLONG~1.TXT")
    );
    assert_eq!(short_name("Évelyne Long", 1).as_deref(), Some("VELYNE~1"));
    assert_eq!(short_name("日本語日本語日本語", 1), None);
    assert_eq!(short_name("???????????", 1), None);
    assert_eq!(short_name("UPPER", 1), None);
    assert_eq!(short_name("exactly8", 1), None);
    assert_eq!(short_name("ninechars", 1).as_deref(), Some("NINECH~1"));
    // Two long components: either, or both, in short form, with each number.
    let two = scanner(vec![
        Needle::new("p", Path::new(r"C:\Work Place\My Project Dir")).unwrap(),
    ]);
    for hit in [
        r"C:\WORKPL~1\MYPROJ~1",
        r"C:\WORKPL~1\My Project Dir",
        r"C:\Work Place\MYPROJ~3",
        r"C:\WORKPL~2\MYPROJ~4\x",
    ] {
        assert_eq!(labels(&two, hit.as_bytes()), ["p"], "{hit}");
    }
    assert!(labels(&two, br"C:\WORKPL~5\MYPROJ~1").is_empty());
    // Past four long components only `~1` is tried, to keep the spellings few.
    let many = scanner(vec![
        Needle::new(
            "m",
            Path::new(r"C:\aaaaaaaaa\bbbbbbbbb\ccccccccc\ddddddddd\eeeeeeeee"),
        )
        .unwrap(),
    ]);
    assert_eq!(
        labels(&many, br"C:\AAAAAA~1\BBBBBB~1\CCCCCC~1\DDDDDD~1\EEEEEE~1"),
        ["m"]
    );
    assert!(labels(&many, br"C:\AAAAAA~2\BBBBBB~1\CCCCCC~1\DDDDDD~1\EEEEEE~1").is_empty());
}

#[test]
fn every_detector_runs_on_a_file_that_is_more_than_one_container() {
    let s = ws();
    let text = br"built in C:\Work\Repo\target".repeat(8);
    // A PNG with a compressed text chunk, and a zip after it: a polyglot. Both are looked into.
    let mut ztxt = b"Comment\0\0".to_vec();
    ztxt.extend(zlib(&text));
    let mut file = png_with(b"zTXt", &ztxt);
    file.extend_from_slice(&zip_of(&[("log.txt", &text)]));
    let hits = s.scan_file(&file);
    assert!(hits.contains(&leak("!zTXt chunk")), "{hits:?}");
    assert!(hits.contains(&leak("!log.txt")), "{hits:?}");
}

#[test]
fn a_gap_between_the_central_directory_and_the_end_record_is_still_a_zip() {
    let s = ws();
    let text = br"built in C:\Work\Repo\target".repeat(8);
    let mut file = zip_of(&[("log.txt", &text)]);
    // Something a zip may hold there (a zip64 end record, a signature record): the zip is not a clean file because of it.
    let at = file.windows(4).rposition(|w| w == b"PK\x05\x06").unwrap();
    file.splice(at..at, [0u8; 100]);
    let hits = s.scan_file(&file);
    assert!(hits.iter().any(Hit::is_hard), "{hits:?}");
}

#[test]
fn short_name_spellings_are_capped_with_the_lowest_numbers_first() {
    let forms = short_forms("C:/aaaaaaaaa/bbbbbbbbb/ccccccccc/ddddddddd");
    assert_eq!(forms.len(), MAX_SHORT_SPELLINGS);
    assert!(forms.contains(&"C:/AAAAAA~1/BBBBBB~1/CCCCCC~1/DDDDDD~1".to_string()));
    assert!(forms.contains(&"C:/aaaaaaaaa/BBBBBB~2/ccccccccc/ddddddddd".to_string()));
    let unique: std::collections::HashSet<&String> = forms.iter().collect();
    assert_eq!(unique.len(), forms.len());
    // A folder with few long parts is not cut.
    assert_eq!(short_forms("C:/Work Place/My Project Dir").len(), 24);
}

#[test]
fn encoded_separators_are_found() {
    let s = ws();
    for hit in [
        "C%3A%5CWork%5CRepo",
        "C%3a%5cwork%5crepo%5csrc",
        "C%3A%2FWork%2FRepo",
        "C:%2FWork%2FRepo",
        "C:%5CWork%5CRepo",
    ] {
        assert_eq!(labels(&s, hit.as_bytes()), ["the workspace"], "{hit}");
    }
    assert!(labels(&s, b"C%3A%5CWork%5COther").is_empty());
}
