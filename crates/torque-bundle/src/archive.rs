//! Archives of a bundle that come out byte for byte the same from the same files: a zip (Windows) or a gzipped tar (Linux and
//! macOS).
//!
//! Everything a tool would stamp is fixed: every entry has the modification time 1980-01-01 00:00:00 UTC (the first time a zip
//! can hold), owner root (id 0), mode 0755 for folders and executables and 0644 for the rest, no extra fields, no comment, and
//! the entries are sorted by path, folders before what they hold. The compressor runs at one level and its output depends on
//! nothing else; the gzip header has no name and no time. Both formats are written here, not by a library that picks its own
//! headers. The deflate bytes are those of the compressor version Cargo.lock pins (another version can compress differently, so
//! archives are compared from builds of one lock file); builds on different CPUs have not been compared.
//!
//! Every path is under one top folder (`<name>-<os>-<arch>/`), so an archive unpacks into a folder of its own.

use std::collections::{BTreeSet, HashSet};
use std::io::Write;

use flate2::{Compression, Crc, write::DeflateEncoder};

/// 1980-01-01 00:00:00 UTC in seconds since 1970: the entries' modification time, and the `SOURCE_DATE_EPOCH` of the builds.
pub const EPOCH: u64 = 315_532_800;

const FOLDER_MODE: u32 = 0o755;
pub(crate) const EXECUTABLE_MODE: u32 = 0o755;
const FILE_MODE: u32 = 0o644;

/// The archive formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    TarGz,
}

impl Format {
    /// The format for a bundle built for `target`: a zip on Windows, a gzipped tar elsewhere.
    pub fn for_target(target: &str) -> Format {
        if target.contains("windows") {
            Format::Zip
        } else {
            Format::TarGz
        }
    }

    /// The file name's ending.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::TarGz => "tar.gz",
        }
    }
}

/// A file to archive.
#[derive(Clone, Debug)]
pub struct Item<'a> {
    /// The path in the bundle, with `/` separators.
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub executable: bool,
}

/// One entry of the archive, in the order it is written.
struct Entry<'a> {
    /// The full path; a folder's ends in `/`.
    path: String,
    mode: u32,
    bytes: &'a [u8],
}

/// A path component an archive may hold: not empty, not `.` or `..`, none of `/`, backslash and `:` (they make another path on
/// Windows: a separator, a drive or a stream), and no control character.
fn plain_component(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && !part.contains(['/', '\\', ':'])
        && !part.chars().any(char::is_control)
}

/// The entries of `items` under the folder `top`: the folders that hold them and the files, sorted by path.
fn plan<'a>(top: &str, items: &[Item<'a>]) -> Result<Vec<Entry<'a>>, String> {
    if !plain_component(top) {
        return Err(format!(
            "{top}: not a name for the top folder of an archive"
        ));
    }
    let mut folders: BTreeSet<String> = BTreeSet::new();
    folders.insert(format!("{top}/"));
    let mut sorted: Vec<&Item> = items.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(b.path));
    let mut out: Vec<Entry<'a>> = Vec::new();
    for item in &sorted {
        if item.path.split('/').any(|p| !plain_component(p)) {
            return Err(format!("{}: not a path inside a bundle", item.path));
        }
        let mut at = item.path;
        while let Some((parent, _)) = at.rsplit_once('/') {
            folders.insert(format!("{top}/{parent}/"));
            at = parent;
        }
        out.push(Entry {
            path: format!("{top}/{}", item.path),
            mode: if item.executable {
                EXECUTABLE_MODE
            } else {
                FILE_MODE
            },
            bytes: item.bytes,
        });
    }
    for pair in out.windows(2) {
        if pair[0].path == pair[1].path {
            return Err(format!("{}: twice in the archive", pair[0].path));
        }
    }
    // A file and a folder of one name cannot both unpack.
    let files: HashSet<&str> = out.iter().map(|e| e.path.as_str()).collect();
    if let Some(clash) = folders
        .iter()
        .map(|folder| folder.trim_end_matches('/'))
        .find(|name| files.contains(name))
    {
        return Err(format!("{clash}: both a file and a folder"));
    }
    out.extend(folders.into_iter().map(|path| Entry {
        path,
        mode: FOLDER_MODE,
        bytes: &[],
    }));
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// The archive of `items` in `format`, under the top folder `top`.
pub fn build(top: &str, items: &[Item], format: Format) -> Result<Vec<u8>, String> {
    let entries = plan(top, items)?;
    match format {
        Format::Zip => write_zip(&entries),
        Format::TarGz => write_tar_gz(&entries),
    }
}

fn deflate(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(bytes)
        .map_err(|e| format!("compress: {e}"))?;
    encoder.finish().map_err(|e| format!("compress: {e}"))
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(bytes);
    crc.sum()
}

fn u16_of(n: usize, what: &str) -> Result<u16, String> {
    u16::try_from(n).map_err(|_| format!("zip: too many {what} for a zip file"))
}

fn u32_of(n: usize, what: &str) -> Result<u32, String> {
    u32::try_from(n)
        .map_err(|_| format!("zip: {what} is over 4 GiB, which this writer does not do"))
}

/// The DOS date of 1980-01-01 (day 1, month 1, year 0 since 1980); the DOS time is 0.
const DOS_DATE: u16 = (1 << 5) | 1;
/// General purpose flag bit 11: names are UTF-8.
const UTF8_NAMES: u16 = 0x0800;

fn write_zip(entries: &[Entry]) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for entry in entries {
        let folder = entry.path.ends_with('/');
        let crc = crc32(entry.bytes);
        let packed = if folder || entry.bytes.is_empty() {
            None
        } else {
            Some(deflate(entry.bytes)?).filter(|p| p.len() < entry.bytes.len())
        };
        let (method, data): (u16, &[u8]) = match &packed {
            Some(p) => (8, p),
            None => (0, entry.bytes),
        };
        let name = entry.path.as_bytes();
        let name_len = u16_of(name.len(), "name bytes")?;
        let offset = u32_of(out.len(), "the archive")?;
        let size = u32_of(entry.bytes.len(), "a file")?;
        let packed_size = u32_of(data.len(), "a file")?;

        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(UTF8_NAMES.to_le_bytes());
        out.extend(method.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(DOS_DATE.to_le_bytes());
        out.extend(crc.to_le_bytes());
        out.extend(packed_size.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(name_len.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(name);
        out.extend(data);

        // Made by Unix (3), version 2.0; the mode in the high half of the external attributes, the DOS folder bit in the low.
        let type_bits: u32 = if folder { 0o040_000 } else { 0o100_000 };
        let attributes = ((type_bits | entry.mode) << 16) | u32::from(folder);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(((3u16 << 8) | 20).to_le_bytes());
        central.extend(20u16.to_le_bytes());
        central.extend(UTF8_NAMES.to_le_bytes());
        central.extend(method.to_le_bytes());
        central.extend(0u16.to_le_bytes());
        central.extend(DOS_DATE.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend(packed_size.to_le_bytes());
        central.extend(size.to_le_bytes());
        central.extend(name_len.to_le_bytes());
        central.extend([0u8; 2 + 2 + 2 + 2]);
        central.extend(attributes.to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(name);
    }
    let count = u16_of(entries.len(), "entries")?;
    let central_offset = u32_of(out.len(), "the archive")?;
    let central_size = u32_of(central.len(), "the directory")?;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(count.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend(central_size.to_le_bytes());
    out.extend(central_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    u32_of(out.len(), "the archive")?;
    Ok(out)
}

fn octal(field: &mut [u8], value: u64) -> Result<(), String> {
    let digits = field.len() - 1;
    let text = format!("{value:0digits$o}");
    if text.len() > digits {
        return Err(format!("tar: {value} does not fit a header field"));
    }
    field[..digits].copy_from_slice(text.as_bytes());
    field[digits] = 0;
    Ok(())
}

/// A ustar header: the path split into prefix and name when it is over 100 bytes.
fn tar_header(entry: &Entry) -> Result<[u8; 512], String> {
    let folder = entry.path.ends_with('/');
    let path = entry.path.trim_end_matches('/');
    let full = if folder {
        format!("{path}/")
    } else {
        path.to_string()
    };
    let (prefix, name) = if full.len() <= 100 {
        ("", full.as_str())
    } else {
        let split = full
            .match_indices('/')
            .map(|(i, _)| i)
            .find(|&i| i <= 155 && full.len() - i - 1 <= 100 && full.len() - i - 1 > 0)
            .ok_or_else(|| format!("tar: the path {full} is too long"))?;
        (&full[..split], &full[split + 1..])
    };
    let mut header = [0u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut header[100..108], u64::from(entry.mode))?;
    octal(&mut header[108..116], 0)?;
    octal(&mut header[116..124], 0)?;
    octal(&mut header[124..136], entry.bytes.len() as u64)?;
    octal(&mut header[136..148], EPOCH)?;
    header[156] = if folder { b'5' } else { b'0' };
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    header[265..269].copy_from_slice(b"root");
    header[297..301].copy_from_slice(b"root");
    octal(&mut header[329..337], 0)?;
    octal(&mut header[337..345], 0)?;
    header[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    header[148..156].fill(b' ');
    let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
    let text = format!("{sum:06o}\0 ");
    header[148..156].copy_from_slice(text.as_bytes());
    Ok(header)
}

fn write_tar_gz(entries: &[Entry]) -> Result<Vec<u8>, String> {
    let mut tar: Vec<u8> = Vec::new();
    for entry in entries {
        tar.extend(tar_header(entry)?);
        tar.extend(entry.bytes);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }
    tar.extend([0u8; 1024]);
    let mut out = vec![
        0x1f, 0x8b, // magic
        8,    // deflate
        0,    // no flags: no name, no extra, no comment
        0, 0, 0, 0, // no time
        2, // the slowest compressor was used
        3, // Unix, whatever built it
    ];
    out.extend(deflate(&tar)?);
    out.extend(crc32(&tar).to_le_bytes());
    // ISIZE is the length modulo 2^32, which is what the cast keeps.
    out.extend((tar.len() as u32).to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn items<'a>() -> Vec<Item<'a>> {
        vec![
            Item {
                path: "game/z.txt",
                bytes: b"zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
                executable: false,
            },
            Item {
                path: "demo.exe",
                bytes: b"MZ\x00\x01",
                executable: true,
            },
            Item {
                path: "game/sub/empty",
                bytes: b"",
                executable: false,
            },
            Item {
                path: "game/a.txt",
                bytes: b"hello\n",
                executable: false,
            },
        ]
    }

    #[test]
    fn entries_are_sorted_with_their_folders() {
        let paths: Vec<String> = plan("demo-x", &items())
            .unwrap()
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert_eq!(
            paths,
            [
                "demo-x/",
                "demo-x/demo.exe",
                "demo-x/game/",
                "demo-x/game/a.txt",
                "demo-x/game/sub/",
                "demo-x/game/sub/empty",
                "demo-x/game/z.txt",
            ]
        );
    }

    #[test]
    fn bad_and_repeated_paths_are_refused() {
        let item = |path| {
            vec![Item {
                path,
                bytes: b"",
                executable: false,
            }]
        };
        assert!(build("t", &item("../x"), Format::Zip).is_err());
        assert!(build("t", &item("/x"), Format::TarGz).is_err());
        assert!(build("t", &item("a//b"), Format::Zip).is_err());
        let twice = vec![item("a")[0].clone(), item("a")[0].clone()];
        assert!(build("t", &twice, Format::Zip).is_err());
    }

    #[test]
    fn backslashes_colons_dots_and_clashes_of_file_and_folder_are_refused() {
        let item = |path| Item {
            path,
            bytes: b"x",
            executable: false,
        };
        for bad in [
            "a\\b",
            "C:/x",
            "a/b:stream",
            "./a",
            "a/./b",
            "a/..",
            "a/",
            "",
        ] {
            for format in [Format::Zip, Format::TarGz] {
                let error = build("t", &[item(bad)], format).unwrap_err();
                assert!(error.contains("not a path"), "{bad}: {error}");
            }
        }
        let clash = build("t", &[item("a"), item("a/b")], Format::TarGz).unwrap_err();
        assert!(clash.contains("both a file and a folder"), "{clash}");
        let clash = build("t", &[item("a/b/c"), item("a/b")], Format::Zip).unwrap_err();
        assert!(clash.contains("both a file and a folder"), "{clash}");
        assert!(build("t", &[item("a.b"), item("a/b")], Format::Zip).is_ok());
        for top in ["", ".", "..", "a/b", "a\\b", "c:", "a:b"] {
            let error = build(top, &[item("a")], Format::Zip).unwrap_err();
            assert!(error.contains("top folder"), "{top}: {error}");
        }
    }

    #[test]
    fn the_same_files_in_any_order_give_the_same_bytes() {
        let mut reversed = items();
        reversed.reverse();
        for format in [Format::Zip, Format::TarGz] {
            assert_eq!(
                build("demo-x", &items(), format).unwrap(),
                build("demo-x", &reversed, format).unwrap()
            );
        }
    }

    #[test]
    fn the_zip_reads_back_with_modes_and_the_fixed_time() {
        let bytes = build("demo-x", &items(), Format::Zip).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(zip.len(), 7);
        let mut seen = Vec::new();
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).unwrap();
            let time = file.last_modified().unwrap();
            assert_eq!(
                (time.year(), time.month(), time.day(), time.hour()),
                (1980, 1, 1, 0)
            );
            let mode = file.unix_mode().unwrap() & 0o777;
            let mut text = Vec::new();
            file.read_to_end(&mut text).unwrap();
            seen.push((file.name().unwrap().to_string(), mode, text.len()));
        }
        assert_eq!(seen[0], ("demo-x/".to_string(), 0o755, 0));
        assert_eq!(seen[1], ("demo-x/demo.exe".to_string(), 0o755, 4));
        assert_eq!(seen[3], ("demo-x/game/a.txt".to_string(), 0o644, 6));
        assert_eq!(seen[6], ("demo-x/game/z.txt".to_string(), 0o644, 32));
    }

    fn octal_of(field: &[u8]) -> u64 {
        let text = std::str::from_utf8(field)
            .unwrap()
            .trim_matches(['\0', ' ']);
        u64::from_str_radix(text, 8).unwrap()
    }

    #[test]
    fn the_tar_reads_back_with_owner_modes_and_the_fixed_time() {
        let bytes = build("demo-x", &items(), Format::TarGz).unwrap();
        assert_eq!(&bytes[..10], [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 3]);
        let mut tar = Vec::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .read_to_end(&mut tar)
            .unwrap();
        let mut at = 0;
        let mut seen = Vec::new();
        while tar[at..at + 512].iter().any(|&b| b != 0) {
            let header = &tar[at..at + 512];
            let name = std::str::from_utf8(&header[..100])
                .unwrap()
                .trim_end_matches('\0');
            let size = octal_of(&header[124..136]) as usize;
            assert_eq!(
                octal_of(&header[108..116]) + octal_of(&header[116..124]),
                0,
                "owner ids"
            );
            assert_eq!(octal_of(&header[136..148]), EPOCH);
            let check = octal_of(&header[148..156]);
            let mut blank = header.to_vec();
            blank[148..156].fill(b' ');
            assert_eq!(check, blank.iter().map(|&b| u64::from(b)).sum::<u64>());
            seen.push((name.to_string(), octal_of(&header[100..108]), size));
            at += 512 + size.div_ceil(512) * 512;
        }
        assert_eq!(seen.len(), 7);
        assert_eq!(seen[0], ("demo-x/".to_string(), 0o755, 0));
        assert_eq!(seen[1], ("demo-x/demo.exe".to_string(), 0o755, 4));
        assert_eq!(seen[3], ("demo-x/game/a.txt".to_string(), 0o644, 6));
        assert_eq!(tar.len() - at, 1024);
    }

    #[test]
    fn a_long_path_is_split_into_prefix_and_name() {
        let deep = format!("{}/file.txt", "d".repeat(60) + "/" + &"e".repeat(60));
        let item = [Item {
            path: &deep,
            bytes: b"x",
            executable: false,
        }];
        let bytes = build("top", &item, Format::TarGz).unwrap();
        let mut tar = Vec::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .read_to_end(&mut tar)
            .unwrap();
        let mut at = 0;
        let mut last = String::new();
        while tar[at..at + 512].iter().any(|&b| b != 0) {
            let h = &tar[at..at + 512];
            let prefix = std::str::from_utf8(&h[345..500])
                .unwrap()
                .trim_end_matches('\0');
            let name = std::str::from_utf8(&h[..100])
                .unwrap()
                .trim_end_matches('\0');
            last = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            };
            at += 512 + (octal_of(&h[124..136]) as usize).div_ceil(512) * 512;
        }
        assert_eq!(last, format!("top/{deep}"));
    }
}
