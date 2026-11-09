//! A seeded random source for tests and fixed-seed fuzz loops: a xorshift, so one seed gives the same numbers on every run
//! and target. Never for anything that must be unpredictable (keys, nonces, sequences).

/// A xorshift64 from a seed (module docs).
#[derive(Debug, Clone)]
pub struct TestRng(u64);

impl TestRng {
    /// A source seeded with `seed` (a zero seed is taken as one, which xorshift needs).
    #[must_use]
    pub const fn new(seed: u64) -> TestRng {
        TestRng(if seed == 0 { 1 } else { seed })
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number below `n` (0 for an `n` of 0).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    /// An index below `len` (0 for an empty one).
    pub fn index(&mut self, len: usize) -> usize {
        let len = u64::try_from(len).unwrap_or(u64::MAX);
        usize::try_from(self.below(len)).unwrap_or(0)
    }

    /// A random byte.
    pub fn byte(&mut self) -> u8 {
        self.next_u64().to_le_bytes()[0]
    }
}

#[cfg(test)]
mod tests {
    use super::TestRng;

    #[test]
    fn a_seed_repeats_its_numbers_and_stays_in_range() {
        let (mut a, mut b) = (TestRng::new(7), TestRng::new(7));
        for _ in 0..1_000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        for _ in 0..1_000 {
            assert!(a.below(10) < 10);
            assert!(a.index(3) < 3);
        }
        assert_eq!(TestRng::new(0).next_u64(), TestRng::new(1).next_u64());
    }
}
