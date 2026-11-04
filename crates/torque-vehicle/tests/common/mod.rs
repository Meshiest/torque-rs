//! The case generator the fuzz test uses: xorshift64*, integer-only randomness, floats built by IEEE operations whose
//! results do not depend on the platform.
#![allow(dead_code)]

pub mod host;

pub const SPECIAL: [f32; 8] = [
    0.0,
    -0.0,
    1e30,
    -1e30,
    f32::NAN,
    f32::INFINITY,
    f32::NEG_INFINITY,
    3.0e38,
];

pub struct Gen(u64);

impl Gen {
    pub fn new(seed: u64) -> Gen {
        let s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        Gen(if s == 0 { 1 } else { s })
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    pub fn f(&mut self) -> f32 {
        let k = self.below(100);
        if k < 3 {
            return SPECIAL[self.below(8) as usize];
        }
        if k < 10 {
            return [0.0, 1.0, -1.0, 0.5][self.below(4) as usize];
        }
        if k < 20 {
            return ((self.below(200_001) as f64 - 100_000.0) / 100.0) as f32;
        }
        ((self.below(20_001) as f64 - 10_000.0) / 2000.0) as f32
    }
}
