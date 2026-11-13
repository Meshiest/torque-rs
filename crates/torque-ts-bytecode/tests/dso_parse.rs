use crate::common;

use common::{empty, encode, encode_without_local_variables, sample, t3d};
use torque_ts_bytecode::{Dialect, DsoError, DsoFile, FileLayout, StringTable};

#[test]
fn parses_a_file_with_every_table_empty() {
    let dso = DsoFile::parse(&encode(&empty()), &t3d()).unwrap();
    assert_eq!(dso.version, 50);
    assert!(dso.code.is_empty() && dso.line_breaks.is_empty() && dso.identifiers.is_empty());
    assert!(dso.global_strings.is_empty() && dso.function_strings.is_empty());
    assert!(dso.local_variables.is_empty());
}

#[test]
fn parses_tables_local_variables_code_line_breaks_and_identifiers() {
    let s = sample();
    let dso = DsoFile::parse(&encode(&s), &t3d()).unwrap();
    assert_eq!(dso.version, 50);
    assert_eq!(dso.global_strings, s.global_strings);
    assert_eq!(dso.function_strings, s.function_strings);
    assert_eq!(dso.global_floats, vec![1.5, -2.0]);
    assert_eq!(dso.function_floats, vec![0.25]);
    assert_eq!(dso.local_variables.len(), 1);
    assert_eq!(dso.local_variables[0].function, b"(null)::f");
    assert_eq!(
        dso.local_variables[0].names,
        vec![b"%a".to_vec(), b"%b".to_vec()]
    );
    assert_eq!(
        dso.code,
        vec![0x0f, 5, 0xFE, 0xFF, 0x100, 0xDEAD_BEEF, 0x48]
    );
    assert_eq!(dso.line_breaks, vec![1, 0, 2, 3]);
    assert_eq!(dso.identifiers.len(), 2);
    assert_eq!(dso.identifiers[0].string_index, 1);
    assert_eq!(dso.identifiers[0].positions, vec![1, 4]);
    assert_eq!(dso.identifiers[1].positions, vec![2]);
}

#[test]
fn the_version_is_reported_not_enforced() {
    let mut s = empty();
    s.version = 36;
    assert_eq!(DsoFile::parse(&encode(&s), &t3d()).unwrap().version, 36);
}

#[test]
fn a_layout_without_the_local_variable_table_reads_and_writes_without_it() {
    let dialect = Dialect {
        dso_version: 36,
        layout: FileLayout {
            local_variable_table: false,
        },
        ..Dialect::torque3d()
    };
    let mut s = sample();
    s.version = 36;
    s.local_variables.clear();
    let bytes = encode_without_local_variables(&s);
    let dso = DsoFile::parse(&bytes, &dialect).unwrap();
    assert_eq!(dso.global_floats, vec![1.5, -2.0]);
    assert_eq!(dso.global_string_table().get(1), Some(&b"hello"[..]));
    assert!(dso.local_variables.is_empty());
    assert_eq!(dso.to_bytes(&dialect), bytes);
    // The same bytes are not a Torque3D file: the code size is read as the local-variable count.
    assert!(DsoFile::parse(&bytes, &t3d()).is_err());
}

#[test]
fn string_tables_find_strings() {
    let table = StringTable::new(&b"\0name\0other\0"[..]);
    assert_eq!(table.len(), 12);
    assert_eq!(table.get(0), Some(&b""[..]));
    assert_eq!(table.get(1), Some(&b"name"[..]));
    assert_eq!(table.get(6), Some(&b"other"[..]));
    assert!(table.is_string_start(1) && table.is_string_start(6));
    assert!(!table.is_string_start(2));
    assert_eq!(table.get(100), None);
    assert!(!table.is_string_start(100));
}

#[test]
fn a_string_table_exposes_its_bytes() {
    let plain: &[u8] = b"\0name\0\xb0other\0";
    let table = StringTable::new(plain);
    assert_eq!(table.as_bytes(), plain);
    assert_eq!(table.as_bytes().len(), table.len());
    assert_eq!(StringTable::new(Vec::new()).as_bytes(), b"");
}

#[test]
fn the_dso_file_exposes_its_string_tables() {
    let dso = DsoFile::parse(&encode(&sample()), &t3d()).unwrap();
    assert_eq!(dso.global_string_table().get(1), Some(&b"hello"[..]));
    assert_eq!(dso.function_string_table().get(0), Some(&b"fn"[..]));
}

#[test]
fn every_strict_prefix_of_a_valid_file_is_an_error_not_a_panic() {
    let bytes = encode(&sample());
    for len in 0..bytes.len() {
        assert!(
            DsoFile::parse(&bytes[..len], &t3d()).is_err(),
            "prefix of {len} bytes should not parse"
        );
    }
}

fn too_large(bytes: &[u8]) {
    let err = DsoFile::parse(bytes, &t3d()).unwrap_err();
    assert!(matches!(err, DsoError::TooLarge { .. }), "got {err:?}");
}

/// The version and the four empty constant tables.
fn empty_tables() -> Vec<u8> {
    let mut head = Vec::new();
    head.extend_from_slice(&50u32.to_le_bytes());
    for _ in 0..4 {
        head.extend_from_slice(&0u32.to_le_bytes());
    }
    head
}

#[test]
fn a_huge_declared_string_table_is_rejected_without_allocating() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&50u32.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    too_large(&bytes);
}

#[test]
fn a_huge_declared_float_count_is_rejected() {
    // version, two empty string tables, then a global float count of u32::MAX.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&50u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    too_large(&bytes);
}

#[test]
fn huge_declared_local_variable_counts_are_rejected() {
    let mut entries = empty_tables();
    entries.extend_from_slice(&u32::MAX.to_le_bytes());
    too_large(&entries);

    let mut names = empty_tables();
    names.extend_from_slice(&1u32.to_le_bytes());
    names.extend_from_slice(&[1, b'f']);
    names.extend_from_slice(&u32::MAX.to_le_bytes());
    too_large(&names);
}

#[test]
fn huge_declared_code_and_line_break_and_identifier_counts_are_rejected() {
    // version, 4 empty tables, an empty local-variable table, then the code.
    let mut head = empty_tables();
    head.extend_from_slice(&0u32.to_le_bytes());

    let mut code = head.clone();
    code.extend_from_slice(&u32::MAX.to_le_bytes());
    code.extend_from_slice(&0u32.to_le_bytes());
    too_large(&code);

    let mut breaks = head.clone();
    breaks.extend_from_slice(&0u32.to_le_bytes());
    breaks.extend_from_slice(&u32::MAX.to_le_bytes());
    too_large(&breaks);

    let mut idents = head.clone();
    idents.extend_from_slice(&0u32.to_le_bytes());
    idents.extend_from_slice(&0u32.to_le_bytes());
    idents.extend_from_slice(&u32::MAX.to_le_bytes());
    too_large(&idents);

    // One identifier entry declaring u32::MAX positions.
    let mut positions = head;
    positions.extend_from_slice(&0u32.to_le_bytes()); // code size
    positions.extend_from_slice(&0u32.to_le_bytes()); // line break count
    positions.extend_from_slice(&1u32.to_le_bytes()); // identifier count
    positions.extend_from_slice(&0u32.to_le_bytes()); // string index
    positions.extend_from_slice(&u32::MAX.to_le_bytes()); // position count
    too_large(&positions);
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut bytes = encode(&sample());
    bytes.push(0);
    assert!(matches!(
        DsoFile::parse(&bytes, &t3d()).unwrap_err(),
        DsoError::TrailingBytes { extra: 1, .. }
    ));
}
