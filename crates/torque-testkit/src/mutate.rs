//! Byte mutators for hostile-input tests: each takes a valid message and damages it one way. The result is whatever the
//! decoder under test must still answer with a value or an error.

use crate::Rng;

/// Flips one to four random bits.
pub fn flip(rng: &mut Rng, bytes: &mut [u8]) {
    for _ in 0..1 + rng.below(4) {
        if bytes.is_empty() {
            return;
        }
        let at = rng.index(bytes.len());
        bytes[at] ^= 1 << rng.below(8);
    }
}

/// Overwrites one to four bytes, each with a random byte or `0xff`.
pub fn stomp(rng: &mut Rng, bytes: &mut [u8]) {
    for _ in 0..1 + rng.below(4) {
        if bytes.is_empty() {
            return;
        }
        let at = rng.index(bytes.len());
        bytes[at] = if rng.chance(25) { 0xff } else { rng.byte() };
    }
}

/// Cuts the message to a random length, possibly its own (so a mutant can be the original).
pub fn cut(rng: &mut Rng, bytes: &mut Vec<u8>) {
    let len = rng.index(bytes.len() + 1);
    bytes.truncate(len);
}

/// Inserts up to 40 random bytes at a random place.
pub fn splice(rng: &mut Rng, bytes: &mut Vec<u8>) {
    let at = rng.index(bytes.len() + 1);
    let count = rng.index(41);
    let insert = rng.bytes(count);
    bytes.splice(at..at, insert);
}

/// Appends up to 40 random bytes: a decoder that ignores what follows its message must still accept the original.
pub fn append(rng: &mut Rng, bytes: &mut Vec<u8>) {
    let count = rng.index(41);
    bytes.extend(rng.bytes(count));
}

/// Removes one byte.
pub fn remove(rng: &mut Rng, bytes: &mut Vec<u8>) {
    if !bytes.is_empty() {
        let at = rng.index(bytes.len());
        bytes.remove(at);
    }
}

/// Damages `bytes` one random way: [`flip`], [`stomp`], [`cut`], [`splice`], [`append`], [`remove`], or replaced by up to 200
/// bytes of noise.
pub fn mutate(rng: &mut Rng, bytes: &mut Vec<u8>) {
    match rng.below(7) {
        0 => flip(rng, bytes),
        1 => stomp(rng, bytes),
        2 => cut(rng, bytes),
        3 => splice(rng, bytes),
        4 => append(rng, bytes),
        5 => remove(rng, bytes),
        _ => {
            let count = rng.index(201);
            *bytes = rng.bytes(count);
        }
    }
}

/// A [`mutate`]d copy of `original`.
#[must_use]
pub fn mutant(rng: &mut Rng, original: &[u8]) -> Vec<u8> {
    let mut bytes = original.to_vec();
    mutate(rng, &mut bytes);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutators_survive_empty_and_tiny_input() {
        let mut rng = Rng::new(5);
        for len in 0..4 {
            for _ in 0..500 {
                let mut bytes = vec![7u8; len];
                mutate(&mut rng, &mut bytes);
            }
        }
    }

    #[test]
    fn a_mutant_leaves_the_original_alone() {
        let mut rng = Rng::new(3);
        let original = vec![1u8; 16];
        let mutants: Vec<_> = (0..50).map(|_| mutant(&mut rng, &original)).collect();
        assert_eq!(original, vec![1u8; 16]);
        assert!(mutants.iter().any(|m| *m != original));
    }

    #[test]
    fn every_mutator_changes_some_input() {
        let mut rng = Rng::new(11);
        let original = vec![0u8; 64];
        let steps: [fn(&mut Rng, &mut Vec<u8>); 7] = [
            |r, b| flip(r, b),
            |r, b| stomp(r, b),
            cut,
            splice,
            append,
            remove,
            mutate,
        ];
        for step in steps {
            let changed = (0..200).any(|_| {
                let mut bytes = original.clone();
                step(&mut rng, &mut bytes);
                bytes != original
            });
            assert!(changed);
        }
    }
}
