//! [`Rng`]: the seeded random numbers generators use
//! ([`GenContext::rng`](crate::GenContext::rng)).
//!
//! Generators must be deterministic, so randomness comes only from the
//! entity's `Procedural::seed` through this generator (SplitMix64): the
//! same seed gives the same sequence on every machine and in every run.
//! Never use a thread-local or time-seeded RNG in a generator.

/// A small, fast, seeded pseudo-random generator (SplitMix64). Not for
/// cryptography.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    seed: u64,
    state: u64,
}

impl Rng {
    /// A generator for `seed`. Each seed gives its own sequence.
    pub fn new(seed: u64) -> Self {
        Self { seed, state: seed }
    }

    /// A generator derived from this one's seed and `stream`, independent
    /// of how many numbers this one has produced: give each element its own
    /// stream (`ctx.rng().fork(step_index)`) so adding an element does not
    /// change the numbers of the others.
    pub fn fork(&self, stream: u64) -> Self {
        let mut r = Self::new(self.seed ^ stream.wrapping_mul(0xd1b5_4a32_d192_ed03));
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        // The top 24 bits: every value is exactly representable.
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in `[min, max)` (`min` when the range is empty).
    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        if max > min {
            min + (max - min) * self.next_f32()
        } else {
            min
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let a: Vec<u64> = {
            let mut r = Rng::new(7);
            (0..4).map(|_| r.next_u64()).collect()
        };
        let mut r = Rng::new(7);
        let b: Vec<u64> = (0..4).map(|_| r.next_u64()).collect();
        assert_eq!(a, b);
        // A reference value pins the algorithm (stored hashes depend on it).
        assert_eq!(Rng::new(0).next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_ne!(Rng::new(8).next_u64(), a[0]);
    }

    #[test]
    fn floats_stay_in_range_and_forks_are_independent() {
        let mut r = Rng::new(3);
        for _ in 0..1000 {
            let f = r.next_f32();
            assert!((0.0..1.0).contains(&f));
            let x = r.range(-2.0, 5.0);
            assert!((-2.0..5.0).contains(&x));
        }
        let base = Rng::new(3);
        let mut used = base.clone();
        used.next_u64();
        assert_eq!(used.fork(1), Rng::new(3).fork(1));
        assert_ne!(base.fork(1), base.fork(2));
        assert_eq!(Rng::new(1).range(4.0, 4.0), 4.0);
    }
}
