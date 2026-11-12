//! CRC-32 (polynomial 0xEDB88320, reflected). [`crc32`] is the standard one (zlib's, PNG's); [`Crc32`] exposes the running state
//! so a caller can start from `0xFFFFFFFF` and stop without the final complement, as a wire checksum may (init `FFFFFFFF`, no
//! final xor). The update takes eight bytes a step (slicing by eight), which gives the value of one byte a step.

const fn table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

/// [`table`] and seven more: table `k` is a byte's CRC with `k` zero bytes after it.
const fn tables() -> [[u32; 256]; 8] {
    let mut tables = [table(); 8];
    let mut k = 1;
    while k < 8 {
        let mut n = 0;
        while n < 256 {
            let c = tables[k - 1][n];
            tables[k][n] = (c >> 8) ^ tables[0][(c & 0xff) as usize];
            n += 1;
        }
        k += 1;
    }
    tables
}

static TABLES: [[u32; 256]; 8] = tables();

/// A running CRC-32 state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crc32 {
    state: u32,
}

impl Crc32 {
    /// The state `0xFFFFFFFF`.
    #[must_use]
    pub fn new() -> Crc32 {
        Crc32 { state: u32::MAX }
    }

    /// Feeds `bytes`.
    pub fn update(&mut self, bytes: &[u8]) {
        let t = &TABLES;
        let mut c = self.state;
        let (words, rest) = bytes.as_chunks::<8>();
        for w in words {
            let one = u32::from_le_bytes([w[0], w[1], w[2], w[3]]) ^ c;
            let two = u32::from_le_bytes([w[4], w[5], w[6], w[7]]);
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
            c = t[0][((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
        }
        self.state = c;
    }

    /// The state as it stands, with no final complement.
    #[must_use]
    pub fn raw(&self) -> u32 {
        self.state
    }

    /// The standard CRC-32 of what was fed: the state complemented.
    #[must_use]
    pub fn finish(&self) -> u32 {
        !self.state
    }
}

impl Default for Crc32 {
    fn default() -> Crc32 {
        Crc32::new()
    }
}

/// The standard CRC-32 of `bytes`.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(bytes);
    c.finish()
}

/// The standard CRC-32 of `parts` one after the other (a PNG chunk's name and data, without joining them).
#[must_use]
pub fn crc32_parts(parts: &[&[u8]]) -> u32 {
    let mut c = Crc32::new();
    for part in parts {
        c.update(part);
    }
    c.finish()
}

#[cfg(test)]
#[path = "crc_tests.rs"]
mod tests;
