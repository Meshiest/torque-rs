//! The hash a trace folds its per-tick state into: 64-bit FNV-1a over the bytes of what is fed, in a fixed order.
//!
//! The hash is not cryptographic; it only has to differ when one bit of the state does. Floats go in by their bit patterns
//! (`f32::to_bits`), so a one-ulp difference between native and wasm32 changes the value. A NaN goes in as one canonical
//! pattern: its payload is not part of the game's state, and the two targets may choose it differently.

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// The canonical bits a NaN is hashed as.
const NAN_BITS: u32 = 0x7fc0_0000;

/// A trace being folded into one value.
#[derive(Debug, Clone)]
pub struct TraceHasher {
    state: u64,
    values: u64,
}

impl Default for TraceHasher {
    fn default() -> TraceHasher {
        TraceHasher::new()
    }
}

impl TraceHasher {
    pub fn new() -> TraceHasher {
        TraceHasher {
            state: OFFSET,
            values: 0,
        }
    }

    fn byte(&mut self, b: u8) {
        self.state ^= u64::from(b);
        self.state = self.state.wrapping_mul(PRIME);
    }

    pub fn u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.byte(b);
        }
        self.values += 1;
    }

    pub fn u64(&mut self, v: u64) {
        self.u32(v as u32);
        self.u32((v >> 32) as u32);
    }

    pub fn bool(&mut self, v: bool) {
        self.u32(u32::from(v));
    }

    pub fn f32(&mut self, v: f32) {
        self.u32(if v.is_nan() { NAN_BITS } else { v.to_bits() });
    }

    pub fn floats(&mut self, values: &[f32]) {
        for v in values {
            self.f32(*v);
        }
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        self.u32(bytes.len() as u32);
        for b in bytes {
            self.byte(*b);
        }
    }

    /// How many values were fed (a scenario that fed none hashes to the empty value: its caller can tell).
    pub fn count(&self) -> u64 {
        self.values
    }

    pub fn finish(&self) -> u64 {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_matches_the_published_vectors() {
        let mut h = TraceHasher::new();
        assert_eq!(h.finish(), 0xcbf2_9ce4_8422_2325);
        for b in b"a" {
            h.byte(*b);
        }
        assert_eq!(h.finish(), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn one_ulp_changes_the_value_and_a_nan_payload_does_not() {
        let hash = |v: f32| {
            let mut h = TraceHasher::new();
            h.f32(v);
            h.finish()
        };
        assert_ne!(hash(1.0), hash(f32::from_bits(1.0f32.to_bits() + 1)));
        assert_eq!(hash(f32::NAN), hash(f32::from_bits(0x7fc0_1234)));
        assert_ne!(hash(0.0), hash(-0.0));
    }
}
