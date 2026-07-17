//! A tiny deterministic pseudo-random generator.
//!
//! # Why the world owns its randomness
//!
//! The tick is replayable: the same commands from the same start produce the
//! same world, which is what makes a bug reproducible from a log. A roll — does
//! this swing land, does this skill gain — is randomness *inside* that tick, and
//! if it came from the OS or a thread-local, replay would break the first time a
//! die was cast.
//!
//! So the generator is a plain value the world holds, seeded once and advanced
//! only by the tick. Replay the tick and every roll comes out the same. This is
//! `xorshift64*` — fast, tiny, and nowhere near cryptographic, which is right:
//! nothing here is a secret, only whether a blow connects.

/// A seeded `xorshift64*` generator.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// A generator seeded with `seed`. Zero would wedge `xorshift` at zero
    /// forever, so it is replaced with a fixed non-zero constant.
    pub const fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// The next 64 bits.
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A value in `0..bound`. `bound` of zero yields zero.
    ///
    /// The modulo is very slightly biased for a `bound` that does not divide
    /// `2^64`, which for a skill roll out of 1000 is far below anything a player
    /// could feel.
    pub fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() % u64::from(bound)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.below(1000), b.below(1000));
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        let a: Vec<u32> = (0..16).map(|_| a.below(1000)).collect();
        let b: Vec<u32> = (0..16).map(|_| b.below(1000)).collect();
        assert_ne!(a, b);
    }

    #[test]
    fn a_bound_is_never_exceeded() {
        let mut rng = Rng::new(7);
        for _ in 0..1000 {
            assert!(rng.below(10) < 10);
        }
        assert_eq!(rng.below(0), 0);
    }

    #[test]
    fn zero_seed_still_generates() {
        let mut rng = Rng::new(0);
        // Would be stuck at zero if the seed had not been replaced.
        assert!((0..8).map(|_| rng.below(1000)).any(|v| v != 0));
    }
}
