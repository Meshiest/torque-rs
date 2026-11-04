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

/// The CRC-32 of PNG (ISO 3309, reflected polynomial 0xEDB88320).
pub fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for part in parts {
        for &b in *part {
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
