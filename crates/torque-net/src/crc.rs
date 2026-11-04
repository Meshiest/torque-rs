//! CRC-32 (polynomial 0xEDB88320, reflected). [`crc32`] is the standard one (zlib's); [`Crc32`] exposes the running state so a
//! caller can start from `0xFFFFFFFF` and stop without the final complement, as the camera's checksum does (init
//! `FFFFFFFF`, no final xor).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `core/crc.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static TABLE: [u32; 256] = table();

/// A running CRC-32 state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crc32 {
    state: u32,
}

impl Crc32 {
    /// The state `0xFFFFFFFF`.
    pub fn new() -> Crc32 {
        Crc32 { state: u32::MAX }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            let index = usize::from((self.state as u8) ^ b);
            self.state = (self.state >> 8) ^ TABLE.get(index).copied().unwrap_or(0);
        }
    }

    /// The state as it stands, with no final complement.
    pub fn raw(&self) -> u32 {
        self.state
    }

    /// The standard CRC-32 of what was fed: the state complemented.
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
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(bytes);
    c.finish()
}

#[cfg(test)]
#[path = "crc_tests.rs"]
mod tests;
