#![allow(dead_code)]

use torque_ts_bytecode::Dialect;

/// The fields of a DSO file as plain values, for the independent test encoder.
#[derive(Clone)]
pub struct Sample {
    pub version: u32,
    pub global_strings: Vec<u8>,
    pub global_floats: Vec<f64>,
    pub function_strings: Vec<u8>,
    pub function_floats: Vec<f64>,
    pub local_variables: Vec<(Vec<u8>, Vec<Vec<u8>>)>,
    pub code: Vec<u32>,
    pub line_breaks: Vec<u32>,
    pub identifiers: Vec<(u32, Vec<u32>)>,
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_floats(out: &mut Vec<u8>, floats: &[f64]) {
    put_u32(out, floats.len() as u32);
    for f in floats {
        out.extend_from_slice(&f.to_le_bytes());
    }
}

fn put_strings(out: &mut Vec<u8>, strings: &[u8]) {
    put_u32(out, strings.len() as u32);
    out.extend_from_slice(strings);
}

fn put_short(out: &mut Vec<u8>, text: &[u8]) {
    out.push(text.len() as u8);
    out.extend_from_slice(text);
}

/// The code, line breaks and identifier table, which every layout writes last.
fn put_tail(out: &mut Vec<u8>, s: &Sample) {
    put_u32(out, s.code.len() as u32);
    put_u32(out, (s.line_breaks.len() / 2) as u32);
    for &w in &s.code {
        if w < 0xFF {
            out.push(w as u8);
        } else {
            out.push(0xFF);
            put_u32(out, w);
        }
    }
    for &w in &s.line_breaks {
        put_u32(out, w);
    }
    put_u32(out, s.identifiers.len() as u32);
    for (index, positions) in &s.identifiers {
        put_u32(out, *index);
        put_u32(out, positions.len() as u32);
        for &p in positions {
            put_u32(out, p);
        }
    }
}

/// Encodes a sample in Torque3D's layout, with code that does not share any logic with the library's writer.
pub fn encode(s: &Sample) -> Vec<u8> {
    let mut out = Vec::new();
    put_u32(&mut out, s.version);
    put_strings(&mut out, &s.global_strings);
    put_strings(&mut out, &s.function_strings);
    put_floats(&mut out, &s.global_floats);
    put_floats(&mut out, &s.function_floats);
    put_u32(&mut out, s.local_variables.len() as u32);
    for (function, names) in &s.local_variables {
        put_short(&mut out, function);
        put_u32(&mut out, names.len() as u32);
        for n in names {
            put_short(&mut out, n);
        }
    }
    put_tail(&mut out, s);
    out
}

/// Encodes a sample without a local-variable table.
pub fn encode_without_local_variables(s: &Sample) -> Vec<u8> {
    let mut out = Vec::new();
    put_u32(&mut out, s.version);
    put_strings(&mut out, &s.global_strings);
    put_strings(&mut out, &s.function_strings);
    put_floats(&mut out, &s.global_floats);
    put_floats(&mut out, &s.function_floats);
    put_tail(&mut out, s);
    out
}

/// A small, valid file exercising every section: strings, floats, local variables, escaped code words, line
/// breaks and identifiers.
pub fn sample() -> Sample {
    Sample {
        version: 50,
        global_strings: b"\0hello\0world\0".to_vec(),
        global_floats: vec![1.5, -2.0],
        function_strings: b"fn\0".to_vec(),
        function_floats: vec![0.25],
        local_variables: vec![(b"(null)::f".to_vec(), vec![b"%a".to_vec(), b"%b".to_vec()])],
        code: vec![0x0f, 5, 0xFE, 0xFF, 0x100, 0xDEAD_BEEF, 0x48],
        line_breaks: vec![1, 0, 2, 3],
        identifiers: vec![(1, vec![1, 4]), (7, vec![2])],
    }
}

pub fn empty() -> Sample {
    Sample {
        version: 50,
        global_strings: vec![],
        global_floats: vec![],
        function_strings: vec![],
        function_floats: vec![],
        local_variables: vec![],
        code: vec![],
        line_breaks: vec![],
        identifiers: vec![],
    }
}

pub fn t3d() -> Dialect {
    Dialect::torque3d()
}
