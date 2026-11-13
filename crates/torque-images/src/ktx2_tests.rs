use super::*;

/// A texture of `format` at `w` x `h` with its whole chain, the bytes of each level made from its index.
pub(crate) fn chain(format: Ktx2Format, w: u32, h: u32) -> Ktx2 {
    let levels = (0..chain_len(w, h))
        .map(|k| {
            let n = format.level_bytes((w >> k).max(1), (h >> k).max(1)) as usize;
            (0..n).map(|i| (i * 31 + k as usize * 7) as u8).collect()
        })
        .collect();
    Ktx2 {
        format,
        width: w,
        height: h,
        levels,
        key_values: vec![("torque.role".to_string(), b"normal\0".to_vec())],
    }
}

#[test]
fn every_format_writes_and_reads_back_byte_for_byte() {
    for format in Ktx2Format::ALL {
        for sc in [Supercompression::None, Supercompression::Zstandard] {
            for (w, h) in [(1, 1), (8, 8), (64, 16), (5, 3)] {
                let t = chain(format, w, h);
                let file = write(&t, sc).unwrap();
                let back = read(&file, &Limits::default()).unwrap();
                assert_eq!(back.levels, t.levels, "{format:?} {sc:?} {w} x {h}");
                assert_eq!((back.format, back.width, back.height), (format, w, h));
                // The writer's name is added; written again, the file is the same.
                assert!(back.key_values.iter().any(|(k, _)| k == "KTXwriter"));
                assert_eq!(write(&back, sc).unwrap(), file);
                let hd = header(&file, &Limits::default()).unwrap();
                assert_eq!(hd.level_count, chain_len(w, h));
                assert_eq!(hd.supercompression, sc);
            }
        }
    }
}

#[test]
fn levels_are_stored_smallest_first_and_aligned() {
    let file = write(&chain(Ktx2Format::Bc7, 16, 16), Supercompression::None).unwrap();
    let offset = |k: usize| u64_at(&file, FIXED + LEVEL_ENTRY * k, "").unwrap();
    assert!(offset(0) > offset(1) && offset(1) > offset(2));
    assert!((0..5).all(|k| offset(k) % 16 == 0));
    // The descriptor's total size is its own length; the key/value data follows it.
    let dfd = u32_at(&file, 48, "").unwrap() as usize;
    assert_eq!(
        u32_at(&file, dfd, "").unwrap(),
        u32_at(&file, 52, "").unwrap()
    );
    assert_eq!(u32_at(&file, 56, "").unwrap() as usize, dfd + 44);
}

#[test]
fn files_past_their_limits_are_refused_from_the_header() {
    let file = write(
        &chain(Ktx2Format::Rgba8, 64, 64),
        Supercompression::Zstandard,
    )
    .unwrap();
    let small = Limits {
        max_side: 32,
        ..Limits::default()
    };
    assert!(matches!(read(&file, &small), Err(Ktx2Error::Size { .. })));
    let few = Limits {
        max_bytes: 1000,
        ..Limits::default()
    };
    assert!(matches!(read(&file, &few), Err(Ktx2Error::TooBig(..))));
    let no_kv = Limits {
        max_key_value_bytes: 8,
        ..Limits::default()
    };
    assert!(matches!(read(&file, &no_kv), Err(Ktx2Error::TooBig(..))));
}

/// `file` with the header's `i`th field set to `v`.
fn with_field(file: &[u8], i: usize, v: u32) -> Vec<u8> {
    let mut f = file.to_vec();
    f[12 + 4 * i..16 + 4 * i].copy_from_slice(&v.to_le_bytes());
    f
}

#[test]
fn headers_of_other_textures_are_refused() {
    let file = write(&chain(Ktx2Format::Bc5, 16, 16), Supercompression::None).unwrap();
    let l = Limits::default();
    assert_eq!(read(&file[..40], &l), Err(Ktx2Error::Truncated("header")));
    assert_eq!(read(&[0u8; 100], &l), Err(Ktx2Error::NotKtx2));
    assert_eq!(
        read(&with_field(&file, 0, 23), &l),
        Err(Ktx2Error::Format(23))
    );
    for (i, v) in [(1, 4), (4, 1), (5, 2), (6, 6), (7, 9)] {
        assert!(
            matches!(
                read(&with_field(&file, i, v), &l),
                Err(Ktx2Error::Header { .. })
            ),
            "{i}"
        );
    }
    assert!(matches!(
        read(&with_field(&file, 2, 0), &l),
        Err(Ktx2Error::Size { .. })
    ));
    assert_eq!(
        read(&with_field(&file, 8, 1), &l),
        Err(Ktx2Error::Scheme(1))
    );
    // A level count of 0 (make the chain) reads level 0 only, which a level 0 of its size is.
    assert_eq!(read(&with_field(&file, 7, 0), &l).unwrap().levels.len(), 1);
    // Truncated inside the levels.
    assert!(matches!(
        read(&file[..file.len() - 1], &l),
        Err(Ktx2Error::Range(_))
    ));
}

#[test]
fn a_zstandard_level_of_the_wrong_length_is_refused() {
    let t = chain(Ktx2Format::Rgba8, 4, 4);
    let mut file = write(&t, Supercompression::Zstandard).unwrap();
    // Level 0 named as level 1's stream: it decodes to 16 bytes, not 64.
    let mut swapped = file.clone();
    let level1 = swapped[FIXED + LEVEL_ENTRY..FIXED + LEVEL_ENTRY + 16].to_vec();
    swapped[FIXED..FIXED + 16].copy_from_slice(&level1);
    assert!(matches!(
        read(&swapped, &Limits::default()),
        Err(Ktx2Error::Zstandard { level: 0, .. })
    ));
    // Its level 0 stream cut in half.
    let at = FIXED;
    let offset = u64_at(&file, at, "").unwrap() as usize;
    let len = u64_at(&file, at + 8, "").unwrap() as usize;
    file.truncate(offset + len / 2);
    file[at + 8..at + 16].copy_from_slice(&((len / 2) as u64).to_le_bytes());
    assert!(matches!(
        read(&file, &Limits::default()),
        Err(Ktx2Error::Zstandard { level: 0, .. })
    ));
}

#[test]
fn the_writer_refuses_levels_that_are_not_their_size() {
    let mut t = chain(Ktx2Format::Bc4, 8, 8);
    t.levels[1].pop();
    assert!(matches!(
        write(&t, Supercompression::None),
        Err(Ktx2Error::Level { level: 1, .. })
    ));
    let mut t = chain(Ktx2Format::Bc4, 8, 8);
    t.levels.push(vec![0; 8]);
    assert!(write(&t, Supercompression::None).is_err());
    let mut t = chain(Ktx2Format::Bc4, 8, 8);
    t.key_values.push(("torque.role".to_string(), vec![]));
    assert!(matches!(
        write(&t, Supercompression::None),
        Err(Ktx2Error::KeyValue(_))
    ));
}

#[test]
fn malformed_key_value_data_is_refused() {
    assert!(key_value_entries(&[5, 0, 0, 0, b'a', b'b']).is_err());
    assert!(key_value_entries(&[2, 0, 0, 0, b'a', b'b', 0, 0]).is_err());
    assert!(key_value_entries(&[1, 0, 0, 0, 0, 0, 0, 0]).is_err());
    assert!(key_value_entries(&[2, 0, 0, 0, 0xff, 0, 0, 0]).is_err());
    assert_eq!(
        key_value_entries(&[3, 0, 0, 0, b'k', 0, 7, 0]).unwrap(),
        vec![("k", &[7u8][..])]
    );
}
