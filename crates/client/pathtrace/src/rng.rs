//! The randomness, and the reason it is not `rand`.
//!
//! A reference image has to be reproducible on any machine, at any sample
//! count, whatever order the pixels are visited in — otherwise a difference
//! between two runs is indistinguishable from a difference between two
//! renderers, which is the only thing this crate is for. That rules out a
//! thread-local generator and rules out sharing one stream across pixels, and
//! it is the same rule this workspace already applies to the simulation tick
//! (`docs/style.md`, "Randomness and time").
//!
//! So: each pixel draws from its own stream, addressed by its own coordinates.
//! PCG-XSH-RR, thirty lines, no dependency — small enough to read in full,
//! which for a component nothing verifies from the outside is worth more than
//! statistical pedigree. It is not cryptographic and does not need to be.

/// One pixel's own stream of numbers.
#[derive(Clone, Debug)]
pub struct Stream {
    state: u64,
    /// The stream selector, forced odd — what makes two pixels' sequences
    /// different rather than offset copies of one sequence.
    increment: u64,
}

impl Stream {
    /// The stream a given pixel of a given render draws from.
    ///
    /// `seed` names the render and `sequence` names the pixel inside it. Two
    /// renders at different sample counts share a pixel's stream prefix, which
    /// is what makes a 16-sample preview a genuine prefix of a 256-sample final
    /// image rather than an unrelated picture of the same scene.
    pub fn new(seed: u64, sequence: u64) -> Self {
        let mut stream = Self {
            state: 0,
            increment: (sequence << 1) | 1,
        };
        stream.next_u32();
        stream.state = stream.state.wrapping_add(seed);
        stream.next_u32();
        stream
    }

    fn next_u32(&mut self) -> u32 {
        let previous = self.state;
        self.state = previous
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(self.increment);
        let xorshifted = (((previous >> 18) ^ previous) >> 27) as u32;
        let rotation = (previous >> 59) as u32;
        xorshifted.rotate_right(rotation)
    }

    /// A number in `0.0..1.0`, never 1.0.
    pub fn unit(&mut self) -> f64 {
        // 2^-32 exactly, so the largest `u32` maps to just under one and the
        // half-open interval every sampling formula below assumes really is
        // half-open — an inverse transform that can be handed exactly 1.0 has a
        // singularity waiting in it.
        f64::from(self.next_u32()) * 2.328_306_436_538_696_3e-10
    }
}

#[cfg(test)]
mod tests {
    use super::Stream;

    #[test]
    fn one_pixels_stream_is_the_same_sequence_every_time() {
        let first: Vec<f64> = (0..8)
            .scan(Stream::new(7, 12345), |s, _| Some(s.unit()))
            .collect();
        let again: Vec<f64> = (0..8)
            .scan(Stream::new(7, 12345), |s, _| Some(s.unit()))
            .collect();
        assert_eq!(first, again, "the same seed and pixel replay exactly");
    }

    #[test]
    fn a_longer_render_extends_a_shorter_ones_samples_rather_than_replacing_them() {
        // What lets a preview be trusted: the 4-sample picture is the 64-sample
        // one's own first four samples, so a difference between the two is
        // convergence and not a different random scene.
        let mut short = Stream::new(1, 99);
        let mut long = Stream::new(1, 99);
        let prefix: Vec<f64> = (0..4).map(|_| short.unit()).collect();
        let whole: Vec<f64> = (0..64).map(|_| long.unit()).collect();
        assert_eq!(prefix, whole[..4], "the prefix is shared");
    }

    #[test]
    fn neighbouring_pixels_do_not_draw_the_same_numbers() {
        // Correlated neighbours are the failure that looks like a picture: the
        // noise lines up into bands and reads as structure in the scene.
        let a: Vec<f64> = (0..16)
            .scan(Stream::new(3, 1000), |s, _| Some(s.unit()))
            .collect();
        let b: Vec<f64> = (0..16)
            .scan(Stream::new(3, 1001), |s, _| Some(s.unit()))
            .collect();
        assert_ne!(a, b, "two pixels of one render are two streams");
        let shared = a.iter().zip(&b).filter(|(x, y)| x == y).count();
        assert_eq!(shared, 0, "and not one stream read at two offsets");
    }

    #[test]
    fn the_samples_are_in_the_half_open_unit_interval_and_spread_over_it() {
        // Not a randomness test — a "this is not broken" test. A generator
        // stuck in one octave of the interval still passes every determinism
        // check above and quietly halves every estimator that uses it.
        let mut stream = Stream::new(11, 4);
        let mut buckets = [0usize; 8];
        for _ in 0..80_000 {
            let sample = stream.unit();
            assert!((0.0..1.0).contains(&sample), "{sample} left the unit interval");
            buckets[(sample * 8.0) as usize] += 1;
        }
        for (index, count) in buckets.iter().enumerate() {
            assert!(
                (8_000..12_000).contains(count),
                "eighth {index} took {count} of 80,000 samples"
            );
        }
    }
}
