// A sum of floats kept without rounding error, as non-overlapping partials (Shewchuk's
// algorithm, the one behind Python's `math.fsum`). Its value is the exact sum rounded
// once, so it does not depend on the order values were added in, and adding `-x` takes
// back exactly what adding `x` put in. SUMIF can then update a kept total by delta and
// still return the same number as summing the whole range again.
#[derive(Clone, Debug, Default)]
pub(crate) struct ExactSum {
    partials: Vec<f64>,
    // Summed the plain way, returned only when a value or a partial is not finite.
    plain: f64,
    overflowed: bool,
}

// Non-overlapping partials cover at most the 2098 bits from the smallest subnormal to the
// largest finite double, 53 bits each.
pub(crate) const MAX_PARTIALS: usize = 40;

impl ExactSum {
    pub(crate) fn add(&mut self, value: f64) {
        self.plain += value;
        if self.overflowed {
            return;
        }
        if !value.is_finite() {
            self.overflowed = true;
            return;
        }
        let mut x = value;
        let mut kept = 0;
        for index in 0..self.partials.len() {
            let mut y = self.partials[index];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let high = x + y;
            if !high.is_finite() {
                self.overflowed = true;
                return;
            }
            let low = y - (high - x);
            if low != 0.0 {
                self.partials[kept] = low;
                kept += 1;
            }
            x = high;
        }
        self.partials.truncate(kept);
        self.partials.push(x);
    }

    // A kept total never reallocates, so the bytes charged for it hold.
    pub(crate) fn reserve_all(&mut self) {
        self.partials
            .reserve_exact(MAX_PARTIALS.saturating_sub(self.partials.len()));
    }

    // Whether the value is the exact sum rounded once; past an overflow it is the plain
    // sum, which depends on the order values came in.
    pub(crate) fn is_exact(&self) -> bool {
        !self.overflowed
    }

    pub(crate) fn value(&self) -> f64 {
        if self.overflowed {
            return self.plain;
        }
        let partials = &self.partials;
        let mut index = partials.len();
        if index == 0 {
            return 0.0;
        }
        index -= 1;
        let mut high = partials[index];
        let mut low = 0.0;
        while index > 0 {
            let x = high;
            index -= 1;
            let y = partials[index];
            high = x + y;
            low = y - (high - x);
            if low != 0.0 {
                break;
            }
        }
        // Round half to even when the partials below `low` push it past the halfway point.
        if index > 0
            && ((low < 0.0 && partials[index - 1] < 0.0)
                || (low > 0.0 && partials[index - 1] > 0.0))
        {
            let y = low * 2.0;
            let x = high + y;
            if y == x - high {
                high = x;
            }
        }
        // An exact zero is +0, as a plain sum starting from 0 returns.
        high + 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::{ExactSum, MAX_PARTIALS};

    fn sum(values: &[f64]) -> f64 {
        let mut total = ExactSum::default();
        for value in values {
            total.add(*value);
        }
        total.value()
    }

    #[test]
    fn rounds_the_exact_sum_once() {
        assert_eq!(sum(&[]), 0.0);
        assert_eq!(sum(&[0.1, 0.2]), 0.1 + 0.2);
        assert_eq!(sum(&[1e100, 1.0, -1e100]), 1.0);
        assert_eq!(sum(&[1e16, 1.0, 1e-16]), 10000000000000002.0);
        assert_eq!(sum(&[0.1; 10]), 1.0);
        assert_eq!(sum(&[1.0, 1e-16, 1e-16]), 1.0000000000000002);
    }

    #[test]
    fn does_not_depend_on_order() {
        let values = [
            561.91 * 16.0 * 1.21,
            0.1,
            -3.3,
            1e15 + 0.3,
            -1e15,
            7.7e-5,
            2.0 / 3.0,
        ];
        let forward = sum(&values);
        let mut backward = values;
        backward.reverse();
        assert_eq!(sum(&backward), forward);
        let mut shuffled = values;
        shuffled.swap(0, 4);
        shuffled.swap(2, 6);
        assert_eq!(sum(&shuffled), forward);
    }

    #[test]
    fn taking_a_value_back_restores_the_sum() {
        let mut total = ExactSum::default();
        for value in [0.1, 0.2, 0.3, 1e15 + 0.3] {
            total.add(value);
        }
        let before = total.value();
        total.add(1e-3 + 7.0);
        total.add(-(1e-3 + 7.0));
        assert_eq!(total.value(), before);
        total.add(-0.1);
        total.add(-0.2);
        total.add(-0.3);
        total.add(-(1e15 + 0.3));
        assert_eq!(total.value(), 0.0);
        assert!(total.value().is_sign_positive());
    }

    #[test]
    fn rounds_ties_to_even() {
        let half_ulp = 2f64.powi(-53);
        assert_eq!(sum(&[1.0, half_ulp]), 1.0);
        assert_eq!(sum(&[1.0, half_ulp, 1e-300]), 1.0 + 2.0 * half_ulp);
    }

    #[test]
    fn falls_back_to_the_plain_sum_past_an_overflow() {
        let mut total = ExactSum::default();
        total.add(f64::MAX);
        total.add(f64::MAX);
        assert!(!total.is_exact());
        assert_eq!(total.value(), f64::INFINITY);
    }

    #[test]
    fn partials_stay_within_the_bound() {
        let mut total = ExactSum::default();
        let mut value = f64::MAX / 2.0;
        while value > 0.0 {
            total.add(value);
            value /= 3.0;
        }
        assert!(total.partials.len() <= MAX_PARTIALS);
    }

    #[test]
    fn reserved_partials_never_grow() {
        let mut total = ExactSum::default();
        total.add(1.0);
        total.reserve_all();
        let capacity = total.partials.capacity();
        let mut value = f64::MAX / 2.0;
        while value > 0.0 {
            total.add(value);
            total.add(-value / 7.0);
            value /= 3.0;
        }
        assert_eq!(total.partials.capacity(), capacity);
        assert!(capacity >= MAX_PARTIALS);
    }
}
