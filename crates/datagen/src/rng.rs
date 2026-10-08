const GOLDEN_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    Values,
    Blanks,
}

// SplitMix64 (Steele, Lea, Flood 2014). Implemented here rather than taken from a crate so
// the output for a given seed can never change with a dependency update.
pub(crate) struct Rng {
    state: u64,
}

impl Rng {
    pub(crate) fn new(seed: u64, column: usize, stream: Stream) -> Rng {
        let stream_tag = match stream {
            Stream::Values => 0,
            Stream::Blanks => 1,
        };
        let column_tag = (column as u64).wrapping_mul(2).wrapping_add(stream_tag);
        Rng {
            state: mix(seed ^ mix(column_tag.wrapping_add(GOLDEN_GAMMA))),
        }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GOLDEN_GAMMA);
        mix(self.state)
    }

    // The caller guarantees `bound > 0`.
    pub(crate) fn below(&mut self, bound: u128) -> u128 {
        match u64::try_from(bound) {
            Ok(small) => u128::from(self.below_u64(small)),
            Err(_) => self.below_wide(bound),
        }
    }

    pub(crate) fn index(&mut self, len: usize) -> usize {
        self.below_u64(len as u64) as usize
    }

    // Lemire's multiply-shift with rejection: unbiased, and each draw is accepted with
    // probability above one half.
    fn below_u64(&mut self, bound: u64) -> u64 {
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let product = u128::from(self.next_u64()) * u128::from(bound);
            if (product as u64) >= threshold {
                return (product >> 64) as u64;
            }
        }
    }

    fn below_wide(&mut self, bound: u128) -> u128 {
        let mask = u128::MAX >> (bound - 1).leading_zeros();
        loop {
            let candidate =
                ((u128::from(self.next_u64()) << 64) | u128::from(self.next_u64())) & mask;
            if candidate < bound {
                return candidate;
            }
        }
    }
}

fn mix(value: u64) -> u64 {
    let mut z = value;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_sequence_never_changes() {
        let mut rng = Rng::new(48_213, 0, Stream::Values);
        let first: Vec<u64> = (0..3).map(|_| rng.next_u64()).collect();
        let mut again = Rng::new(48_213, 0, Stream::Values);
        assert_eq!(first, (0..3).map(|_| again.next_u64()).collect::<Vec<_>>());
        assert_eq!(first, KNOWN_FIRST_DRAWS);
    }

    const KNOWN_FIRST_DRAWS: [u64; 3] = [
        15_837_381_500_263_081_764,
        17_144_577_158_724_361_681,
        13_662_930_274_714_611_599,
    ];

    #[test]
    fn matches_the_reference_splitmix64() {
        let mut rng = Rng { state: 1_234_567 };
        let draws: Vec<u64> = (0..3).map(|_| rng.next_u64()).collect();
        assert_eq!(
            draws,
            [
                6_457_827_717_110_365_317,
                3_203_168_211_198_807_973,
                9_817_491_932_198_370_423,
            ]
        );
    }

    #[test]
    fn streams_differ_by_column_and_purpose() {
        let draw = |column, stream| Rng::new(7, column, stream).next_u64();
        assert_ne!(draw(0, Stream::Values), draw(1, Stream::Values));
        assert_ne!(draw(0, Stream::Values), draw(0, Stream::Blanks));
    }

    #[test]
    fn below_stays_in_bounds() {
        let mut rng = Rng::new(1, 0, Stream::Values);
        for bound in [1u128, 2, 3, 10, u128::from(u64::MAX) + 7, 1 << 122] {
            for _ in 0..200 {
                assert!(rng.below(bound) < bound);
            }
        }
    }
}
