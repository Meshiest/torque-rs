mod common;

use common::{empty, encode, sample, t3d};
use torque_ts_bytecode::dso::MAX_LOCAL_NAME_BYTES;
use torque_ts_bytecode::{Dialect, DsoFile, FileLayout, LocalVariables};

#[test]
fn writing_a_parsed_file_reproduces_the_input_bytes() {
    let bytes = encode(&sample());
    let dso = DsoFile::parse(&bytes, &t3d()).unwrap();
    assert_eq!(dso.to_bytes(&t3d()), bytes);
}

#[test]
fn parse_of_write_is_the_identity() {
    let dso = DsoFile::parse(&encode(&sample()), &t3d()).unwrap();
    assert_eq!(DsoFile::parse(&dso.to_bytes(&t3d()), &t3d()).unwrap(), dso);
}

#[test]
fn code_words_use_the_documented_widths() {
    let mut s = empty();
    s.code = vec![0, 0xFE, 0xFF, 0x100, 0xFFFF_FFFF];
    let bytes = DsoFile::parse(&encode(&s), &t3d())
        .unwrap()
        .to_bytes(&t3d());

    // version(4) + 4 table headers(16) + local-variable count(4) + code size(4) + line break count(4) = 32 bytes
    // before the code.
    let code_bytes = &bytes[32..bytes.len() - 4]; // the last 4 bytes are the identifier count
    let expected: Vec<u8> = vec![
        0x00, //                      0       -> 1 byte
        0xFE, //                      0xFE    -> 1 byte
        0xFF, 0xFF, 0, 0, 0, //       0xFF    -> escape + u32
        0xFF, 0x00, 0x01, 0, 0, //    0x100   -> escape + u32
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // 0xFFFFFFFF -> escape + u32
    ];
    assert_eq!(code_bytes, expected.as_slice());
}

#[test]
fn floats_keep_their_exact_bit_patterns() {
    let mut s = sample();
    s.global_floats = vec![f64::NAN, -0.0, f64::INFINITY, f64::MIN_POSITIVE];
    let bytes = encode(&s);
    let dso = DsoFile::parse(&bytes, &t3d()).unwrap();
    assert_eq!(dso.to_bytes(&t3d()), bytes);
}

#[test]
fn a_file_with_no_code_round_trips() {
    let bytes = encode(&empty());
    assert_eq!(
        DsoFile::parse(&bytes, &t3d()).unwrap().to_bytes(&t3d()),
        bytes
    );
}

#[test]
fn non_canonical_escapes_are_accepted_and_written_in_canonical_form() {
    // A file whose one code word is 0xFF followed by 5 (which should be one byte).
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&50u32.to_le_bytes()); // version
    for _ in 0..5 {
        bytes.extend_from_slice(&0u32.to_le_bytes()); // 4 constant tables, the local-variable table
    }
    bytes.extend_from_slice(&1u32.to_le_bytes()); // code size
    bytes.extend_from_slice(&0u32.to_le_bytes()); // line break count
    bytes.extend_from_slice(&[0xFF, 5, 0, 0, 0]);
    bytes.extend_from_slice(&0u32.to_le_bytes()); // identifier count

    let dso = DsoFile::parse(&bytes, &t3d()).unwrap();
    assert_eq!(dso.code, vec![5]);

    let written = dso.to_bytes(&t3d());
    assert_eq!(written.len(), bytes.len() - 4);
    assert_eq!(written[32], 5);
}

#[test]
fn long_local_variable_names_are_cut_to_the_one_byte_length() {
    let mut dso = DsoFile::parse(&encode(&empty()), &t3d()).unwrap();
    dso.local_variables.push(LocalVariables {
        function: vec![b'f'; 300],
        names: vec![vec![b'v'; 256]],
    });
    let back = DsoFile::parse(&dso.to_bytes(&t3d()), &t3d()).unwrap();
    assert_eq!(back.local_variables[0].function.len(), MAX_LOCAL_NAME_BYTES);
    assert_eq!(back.local_variables[0].names[0].len(), MAX_LOCAL_NAME_BYTES);
}

#[test]
#[should_panic(expected = "pairs")]
fn odd_line_breaks_cannot_be_written() {
    let mut dso = DsoFile::parse(&encode(&sample()), &t3d()).unwrap();
    dso.line_breaks.push(1);
    dso.to_bytes(&t3d());
}

#[test]
#[should_panic(expected = "no local-variable table")]
fn local_variables_cannot_be_written_in_a_layout_without_them() {
    let dialect = Dialect {
        layout: FileLayout {
            local_variable_table: false,
        },
        ..Dialect::torque3d()
    };
    let dso = DsoFile::parse(&encode(&sample()), &t3d()).unwrap();
    dso.to_bytes(&dialect);
}
