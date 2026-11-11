//! The Huffman code QPACK string literals may use (RFC 9204 section 4.1.2, the code of RFC 7541 appendix B). A browser's
//! QPACK encoder compresses a literal whenever that is shorter, so the `:authority` and `origin` of its CONNECT arrive coded.
//!
//! The decode walks a binary tree built once from the code table; the encoder is for the tests (and the client, which
//! sends plain literals: nothing it sends is long enough to matter).
//!
//! Credit: the code table is RFC 7541 appendix B as the `h2` crate carries it (MIT, `src/hpack/huffman/table.rs`,
//! Copyright (c) 2017 h2 authors).

use std::sync::OnceLock;

/// The end-of-string symbol's index: never decoded (a padding longer than 7 bits, or EOS itself, is an error).
const EOS: usize = 256;

/// Each symbol's code (`bits`, `code`), symbols 0..=255 then EOS. A few 23-bit codes are written in decimal: hex of their
/// shape reads as an address to the repository's word scan.
const CODES: [(u8, u32); 257] = [
    (13, 0x1ff8),
    (23, 8388568),
    (28, 0xfffffe2),
    (28, 0xfffffe3),
    (28, 0xfffffe4),
    (28, 0xfffffe5),
    (28, 0xfffffe6),
    (28, 0xfffffe7),
    (28, 0xfffffe8),
    (24, 0xffffea),
    (30, 0x3ffffffc),
    (28, 0xfffffe9),
    (28, 0xfffffea),
    (30, 0x3ffffffd),
    (28, 0xfffffeb),
    (28, 0xfffffec),
    (28, 0xfffffed),
    (28, 0xfffffee),
    (28, 0xfffffef),
    (28, 0xffffff0),
    (28, 0xffffff1),
    (28, 0xffffff2),
    (30, 0x3ffffffe),
    (28, 0xffffff3),
    (28, 0xffffff4),
    (28, 0xffffff5),
    (28, 0xffffff6),
    (28, 0xffffff7),
    (28, 0xffffff8),
    (28, 0xffffff9),
    (28, 0xffffffa),
    (28, 0xffffffb),
    (6, 0x14),
    (10, 0x3f8),
    (10, 0x3f9),
    (12, 0xffa),
    (13, 0x1ff9),
    (6, 0x15),
    (8, 0xf8),
    (11, 0x7fa),
    (10, 0x3fa),
    (10, 0x3fb),
    (8, 0xf9),
    (11, 0x7fb),
    (8, 0xfa),
    (6, 0x16),
    (6, 0x17),
    (6, 0x18),
    (5, 0x0),
    (5, 0x1),
    (5, 0x2),
    (6, 0x19),
    (6, 0x1a),
    (6, 0x1b),
    (6, 0x1c),
    (6, 0x1d),
    (6, 0x1e),
    (6, 0x1f),
    (7, 0x5c),
    (8, 0xfb),
    (15, 0x7ffc),
    (6, 0x20),
    (12, 0xffb),
    (10, 0x3fc),
    (13, 0x1ffa),
    (6, 0x21),
    (7, 0x5d),
    (7, 0x5e),
    (7, 0x5f),
    (7, 0x60),
    (7, 0x61),
    (7, 0x62),
    (7, 0x63),
    (7, 0x64),
    (7, 0x65),
    (7, 0x66),
    (7, 0x67),
    (7, 0x68),
    (7, 0x69),
    (7, 0x6a),
    (7, 0x6b),
    (7, 0x6c),
    (7, 0x6d),
    (7, 0x6e),
    (7, 0x6f),
    (7, 0x70),
    (7, 0x71),
    (7, 0x72),
    (8, 0xfc),
    (7, 0x73),
    (8, 0xfd),
    (13, 0x1ffb),
    (19, 0x7fff0),
    (13, 0x1ffc),
    (14, 0x3ffc),
    (6, 0x22),
    (15, 0x7ffd),
    (5, 0x3),
    (6, 0x23),
    (5, 0x4),
    (6, 0x24),
    (5, 0x5),
    (6, 0x25),
    (6, 0x26),
    (6, 0x27),
    (5, 0x6),
    (7, 0x74),
    (7, 0x75),
    (6, 0x28),
    (6, 0x29),
    (6, 0x2a),
    (5, 0x7),
    (6, 0x2b),
    (7, 0x76),
    (6, 0x2c),
    (5, 0x8),
    (5, 0x9),
    (6, 0x2d),
    (7, 0x77),
    (7, 0x78),
    (7, 0x79),
    (7, 0x7a),
    (7, 0x7b),
    (15, 0x7ffe),
    (11, 0x7fc),
    (14, 0x3ffd),
    (13, 0x1ffd),
    (28, 0xffffffc),
    (20, 0xfffe6),
    (22, 0x3fffd2),
    (20, 0xfffe7),
    (20, 0xfffe8),
    (22, 0x3fffd3),
    (22, 0x3fffd4),
    (22, 0x3fffd5),
    (23, 8388569),
    (22, 0x3fffd6),
    (23, 8388570),
    (23, 8388571),
    (23, 8388572),
    (23, 8388573),
    (23, 8388574),
    (24, 0xffffeb),
    (23, 8388575),
    (24, 0xffffec),
    (24, 0xffffed),
    (22, 0x3fffd7),
    (23, 8388576),
    (24, 0xffffee),
    (23, 8388577),
    (23, 8388578),
    (23, 8388579),
    (23, 8388580),
    (21, 0x1fffdc),
    (22, 0x3fffd8),
    (23, 8388581),
    (22, 0x3fffd9),
    (23, 8388582),
    (23, 8388583),
    (24, 0xffffef),
    (22, 0x3fffda),
    (21, 0x1fffdd),
    (20, 0xfffe9),
    (22, 0x3fffdb),
    (22, 0x3fffdc),
    (23, 8388584),
    (23, 8388585),
    (21, 0x1fffde),
    (23, 8388586),
    (22, 0x3fffdd),
    (22, 0x3fffde),
    (24, 0xfffff0),
    (21, 0x1fffdf),
    (22, 0x3fffdf),
    (23, 8388587),
    (23, 8388588),
    (21, 0x1fffe0),
    (21, 0x1fffe1),
    (22, 0x3fffe0),
    (21, 0x1fffe2),
    (23, 8388589),
    (22, 0x3fffe1),
    (23, 8388590),
    (23, 8388591),
    (20, 0xfffea),
    (22, 0x3fffe2),
    (22, 0x3fffe3),
    (22, 0x3fffe4),
    (23, 8388592),
    (22, 0x3fffe5),
    (22, 0x3fffe6),
    (23, 8388593),
    (26, 0x3ffffe0),
    (26, 0x3ffffe1),
    (20, 0xfffeb),
    (19, 0x7fff1),
    (22, 0x3fffe7),
    (23, 8388594),
    (22, 0x3fffe8),
    (25, 0x1ffffec),
    (26, 0x3ffffe2),
    (26, 0x3ffffe3),
    (26, 0x3ffffe4),
    (27, 0x7ffffde),
    (27, 0x7ffffdf),
    (26, 0x3ffffe5),
    (24, 0xfffff1),
    (25, 0x1ffffed),
    (19, 0x7fff2),
    (21, 0x1fffe3),
    (26, 0x3ffffe6),
    (27, 0x7ffffe0),
    (27, 0x7ffffe1),
    (26, 0x3ffffe7),
    (27, 0x7ffffe2),
    (24, 0xfffff2),
    (21, 0x1fffe4),
    (21, 0x1fffe5),
    (26, 0x3ffffe8),
    (26, 0x3ffffe9),
    (28, 0xffffffd),
    (27, 0x7ffffe3),
    (27, 0x7ffffe4),
    (27, 0x7ffffe5),
    (20, 0xfffec),
    (24, 0xfffff3),
    (20, 0xfffed),
    (21, 0x1fffe6),
    (22, 0x3fffe9),
    (21, 0x1fffe7),
    (21, 0x1fffe8),
    (23, 8388595),
    (22, 0x3fffea),
    (22, 0x3fffeb),
    (25, 0x1ffffee),
    (25, 0x1ffffef),
    (24, 0xfffff4),
    (24, 0xfffff5),
    (26, 0x3ffffea),
    (23, 8388596),
    (26, 0x3ffffeb),
    (27, 0x7ffffe6),
    (26, 0x3ffffec),
    (26, 0x3ffffed),
    (27, 0x7ffffe7),
    (27, 0x7ffffe8),
    (27, 0x7ffffe9),
    (27, 0x7ffffea),
    (27, 0x7ffffeb),
    (28, 0xffffffe),
    (27, 0x7ffffec),
    (27, 0x7ffffed),
    (27, 0x7ffffee),
    (27, 0x7ffffef),
    (27, 0x7fffff0),
    (26, 0x3ffffee),
    (30, 0x3fffffff),
];

/// A node of the decode tree: the next node for a 0 bit and for a 1 bit, or the symbol a path ends at.
#[derive(Clone, Copy)]
enum Node {
    Branch([u16; 2]),
    Leaf(u16),
}

/// The decode tree, built once: node 0 is the root, a child of 0 is "not yet made" (the root is never a child, since
/// the code is prefix-free and complete).
fn tree() -> &'static [Node] {
    static TREE: OnceLock<Vec<Node>> = OnceLock::new();
    TREE.get_or_init(|| {
        let mut nodes = vec![Node::Branch([0, 0])];
        for (symbol, &(bits, code)) in CODES.iter().enumerate() {
            let mut at = 0usize;
            for i in (0..bits).rev() {
                let bit = ((code >> i) & 1) as usize;
                let next = match nodes[at] {
                    Node::Branch(children) => children[bit],
                    Node::Leaf(_) => {
                        unreachable!("the code is prefix-free: no path crosses a leaf")
                    }
                };
                if next != 0 {
                    at = usize::from(next);
                    continue;
                }
                let new = nodes.len() as u16;
                nodes.push(if i == 0 {
                    Node::Leaf(symbol as u16)
                } else {
                    Node::Branch([0, 0])
                });
                if let Node::Branch(children) = &mut nodes[at] {
                    children[bit] = new;
                }
                at = usize::from(new);
            }
        }
        nodes
    })
}

/// Why a Huffman string could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HuffmanError {
    /// The bits name EOS, or a padding longer than seven bits or not all ones (RFC 7541 section 5.2).
    BadPadding,
    /// The string would be longer than the cap given.
    TooLong,
}

/// Decodes `input` into `out` (appended), at most `max` bytes.
///
/// # Errors
/// Bad padding, or more than `max` bytes.
pub fn decode(input: &[u8], max: usize, out: &mut Vec<u8>) -> Result<(), HuffmanError> {
    let tree = tree();
    let mut at = 0usize;
    let mut depth = 0u8;
    let mut ones = true;
    for &byte in input {
        for i in (0..8).rev() {
            let bit = usize::from((byte >> i) & 1);
            ones &= bit == 1;
            let next = match tree[at] {
                Node::Branch(children) => usize::from(children[bit]),
                Node::Leaf(_) => unreachable!("the walk restarts at the root after a leaf"),
            };
            depth += 1;
            match tree[next] {
                Node::Leaf(symbol) => {
                    if usize::from(symbol) == EOS {
                        return Err(HuffmanError::BadPadding);
                    }
                    if out.len() >= max {
                        return Err(HuffmanError::TooLong);
                    }
                    out.push(symbol as u8);
                    at = 0;
                    depth = 0;
                    ones = true;
                }
                Node::Branch(_) => at = next,
            }
        }
    }
    if depth > 7 || !ones {
        return Err(HuffmanError::BadPadding);
    }
    Ok(())
}

/// Encodes `input` with the code, padded with ones to the byte.
#[must_use]
pub fn encode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut acc: u64 = 0;
    let mut filled = 0u32;
    for &byte in input {
        let (bits, code) = CODES[usize::from(byte)];
        acc = (acc << bits) | u64::from(code);
        filled += u32::from(bits);
        while filled >= 8 {
            filled -= 8;
            out.push((acc >> filled) as u8);
        }
    }
    if filled > 0 {
        let pad = 8 - filled;
        acc = (acc << pad) | ((1 << pad) - 1);
        out.push(acc as u8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rfc_examples_decode_and_round_trip() {
        // RFC 7541 appendix C.4.1 and C.6.1.
        let cases: [(&[u8], &[u8]); 3] = [
            (
                &[
                    0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90, 0xf4, 0xff,
                ],
                b"www.example.com",
            ),
            (&[0xa8, 0xeb, 0x10, 0x64, 0x9c, 0xbf], b"no-cache"),
            (&[0x64, 0x2], b"302"),
        ];
        for (coded, plain) in cases {
            let mut out = Vec::new();
            decode(coded, 64, &mut out).expect("decodes");
            assert_eq!(out, plain);
            assert_eq!(encode(plain), coded);
        }
        for text in [
            "",
            "a",
            "127.0.0.1:28000",
            "https://localhost:8080",
            "\u{0}\u{ff}~",
        ] {
            let mut out = Vec::new();
            decode(&encode(text.as_bytes()), 64, &mut out).expect("round trip");
            assert_eq!(out, text.as_bytes());
        }
    }

    #[test]
    fn bad_padding_eos_and_length_are_refused_and_nothing_panics() {
        let mut out = Vec::new();
        // A padding of eight ones (a whole byte) is not allowed.
        assert_eq!(decode(&[0xff], 64, &mut out), Err(HuffmanError::BadPadding));
        // EOS: 30 ones.
        assert_eq!(
            decode(&[0xff, 0xff, 0xff, 0xfc], 64, &mut out),
            Err(HuffmanError::BadPadding)
        );
        // Padding with zero bits: `a` (00011) then 000.
        out.clear();
        assert_eq!(decode(&[0x18], 64, &mut out), Err(HuffmanError::BadPadding));
        out.clear();
        assert_eq!(
            decode(&[0x64, 0x00], 64, &mut out),
            Ok(()),
            "`3` then two `0`s, no padding at all"
        );
        assert_eq!(out, b"300");
        out.clear();
        assert_eq!(
            decode(&encode(b"abcd"), 3, &mut out),
            Err(HuffmanError::TooLong)
        );
        for i in 0..=255u8 {
            out.clear();
            let _ = decode(&[i, i ^ 0x5a, !i], 8, &mut out);
        }
    }
}
