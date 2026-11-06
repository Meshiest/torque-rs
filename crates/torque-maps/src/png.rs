//! A PNG inside a stream (`GBitmap::readBitmap("png", stream)`): the reader finds where it ends by its chunks, from the signature
//! to the end of the `IEND` chunk, checking each chunk's length and CRC, and keeps the bytes undecoded (the drawing code decodes
//! them with `images`). libpng reads a stream chunk by chunk with no read-ahead, so the next field starts right after `IEND`.

use crate::MapError;
use crate::read::Reader;

/// The eight bytes every PNG starts with.
pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
/// The most chunks of one embedded PNG (a lightmap has a handful).
pub const MAX_CHUNKS: usize = 4_096;
/// PNG's own limit on a chunk length.
const MAX_CHUNK_LEN: u32 = 0x7fff_ffff;

/// A PNG's bytes as they are in the file, with the size from its header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Png {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

/// The CRC-32 of PNG (ISO 3309, reflected polynomial 0xEDB88320), eight bytes a step ([`CRC_TABLES`]: the same value as one
/// byte a step, [`CRC_TABLE`]).
pub fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for part in parts {
        let (words, rest) = part.as_chunks::<8>();
        for w in words {
            let one = u32::from_le_bytes([w[0], w[1], w[2], w[3]]) ^ c;
            let two = u32::from_le_bytes([w[4], w[5], w[6], w[7]]);
            let t = &CRC_TABLES;
            c = t[7][(one & 0xff) as usize]
                ^ t[6][((one >> 8) & 0xff) as usize]
                ^ t[5][((one >> 16) & 0xff) as usize]
                ^ t[4][(one >> 24) as usize]
                ^ t[3][(two & 0xff) as usize]
                ^ t[2][((two >> 8) & 0xff) as usize]
                ^ t[1][((two >> 16) & 0xff) as usize]
                ^ t[0][(two >> 24) as usize];
        }
        for &b in rest {
            c = CRC_TABLE[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
        }
    }
    !c
}

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
};

/// [`CRC_TABLE`] and seven more: table `k` is a byte's CRC with `k` zero bytes after it (slicing by eight).
const CRC_TABLES: [[u32; 256]; 8] = {
    let mut tables = [CRC_TABLE; 8];
    let mut k = 1;
    while k < 8 {
        let mut n = 0;
        while n < 256 {
            let c = tables[k - 1][n];
            tables[k][n] = (c >> 8) ^ CRC_TABLE[(c & 0xff) as usize];
            n += 1;
        }
        k += 1;
    }
    tables
};

pub(crate) fn read(r: &mut Reader, what: &'static str) -> Result<Png, MapError> {
    let start = r.pos();
    if r.take(8, what)? != SIGNATURE {
        return Err(MapError::Bad {
            what,
            value: 0,
            at: start,
        });
    }
    let mut size = None;
    for chunk in 0..MAX_CHUNKS {
        let at = r.pos();
        let len = u32::from_be_bytes(r.take(4, what)?.try_into().unwrap_or([0xff; 4]));
        if len > MAX_CHUNK_LEN {
            return Err(MapError::Bad {
                what,
                value: len as u64,
                at,
            });
        }
        let kind = r.take(4, what)?;
        let data = r.take(len as usize, what)?;
        let crc = u32::from_be_bytes(r.take(4, what)?.try_into().unwrap_or([0; 4]));
        if crc != crc32(&[kind, data]) {
            return Err(MapError::Bad {
                what,
                value: crc as u64,
                at,
            });
        }
        if chunk == 0 {
            if kind != b"IHDR" || data.len() != 13 {
                return Err(MapError::Bad {
                    what,
                    value: len as u64,
                    at,
                });
            }
            let w = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            let h = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
            size = Some((w, h));
        }
        if kind == b"IEND" {
            let end = r.pos();
            r.seek(start);
            let bytes = r.bytes_copy(end - start, what)?;
            let (width, height) = size.unwrap_or((0, 0));
            return Ok(Png {
                width,
                height,
                bytes,
            });
        }
    }
    Err(MapError::Count {
        what,
        value: MAX_CHUNKS as u64 + 1,
        cap: MAX_CHUNKS as u64,
    })
}

/// An 8-bit RGB PNG of `width` x `height` from `rgb` (rows top first, three bytes a pixel; missing bytes are black). Its image
/// data is one fixed-Huffman deflate block whose only matches repeat the byte before (runs: a flat lightmap is a few hundred
/// bytes); what a writer needs without a compressor.
pub fn encode_rgb(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    let row = width as usize * 3;
    let mut raw = Vec::new();
    for y in 0..height as usize {
        raw.push(0);
        let start = (y * row).min(rgb.len());
        let end = (start + row).min(rgb.len());
        raw.extend_from_slice(&rgb[start..end]);
        raw.resize(raw.len() + row - (end - start), 0);
    }
    let mut z = Vec::from([0x78, 0x01]);
    deflate_runs(&raw, &mut z);
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = Vec::from(SIGNATURE);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Bits into bytes, least significant first (deflate's order).
struct Bits<'a> {
    out: &'a mut Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits<'_> {
    /// `n` bits of `v`, low first.
    fn put(&mut self, v: u32, n: u32) {
        self.acc |= v << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// A Huffman code of `n` bits, its first bit the code's highest.
    fn code(&mut self, v: u32, n: u32) {
        self.put(v.reverse_bits() >> (32 - n), n);
    }

    /// A literal or length symbol of the fixed code.
    fn symbol(&mut self, s: u32) {
        match s {
            0..=143 => self.code(0x30 + s, 8),
            144..=255 => self.code(0x190 + s - 144, 9),
            256..=279 => self.code(s - 256, 7),
            _ => self.code(0xc0 + s - 280, 8),
        }
    }

    fn finish(mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.n = 0;
    }
}

/// The base length of each length symbol from 257, and its extra bits.
const LENGTHS: [(u32, u32); 29] = [
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (8, 0),
    (9, 0),
    (10, 0),
    (11, 1),
    (13, 1),
    (15, 1),
    (17, 1),
    (19, 2),
    (23, 2),
    (27, 2),
    (31, 2),
    (35, 3),
    (43, 3),
    (51, 3),
    (59, 3),
    (67, 4),
    (83, 4),
    (99, 4),
    (115, 4),
    (131, 5),
    (163, 5),
    (195, 5),
    (227, 5),
    (258, 0),
];

/// `data` as one final fixed-Huffman block: literals, and each run of a repeated byte as matches at distance 1.
fn deflate_runs(data: &[u8], out: &mut Vec<u8>) {
    let mut w = Bits { out, acc: 0, n: 0 };
    w.put(1, 1);
    w.put(1, 2);
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        w.symbol(u32::from(b));
        i += 1;
        let mut run = data[i..].iter().take_while(|&&x| x == b).count();
        i += run;
        while run >= 3 {
            let len = run.min(258) as u32;
            // A match of 258 leaves the rest; a leftover of one or two goes as literals.
            let k = LENGTHS
                .iter()
                .rposition(|&(base, _)| base <= len)
                .unwrap_or(0);
            let (base, extra) = LENGTHS[k];
            w.symbol(257 + k as u32);
            w.put(len - base, extra);
            w.code(0, 5);
            run -= len as usize;
        }
        for _ in 0..run {
            w.symbol(u32::from(b));
        }
    }
    w.symbol(256);
    w.finish();
}

/// Writes a chunk (for the synthetic files).
pub fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

#[cfg(test)]
#[path = "png_tests.rs"]
mod tests;
