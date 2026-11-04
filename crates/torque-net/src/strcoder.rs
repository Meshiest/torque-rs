//! `BitStream::writeString` / `readString`: the string coder over the Huffman tree.
//!
//! **Without a string buffer** (handshake strings, a blob written outside a packet) a string is [`crate::huffman::Table::write`]'s form: flag,
//! 8-bit length, codes or raw bytes. **With one** (every game packet has a cleared 256-byte buffer on its stream, both ways), `j`
//! is the length of the prefix the string shares with the PREVIOUS string written in the same packet (any string of any event or
//! ghost); the buffer becomes the new string (zero padded); then `flag(j > 2)`; when set, `int(8) j` and the Huffman form of
//! `s[j..]`, else the Huffman form of `s`. The bits of a string therefore depend on what precedes it in the packet.
//!
//! **The reader** mirrors it, as a C string: a string ends at its first NUL. A prefix length that, with its tail, would not fit the
//! 256-byte buffer is an error (a C reader would overrun its buffer there).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `core/stream/bitStream.cpp`
//! (`BitStream::writeString`, `readString`; Copyright (c) 2012 GarageGames, LLC) and `core/stream/bitStream.h` (Copyright (c)
//! 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::NET_STRING_BYTES;
use crate::huffman::Table;

/// `writeString(s, max_len)`: `s` is cut at `max_len` (at most 255) bytes and at its first NUL.
pub fn write_string(w: &mut BitWriter, table: &Table, s: &[u8], max_len: usize) {
    let max = max_len.min(NET_STRING_BYTES);
    let s = s.get(..s.len().min(max)).unwrap_or(&[]);
    let s = s.split(|&b| b == 0).next().unwrap_or(&[]);
    let Some(buffer) = w.string_buffer() else {
        table.write(w, s, max);
        return;
    };
    let mut prefix = 0usize;
    while prefix < max
        && buffer.get(prefix).copied().unwrap_or(0) == s.get(prefix).copied().unwrap_or(0)
        && s.get(prefix).is_some_and(|&b| b != 0)
    {
        prefix += 1;
    }
    for (i, slot) in buffer.iter_mut().enumerate().take(max) {
        *slot = s.get(i).copied().unwrap_or(0);
    }
    if let Some(slot) = buffer.get_mut(max) {
        *slot = 0;
    }
    let shared = prefix > 2;
    w.write_flag(shared);
    if shared {
        w.write_bits(prefix as u32, 8);
        table.write(w, s.get(prefix..).unwrap_or(&[]), max - prefix);
    } else {
        table.write(w, s, max);
    }
}

/// `readString`: the next string of the stream.
pub fn read_string(r: &mut BitReader, table: &Table) -> Result<Vec<u8>, NetError> {
    if r.string_buffer().is_none() {
        return Ok(c_string(table.read(r)?));
    }
    let shared = r.read_flag()?;
    let (offset, tail) = if shared {
        let offset = r.read_bits(8)? as usize;
        (offset, table.read(r)?)
    } else {
        (0, table.read(r)?)
    };
    let tail = c_string(tail);
    let end = offset + tail.len();
    let buffer = r
        .string_buffer()
        .ok_or(NetError::Invalid("string buffer"))?;
    if end >= buffer.len() {
        return Err(NetError::Invalid("string longer than its buffer"));
    }
    buffer[offset..end].copy_from_slice(&tail);
    buffer[end] = 0;
    let whole = &buffer[..];
    let len = whole.iter().position(|&b| b == 0).unwrap_or(whole.len());
    Ok(whole[..len].to_vec())
}

fn c_string(mut bytes: Vec<u8>) -> Vec<u8> {
    if let Some(nul) = bytes.iter().position(|&b| b == 0) {
        bytes.truncate(nul);
    }
    bytes
}

#[cfg(test)]
#[path = "strcoder_tests.rs"]
mod tests;
