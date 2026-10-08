use std::collections::HashMap;

use crate::rng::Rng;

// A Fisher-Yates shuffle of 0..domain advanced one step per draw, storing only the swapped
// slots: every draw is new and costs one random number, never a retry. Callers must not
// draw more than `domain` times; the spec validation guarantees it.
pub(crate) struct DistinctDraws {
    domain: u128,
    drawn: u128,
    swapped: HashMap<u128, u128>,
}

impl DistinctDraws {
    pub(crate) fn new(domain: u128, expected_draws: usize) -> DistinctDraws {
        DistinctDraws {
            domain,
            drawn: 0,
            swapped: HashMap::with_capacity(expected_draws),
        }
    }

    pub(crate) fn next(&mut self, rng: &mut Rng) -> u128 {
        let step = self.drawn;
        let target = step + rng.below(self.domain - step);
        let at_target = self.swapped.get(&target).copied().unwrap_or(target);
        let at_step = self.swapped.get(&step).copied().unwrap_or(step);
        self.swapped.insert(target, at_step);
        self.drawn += 1;
        at_target
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::rng::Stream;

    #[test]
    fn draws_every_value_once_when_exhausting_the_domain() {
        let mut rng = Rng::new(9, 0, Stream::Values);
        let mut draws = DistinctDraws::new(1_000, 1_000);
        let mut drawn: Vec<u128> = (0..1_000).map(|_| draws.next(&mut rng)).collect();
        drawn.sort_unstable();
        assert_eq!(drawn, (0..1_000).collect::<Vec<_>>());
    }

    #[test]
    fn draws_distinct_values_from_a_huge_domain() {
        let mut rng = Rng::new(9, 0, Stream::Values);
        let mut draws = DistinctDraws::new(u128::MAX, 5_000);
        let drawn: HashSet<u128> = (0..5_000).map(|_| draws.next(&mut rng)).collect();
        assert_eq!(drawn.len(), 5_000);
    }
}
