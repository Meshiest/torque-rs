//! The string coder's code tree: `HuffmanProcessor::buildTables` and `writeHuffBuffer`/`readHuffBuffer`.
//! The table is built from 256 character frequencies: Torque's standard ones ([`TORQUE_CHAR_FREQS`], [`Table::torque`]) or any a
//! game passes in.
//!
//! **The tree.** Every byte is a leaf of population `frequency + 1`. While more than one wrap is left, the two lowest populations
//! in wrap order are found (a lower population replaces the first only if strictly lower; a tie keeps the earlier), and the new
//! node, whose population is their sum, takes the lower of the two slots; the other slot is filled by the last wrap, which is
//! removed (unless the second was already the last). Node 0 is a placeholder the root is copied into at the end. A 0 bit goes to
//! a node's first child, a 1 bit to its second; a code's first bit is the first one written.
//!
//! **A string.** Cut at 255 bytes and at the first NUL (a C string is measured); when its codes would take fewer bits
//! than the 8 a byte of the raw form takes, a flag 1, the length in 8 bits and one code per byte follow, else a flag 0, the length
//! and the raw bytes. The reader takes the length it finds: at most 255, whatever the caller's `max_len` was.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `core/stream/bitStream.cpp`
//! (`HuffmanProcessor::buildTables`, `writeHuffBuffer`, `readHuffBuffer`; Copyright (c) 2012 GarageGames, LLC); the code here
//! is this crate's own.

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::{self, NET_STRING_BYTES};

/// A leaf's code: the bits in the order they are written (the first is bit 0) and how many.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Code {
    bits: u32,
    count: u8,
}

#[derive(Clone, Copy)]
enum Wrap {
    Leaf(usize),
    Node(usize),
}

/// The code tree and the codes it gives.
#[derive(Debug, Clone)]
pub struct Table {
    /// Node `i`'s children: a value >= 0 is another node, a value < 0 is leaf `-(v + 1)`. Node 0 is the root.
    nodes: Vec<[i32; 2]>,
    codes: [Code; 256],
}

/// Torque's standard character frequency table (`HuffmanProcessor::csm_charFreqs`), the data the string coder of every Torque
/// connection builds its tree from.
///
/// From Torque3D's `Engine/source/core/stream/bitStream.cpp` (the same table as the Torque Game Engine's `bitStream.cc`),
/// Copyright (c) 2012 GarageGames, LLC, under the MIT License: permission is hereby granted, free of charge, to any person
/// obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without
/// restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
/// copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
/// the above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.
/// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED.
#[rustfmt::skip]
pub const TORQUE_CHAR_FREQS: [u32; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 329, 21, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    2809, 68, 0, 27, 0, 58, 3, 62, 4, 7, 0, 0, 15, 65, 554, 3,
    394, 404, 189, 117, 30, 51, 27, 15, 34, 32, 80, 1, 142, 3, 142, 39,
    0, 144, 125, 44, 122, 275, 70, 135, 61, 127, 8, 12, 113, 246, 122, 36,
    185, 1, 149, 309, 335, 12, 11, 14, 54, 151, 0, 0, 2, 0, 0, 211,
    0, 2090, 344, 736, 993, 2872, 701, 605, 646, 1552, 328, 305, 1240, 735, 1533, 1713,
    562, 3, 1775, 1149, 1469, 979, 407, 553, 59, 279, 31, 0, 0, 0, 68, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// The balanced tree: every byte an 8-bit code, so no string is ever compressed. A stand-in for a host that has no table to read.
impl Default for Table {
    fn default() -> Table {
        Table::balanced()
    }
}

impl Table {
    /// The tree of Torque's standard frequencies ([`TORQUE_CHAR_FREQS`]).
    pub fn torque() -> Table {
        static TABLE: std::sync::OnceLock<Option<Table>> = std::sync::OnceLock::new();
        TABLE
            .get_or_init(|| Table::from_frequencies(&TORQUE_CHAR_FREQS).ok())
            .clone()
            .unwrap_or_else(Table::balanced)
    }

    /// `HuffmanProcessor::buildTables` over `frequencies` (each byte's population is its frequency plus one). Fails when a code
    /// would be longer than 32 bits (codes are 32-bit words; the standard table's longest is far shorter).
    pub fn from_frequencies(frequencies: &[u32; 256]) -> Result<Table, NetError> {
        let leaf_pop: Vec<u32> = frequencies.iter().map(|f| f.saturating_add(1)).collect();
        let mut node_pop: Vec<u32> = vec![0];
        let mut nodes: Vec<[i32; 2]> = vec![[0, 0]];
        let mut wraps: Vec<Wrap> = (0..256).map(Wrap::Leaf).collect();
        let pop = |w: Wrap, node_pop: &[u32]| match w {
            Wrap::Leaf(i) => leaf_pop.get(i).copied().unwrap_or(0),
            Wrap::Node(i) => node_pop.get(i).copied().unwrap_or(0),
        };
        let index = |w: Wrap| match w {
            Wrap::Leaf(i) => -(i as i32 + 1),
            Wrap::Node(i) => i as i32,
        };
        while wraps.len() > 1 {
            let (mut min1, mut min2) = (0xffff_fffeu32, 0xffff_ffffu32);
            let (mut index1, mut index2) = (usize::MAX, usize::MAX);
            for (i, w) in wraps.iter().enumerate() {
                let p = pop(*w, &node_pop);
                if p < min1 {
                    min2 = min1;
                    index2 = index1;
                    min1 = p;
                    index1 = i;
                } else if p < min2 {
                    min2 = p;
                    index2 = i;
                }
            }
            let (Some(first), Some(second)) =
                (wraps.get(index1).copied(), wraps.get(index2).copied())
            else {
                return Err(NetError::Invalid("frequency table (saturated populations)"));
            };
            nodes.push([index(first), index(second)]);
            node_pop.push(pop(first, &node_pop).saturating_add(pop(second, &node_pop)));
            let merge = index1.min(index2);
            let nuke = index1.max(index2);
            let made = Wrap::Node(nodes.len() - 1);
            if let Some(slot) = wraps.get_mut(merge) {
                *slot = made;
            }
            let last = wraps.len() - 1;
            if index2 != last {
                wraps.swap(nuke, last);
            }
            wraps.pop();
        }
        if let Some(Wrap::Node(root)) = wraps.first().copied()
            && let Some(copy) = nodes.get(root).copied()
            && let Some(slot) = nodes.first_mut()
        {
            *slot = copy;
        }
        let mut codes = [Code::default(); 256];
        let mut stack: Vec<(i32, u32, u8)> = vec![(0, 0, 0)];
        while let Some((at, bits, depth)) = stack.pop() {
            if at < 0 {
                if let Some(code) = codes.get_mut((-(at + 1)) as usize) {
                    *code = Code { bits, count: depth };
                }
            } else if let Some(children) = nodes.get(at as usize) {
                if depth >= 32 {
                    return Err(NetError::Invalid("frequency table (a code over 32 bits)"));
                }
                stack.push((children[0], bits, depth + 1));
                stack.push((children[1], bits | (1 << depth), depth + 1));
            }
        }
        Ok(Table { nodes, codes })
    }

    /// The balanced tree every byte of which has an 8-bit code (what equal populations give); it cannot fail to build.
    fn balanced() -> Table {
        let mut nodes = vec![[0i32; 2]; 255];
        let mut codes = [Code::default(); 256];
        for (i, children) in nodes.iter_mut().enumerate() {
            let level = usize::BITS - 1 - (i + 1).leading_zeros();
            for (bit, slot) in children.iter_mut().enumerate() {
                if level < 7 {
                    *slot = (2 * i + 1 + bit) as i32;
                } else {
                    // The path from the root is the bits of i + 1 after its leading one, first choice highest.
                    let path = (i + 1 - 128) as u32;
                    let reversed = (0..7).fold(0u32, |acc, k| acc | (((path >> (6 - k)) & 1) << k));
                    let byte = reversed | ((bit as u32) << 7);
                    *slot = -(byte as i32 + 1);
                    if let Some(code) = codes.get_mut(byte as usize) {
                        *code = Code {
                            bits: byte,
                            count: 8,
                        };
                    }
                }
            }
        }
        Table { nodes, codes }
    }

    /// The code of `byte`: its bits (the first written is bit 0) and how many.
    pub fn code(&self, byte: u8) -> (u32, u8) {
        let c = self.codes[usize::from(byte)];
        (c.bits, c.count)
    }

    /// How many bits the codes of `bytes` take together.
    pub fn coded_bits(&self, bytes: &[u8]) -> usize {
        bytes
            .iter()
            .map(|&b| usize::from(self.codes[usize::from(b)].count))
            .sum()
    }

    /// `writeHuffBuffer(s, max_len)`: the string cut at `max_len` (at most 255) bytes and at its first NUL, then the flag, the
    /// length and the codes or the raw bytes.
    pub fn write(&self, w: &mut BitWriter, s: &[u8], max_len: usize) {
        let s = s
            .get(..s.len().min(max_len).min(NET_STRING_BYTES))
            .unwrap_or(&[]);
        let s = s.split(|&b| b == 0).next().unwrap_or(&[]);
        let coded = self.coded_bits(s);
        let compressed = coded < s.len() * 8;
        w.write_flag(compressed);
        w.write_bits(s.len() as u32, 8);
        if compressed {
            for &b in s {
                let (bits, count) = self.code(b);
                w.write_bits(bits, count);
            }
        } else {
            w.write_bytes(s);
        }
    }

    /// `readHuffBuffer`: the bytes of the next string (they may hold NULs; [`crate::strcoder`] cuts a C string at the first).
    pub fn read(&self, r: &mut BitReader) -> Result<Vec<u8>, NetError> {
        let compressed = r.read_flag()?;
        let len = r.read_bits(8)? as usize;
        let min_bits = if compressed { 1 } else { 8 };
        let mut out = cap::vec_for(
            "string",
            len as u64,
            NET_STRING_BYTES,
            r.bits_left(),
            min_bits,
        )?;
        for _ in 0..len {
            if compressed {
                let mut at = 0i32;
                while at >= 0 {
                    let children = self
                        .nodes
                        .get(at as usize)
                        .ok_or(NetError::Invalid("code tree"))?;
                    at = children[usize::from(r.read_flag()?)];
                }
                out.push(u8::try_from(-(at + 1)).map_err(|_| NetError::Invalid("code tree"))?);
            } else {
                out.push(r.read_u8()?);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "huffman_tests.rs"]
pub(crate) mod tests;
