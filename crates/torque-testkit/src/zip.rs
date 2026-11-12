//! A zip archive built in memory.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn build(entries: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        let options = SimpleFileOptions::default().compression_method(method);
        writer
            .start_file(*name, options)
            .expect("start a zip entry");
        writer.write_all(bytes).expect("write a zip entry");
    }
    writer.finish().expect("finish the zip").into_inner()
}

/// A zip of `(name, bytes)` entries, deflated. Every entry is a file, whatever its name ends in.
#[must_use]
pub fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    build(entries, CompressionMethod::Deflated)
}

/// The same, stored without compression.
#[must_use]
pub fn zip_stored_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    build(entries, CompressionMethod::Stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn both_builders_make_zips_that_read_back() {
        let entries: [(&str, &[u8]); 2] = [("a.txt", b"alpha"), ("dir/b.bin", &[0u8; 300])];
        for bytes in [zip_of(&entries), zip_stored_of(&entries)] {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("a zip");
            assert_eq!(archive.len(), 2);
            let mut text = String::new();
            archive
                .by_name("a.txt")
                .expect("an entry")
                .read_to_string(&mut text)
                .expect("read");
            assert_eq!(text, "alpha");
        }
        assert!(zip_of(&entries).len() < zip_stored_of(&entries).len());
    }
}
