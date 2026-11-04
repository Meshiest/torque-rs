//! A lossless, readable text form of a [`DsoFile`]: the compiler's tests store expected output in it, so a
//! difference shows as a readable diff and the bytes can still be rebuilt exactly.
//!
//! ```text
//! dso 50
//! global-strings 7
//!   0 "print"
//!   6 ""
//! global-floats 1
//!   0 3ff8000000000000 1.5
//! function-strings 0
//! function-floats 0
//! local-variables 1
//!   "(null)::f" "%a" "%b"
//! code 10
//!   000000 52 0 | OP_PUSH_FRAME 0
//! # (the other instructions)
//! line-breaks 0
//! identifiers 1
//!   0 3
//! ```
//!
//! A string table is its length in bytes, then one line per NUL-terminated piece (offset and text; the zero
//! padding of a short tagged string shows as empty pieces); a float table its count, then index, IEEE bits and
//! value; the code its word count, then one line per instruction (address, its words in hex, `|` and the
//! decoded operation); the local-variable table its entry count, then one line per entry (the function's name and
//! its variables' names, quoted); the line-break table its pair count, then pairs (line, address); the
//! identifier table its entry count, then the string offset and the code positions. Only the numbers and the quoted
//! texts are read back; text after `|` and lines starting with `#` are commentary.

use std::fmt::Write as _;

use crate::decode::{Operand, OperandKind, Table, decode_dso};
use crate::dialect::Dialect;
use crate::dso::{DsoFile, IdentEntry, LocalVariables, StringTable};

/// The most a count read from the text reserves up front: a header is not trusted to size an allocation.
const MAX_PREALLOCATED: usize = 1 << 16;

fn escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push('"');
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "\\x{b:02X}");
            }
        }
    }
    out.push('"');
    out
}

fn unescape(text: &str) -> Result<Vec<u8>, String> {
    let inner = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .ok_or_else(|| format!("not a quoted string: {text}"))?;
    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b != b'\\' {
            out.push(b);
            i += 1;
            continue;
        }
        match bytes.get(i + 1) {
            Some(b'"') => out.push(b'"'),
            Some(b'\\') => out.push(b'\\'),
            Some(b'x') => {
                let hex = inner.get(i + 2..i + 4).ok_or("short \\x escape")?;
                if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(format!("bad hex escape in {text}"));
                }
                out.push(u8::from_str_radix(hex, 16).map_err(|e| e.to_string())?);
                i += 2;
            }
            _ => return Err(format!("bad escape in {text}")),
        }
        i += 2;
    }
    Ok(out)
}

fn render_strings(out: &mut String, title: &str, raw: &[u8]) {
    let table = StringTable::new(raw);
    let bytes = table.as_bytes();
    let _ = writeln!(out, "{title} {}", bytes.len());
    let mut start = 0;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|&b| b == 0)
            .map_or(bytes.len(), |p| start + p);
        let tail = if end == bytes.len() {
            " unterminated"
        } else {
            ""
        };
        let _ = writeln!(out, "  {start} {}{tail}", escape(&bytes[start..end]));
        start = end + 1;
    }
}

fn render_floats(out: &mut String, title: &str, floats: &[f64]) {
    let _ = writeln!(out, "{title} {}", floats.len());
    for (i, f) in floats.iter().enumerate() {
        let _ = writeln!(out, "  {i} {:016x} {f:?}", f.to_bits());
    }
}

fn operand_text(
    dso: &DsoFile,
    operand: &Operand,
    names: &std::collections::HashMap<u32, Vec<u8>>,
) -> String {
    match operand.kind {
        OperandKind::Ident => names
            .get(&operand.address)
            .map_or_else(|| "-".to_string(), |n| escape(n)),
        OperandKind::StringIndex(table) => {
            let t = match table {
                Table::Global => dso.global_string_table(),
                Table::Function => dso.function_string_table(),
            };
            t.get(operand.word)
                .map_or_else(|| format!("@{}", operand.word), escape)
        }
        _ => operand.word.to_string(),
    }
}

/// The text form of `dso`, a file of the given dialect.
pub fn render(dso: &DsoFile, dialect: &Dialect) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "dso {}", dso.version);
    render_strings(&mut out, "global-strings", &dso.global_strings);
    render_floats(&mut out, "global-floats", &dso.global_floats);
    render_strings(&mut out, "function-strings", &dso.function_strings);
    render_floats(&mut out, "function-floats", &dso.function_floats);
    let _ = writeln!(out, "local-variables {}", dso.local_variables.len());
    for entry in &dso.local_variables {
        let mut line = escape(&entry.function);
        for name in &entry.names {
            line.push(' ');
            line.push_str(&escape(name));
        }
        let _ = writeln!(out, "  {line}");
    }
    let _ = writeln!(out, "code {}", dso.code.len());
    let globals = dso.global_string_table();
    let mut names = std::collections::HashMap::new();
    for e in &dso.identifiers {
        let name = globals.get(e.string_index).unwrap_or_default().to_vec();
        for &p in &e.positions {
            names.insert(p, name.clone());
        }
    }
    match decode_dso(dso, dialect) {
        Ok(instructions) => {
            for ins in &instructions {
                let start = ins.address as usize;
                let words = dso
                    .code
                    .get(start..start + ins.words() as usize)
                    .unwrap_or_default();
                let hex: Vec<String> = words.iter().map(|w| format!("{w:x}")).collect();
                let ops: Vec<String> = ins
                    .operands
                    .iter()
                    .map(|o| operand_text(dso, o, &names))
                    .collect();
                let _ = writeln!(
                    out,
                    "  {:06} {} | {} {}",
                    ins.address,
                    hex.join(" "),
                    ins.op.name(),
                    ops.join(" ")
                );
            }
        }
        Err(e) => {
            for (i, chunk) in dso.code.chunks(8).enumerate() {
                let hex: Vec<String> = chunk.iter().map(|w| format!("{w:x}")).collect();
                let _ = writeln!(out, "  {:06} {} | (not decoded: {e})", i * 8, hex.join(" "));
            }
        }
    }
    let _ = writeln!(out, "line-breaks {}", dso.line_breaks.len() / 2);
    for pair in dso.line_breaks.chunks(2) {
        let _ = writeln!(
            out,
            "  {}",
            pair.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    let _ = writeln!(out, "identifiers {}", dso.identifiers.len());
    for e in &dso.identifiers {
        let positions: Vec<String> = e.positions.iter().map(u32::to_string).collect();
        let _ = writeln!(out, "  {} {}", e.string_index, positions.join(" "));
    }
    out
}

/// Reads the text form back. Errors name the line.
pub fn parse(text: &str) -> Result<DsoFile, String> {
    let all: Vec<(usize, String)> = text
        .lines()
        .enumerate()
        .map(|(n, l)| (n + 1, l.trim_end().to_string()))
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
        .collect();
    let mut pos = 0;
    let mut next = |want: &str| -> Result<(usize, Vec<(usize, String)>), String> {
        let (n, line) = all.get(pos).ok_or_else(|| format!("missing `{want}`"))?;
        let count = line
            .strip_prefix(want)
            .ok_or_else(|| format!("line {n}: expected `{want}`"))?
            .trim()
            .parse::<usize>()
            .map_err(|e| format!("line {n}: {e}"))?;
        pos += 1;
        let mut body = Vec::new();
        while let Some((m, l)) = all.get(pos) {
            if !l.starts_with(' ') {
                break;
            }
            body.push((*m, l.trim().to_string()));
            pos += 1;
        }
        Ok((count, body))
    };
    let (version, _) = next("dso")?;
    let strings = |(len, body): (usize, Vec<(usize, String)>)| -> Result<Vec<u8>, String> {
        let mut bytes = Vec::with_capacity(len.min(MAX_PREALLOCATED));
        for (n, l) in body {
            let (_, quoted) = l
                .split_once(' ')
                .ok_or_else(|| format!("line {n}: no string"))?;
            let unterminated = quoted.ends_with(" unterminated");
            let quoted = quoted.trim_end_matches(" unterminated");
            bytes.extend(unescape(quoted).map_err(|e| format!("line {n}: {e}"))?);
            if !unterminated {
                bytes.push(0);
            }
        }
        if bytes.len() != len {
            return Err(format!(
                "string table is {} bytes, header says {len}",
                bytes.len()
            ));
        }
        Ok(bytes)
    };
    let floats = |(count, body): (usize, Vec<(usize, String)>)| -> Result<Vec<f64>, String> {
        let mut out = Vec::with_capacity(count.min(MAX_PREALLOCATED));
        for (n, l) in body {
            let bits = l
                .split_whitespace()
                .nth(1)
                .ok_or_else(|| format!("line {n}: no bits"))?;
            out.push(f64::from_bits(
                u64::from_str_radix(bits, 16).map_err(|e| format!("line {n}: {e}"))?,
            ));
        }
        if out.len() != count {
            return Err(format!("{} floats, header says {count}", out.len()));
        }
        Ok(out)
    };
    let global_strings = strings(next("global-strings")?)?;
    let global_floats = floats(next("global-floats")?)?;
    let function_strings = strings(next("function-strings")?)?;
    let function_floats = floats(next("function-floats")?)?;
    let (count, body) = next("local-variables")?;
    let mut local_variables = Vec::with_capacity(count.min(MAX_PREALLOCATED));
    for (n, l) in body {
        let mut quoted = split_quoted(&l)
            .map_err(|e| format!("line {n}: {e}"))?
            .into_iter();
        let function = quoted.next().ok_or_else(|| format!("line {n}: empty"))?;
        local_variables.push(LocalVariables {
            function,
            names: quoted.collect(),
        });
    }
    if local_variables.len() != count {
        return Err(format!(
            "{} local-variable entries, header says {count}",
            local_variables.len()
        ));
    }
    let (count, body) = next("code")?;
    let mut code = Vec::with_capacity(count.min(MAX_PREALLOCATED));
    for (n, l) in body {
        let words = l.split('|').next().unwrap_or("");
        for w in words.split_whitespace().skip(1) {
            code.push(u32::from_str_radix(w, 16).map_err(|e| format!("line {n}: {e}"))?);
        }
    }
    if code.len() != count {
        return Err(format!("{} code words, header says {count}", code.len()));
    }
    let (count, body) = next("line-breaks")?;
    let mut line_breaks = Vec::with_capacity(count.min(MAX_PREALLOCATED).saturating_mul(2));
    for (n, l) in body {
        for w in l.split_whitespace() {
            line_breaks.push(w.parse::<u32>().map_err(|e| format!("line {n}: {e}"))?);
        }
    }
    if Some(line_breaks.len()) != count.checked_mul(2) {
        return Err(format!(
            "{} line-break words, header says {count} pairs",
            line_breaks.len()
        ));
    }
    let (count, body) = next("identifiers")?;
    let mut identifiers = Vec::with_capacity(count.min(MAX_PREALLOCATED));
    for (n, l) in body {
        let mut nums = l
            .split_whitespace()
            .map(|w| w.parse::<u32>().map_err(|e| format!("line {n}: {e}")));
        let string_index = nums.next().ok_or_else(|| format!("line {n}: empty"))??;
        let positions = nums.collect::<Result<Vec<u32>, String>>()?;
        identifiers.push(IdentEntry {
            string_index,
            positions,
        });
    }
    if identifiers.len() != count {
        return Err(format!(
            "{} identifier entries, header says {count}",
            identifiers.len()
        ));
    }
    Ok(DsoFile {
        version: u32::try_from(version).map_err(|e| e.to_string())?,
        global_strings,
        global_floats,
        function_strings,
        function_floats,
        local_variables,
        code,
        line_breaks,
        identifiers,
    })
}

/// The quoted strings of a line, separated by single spaces.
fn split_quoted(line: &str) -> Result<Vec<Vec<u8>>, String> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            return Err(format!("not a quoted string: {line}"));
        }
        let mut j = i + 1;
        while j < bytes.len() && bytes[j] != b'"' {
            j += if bytes[j] == b'\\' { 2 } else { 1 };
        }
        let piece = line.get(i..=j).ok_or("unterminated string")?;
        out.push(unescape(piece)?);
        i = j + 1;
        if i < bytes.len() {
            if bytes[i] != b' ' {
                return Err(format!("strings must be separated by one space: {line}"));
            }
            i += 1;
        }
    }
    Ok(out)
}
