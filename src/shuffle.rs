//! Statistically unbiased shuffling.
//!
//! Spotify's built-in shuffle is widely observed to be non-uniform (it
//! deliberately spreads artists/albums apart and reacts to skip behavior).
//! This module implements the classic Fisher–Yates shuffle, which yields
//! every permutation with equal probability given a uniform RNG.
//!
//! Cryptographic security is not required for shuffling a playlist, but a
//! CSPRNG is cheap, so we use ChaCha20 seeded from the operating system's
//! entropy source anyway.

use rand::Rng;
use rand::SeedableRng;
use rand::TryRngCore;
use rand::rngs::OsRng;
use rand_chacha::ChaCha20Rng;

/// A ChaCha20 CSPRNG freshly seeded from OS entropy.
pub fn os_seeded_csprng() -> ChaCha20Rng {
    let mut seed = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut seed)
        .expect("operating system RNG unavailable");
    ChaCha20Rng::from_seed(seed)
}

/// In-place Fisher–Yates (Durstenfeld) shuffle.
///
/// Iterates from the last index down; each element swaps with a uniformly
/// chosen index in `0..=i`. `Rng::random_range` performs unbiased bounded
/// sampling (rejection-based), so no modulo bias is introduced.
pub fn fisher_yates_shuffle<T, R: Rng>(items: &mut [T], rng: &mut R) {
    for i in (1..items.len()).rev() {
        let j = rng.random_range(0..=i);
        items.swap(i, j);
    }
}

/// Convenience: clone `items` into a new, unbiasedly shuffled `Vec`, using a
/// fresh OS-seeded CSPRNG.
pub fn unbiased_shuffled<T: Clone>(items: &[T]) -> Vec<T> {
    let mut v = items.to_vec();
    fisher_yates_shuffle(&mut v, &mut os_seeded_csprng());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn preserves_elements() {
        let mut rng = ChaCha20Rng::seed_from_u64(7);
        let original: Vec<u32> = (0..1000).collect();
        let mut shuffled = original.clone();
        fisher_yates_shuffle(&mut shuffled, &mut rng);
        let mut sorted = shuffled.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, original);
        assert_ne!(
            shuffled, original,
            "1000 elements staying put is ~impossible"
        );
    }

    #[test]
    fn handles_degenerate_sizes() {
        let mut rng = ChaCha20Rng::seed_from_u64(7);
        let mut empty: Vec<u8> = vec![];
        fisher_yates_shuffle(&mut empty, &mut rng);
        assert!(empty.is_empty());
        let mut one = vec![42];
        fisher_yates_shuffle(&mut one, &mut rng);
        assert_eq!(one, vec![42]);
    }

    /// Every permutation of 4 elements should appear with equal frequency.
    /// 240k trials → expected 10k per permutation, σ ≈ 99; the ±600 (≈6σ)
    /// tolerance makes a false failure essentially impossible while still
    /// catching any real bias (e.g. the classic `rand() % n` bug or a
    /// naive-swap shuffle, which skew counts by several percent).
    #[test]
    fn distribution_is_uniform_over_permutations() {
        const TRIALS: usize = 240_000;
        const PERMS: usize = 24;
        const EXPECTED: i64 = (TRIALS / PERMS) as i64;
        const TOLERANCE: i64 = 600;

        // Deterministic seed: this is a property check, not an entropy check.
        let mut rng = ChaCha20Rng::seed_from_u64(0xDECAF);
        let mut counts: HashMap<[u8; 4], i64> = HashMap::new();
        for _ in 0..TRIALS {
            let mut arr = [0u8, 1, 2, 3];
            fisher_yates_shuffle(&mut arr, &mut rng);
            *counts.entry(arr).or_insert(0) += 1;
        }

        assert_eq!(counts.len(), PERMS, "all 24 permutations must occur");
        for (perm, count) in counts {
            assert!(
                (count - EXPECTED).abs() <= TOLERANCE,
                "permutation {perm:?} occurred {count} times, expected {EXPECTED} ± {TOLERANCE}"
            );
        }
    }

    #[test]
    fn os_seeded_rngs_differ() {
        // Two freshly seeded CSPRNGs must not produce the same stream.
        let a: Vec<u64> = {
            let mut r = os_seeded_csprng();
            (0..8).map(|_| r.random()).collect()
        };
        let b: Vec<u64> = {
            let mut r = os_seeded_csprng();
            (0..8).map(|_| r.random()).collect()
        };
        assert_ne!(a, b);
    }
}
