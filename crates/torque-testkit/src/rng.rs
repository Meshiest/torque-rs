//! A seeded random source for tests: one seed gives the same numbers on every run and target.

/// Which generator an [`Rng`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Algorithm {
    /// xorshift64 with the 13/7/17 triple: the state is the output.
    Shift,
    /// xorshift64* with the 12/25/27 triple: the state times a constant is the output.
    Star,
}

/// A seeded generator for tests and fixed-seed fuzz loops. Never for anything that must be unpredictable (keys, nonces).
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
    algorithm: Algorithm,
}

impl Rng {
    /// A xorshift64 (13/7/17) seeded with `seed`; a zero seed is taken as one, which xorshift needs.
    #[must_use]
    pub const fn new(seed: u64) -> Rng {
        Rng {
            state: if seed == 0 { 1 } else { seed },
            algorithm: Algorithm::Shift,
        }
    }

    /// A xorshift64* (12/25/27, output times `0x2545f4914f6cdd1d`) from `seed` scrambled by `0x9e3779b97f4a7c15` and made odd.
    /// Tests that pin a golden to the sequence keep these exact bits.
    #[must_use]
    pub const fn star(seed: u64) -> Rng {
        Rng {
            state: seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1,
            algorithm: Algorithm::Star,
        }
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        match self.algorithm {
            Algorithm::Shift => {
                self.state ^= self.state << 13;
                self.state ^= self.state >> 7;
                self.state ^= self.state << 17;
                self.state
            }
            Algorithm::Star => {
                self.state ^= self.state >> 12;
                self.state ^= self.state << 25;
                self.state ^= self.state >> 27;
                self.state.wrapping_mul(0x2545_f491_4f6c_dd1d)
            }
        }
    }

    /// The upper 32 bits of the next 64: the better half of either generator.
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
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

    /// True `percent` times in a hundred.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    /// A fair coin.
    pub fn flip(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    /// A random byte (the low byte of the next 64 bits).
    pub fn byte(&mut self) -> u8 {
        self.next_u64().to_le_bytes()[0]
    }

    /// A float from `lo` up to `hi` (at most; both finite).
    pub fn f32_in(&mut self, lo: f32, hi: f32) -> f32 {
        let unit = self.next_u32() as f32 / u32::MAX as f32;
        lo + unit * (hi - lo)
    }

    /// `n` random bytes.
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }

    /// Fewer than `max` random bytes (none for a `max` of 0).
    pub fn some_bytes(&mut self, max: usize) -> Vec<u8> {
        let n = self.index(max);
        self.bytes(n)
    }

    /// Fills `out` with random bytes.
    pub fn fill(&mut self, out: &mut [u8]) {
        for slot in out {
            *slot = self.byte();
        }
    }

    /// One of `items`, chosen at random.
    ///
    /// # Panics
    ///
    /// When `items` is empty.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        assert!(!items.is_empty(), "pick from an empty slice");
        &items[self.index(items.len())]
    }
}

/// SplitMix64: one add and two multiplies, for the tests that seed a separate stream from each case number.
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// A stream from `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> SplitMix64 {
        SplitMix64(seed)
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_repeats_its_numbers_and_stays_in_range() {
        for make in [Rng::new, Rng::star] {
            let (mut a, mut b) = (make(7), make(7));
            for _ in 0..1_000 {
                assert_eq!(a.next_u64(), b.next_u64());
            }
            for _ in 0..1_000 {
                assert!(a.below(10) < 10);
                assert!(a.index(3) < 3);
                let f = a.f32_in(-2.0, 5.0);
                assert!((-2.0..=5.0).contains(&f));
            }
            assert_eq!(a.below(0), 0);
            assert_eq!(a.index(0), 0);
        }
        assert_eq!(Rng::new(0).next_u64(), Rng::new(1).next_u64());
    }

    /// The first outputs of each generator, worked out by hand from the definitions: a change to either would move every
    /// seeded test that adopted it, and the pinned wire goldens that use `star`.
    #[test]
    fn the_generators_keep_their_bits() {
        // xorshift64 13/7/17 from 1, by the definition.
        let mut x = 1u64;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        assert_eq!(Rng::new(1).next_u64(), x);
        assert_eq!(x, 1_082_269_761);

        let mut s = 0x9e37_79b9_7f4a_7c15u64.wrapping_mul(3) | 1;
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        assert_eq!(
            Rng::star(3).next_u64(),
            s.wrapping_mul(0x2545_f491_4f6c_dd1d)
        );
    }

    #[test]
    fn splitmix_matches_the_reference_vector() {
        // The first outputs of the reference implementation from seed 0.
        let mut m = SplitMix64::new(0);
        assert_eq!(m.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(m.next_u64(), 0x6e78_9e6a_a1b9_65f4);
    }

    #[test]
    fn bytes_fill_and_pick_draw_from_the_stream() {
        let mut a = Rng::new(9);
        let mut b = Rng::new(9);
        let mut out = [0u8; 5];
        a.fill(&mut out);
        assert_eq!(out.to_vec(), b.bytes(5));
        assert!(a.some_bytes(8).len() < 8);
        assert!(a.some_bytes(0).is_empty());
        let items = [10, 20, 30];
        assert!(items.contains(a.pick(&items)));
    }
}
