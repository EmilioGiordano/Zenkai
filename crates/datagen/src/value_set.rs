use crate::date::Date;
use crate::locale::{LocaleData, StreetOrder};
use crate::lorem;
use crate::pattern::Pattern;
use crate::rng::Rng;
use crate::spec::{Gender, LastNameCount};
use crate::text::literal_input;

const HOUSE_NUMBERS: u128 = 9_999;
const UUID_DOMAIN: u128 = 1 << 122;

pub(crate) struct FirstNames {
    female: &'static [&'static str],
    male: &'static [&'static str],
}

impl FirstNames {
    pub(crate) fn new(locale: &'static LocaleData, gender: Gender) -> FirstNames {
        let none: &'static [&'static str] = &[];
        let (female, male) = match gender {
            Gender::Any => (locale.female_names, locale.male_names),
            Gender::Female => (locale.female_names, none),
            Gender::Male => (none, locale.male_names),
        };
        FirstNames { female, male }
    }

    fn len(&self) -> u128 {
        (self.female.len() + self.male.len()) as u128
    }

    fn at(&self, index: u128) -> &'static str {
        let index = index as usize;
        match self.female.get(index) {
            Some(name) => name,
            None => self.male[index - self.female.len()],
        }
    }

    pub(crate) fn sample(&self, rng: &mut Rng) -> &'static str {
        self.at(rng.below(self.len()))
    }
}

pub(crate) struct LastNames {
    names: &'static [&'static str],
    count: LastNameCount,
}

impl LastNames {
    pub(crate) fn new(locale: &'static LocaleData, count: LastNameCount) -> LastNames {
        LastNames {
            names: locale.last_names,
            count,
        }
    }

    fn singles(&self) -> u128 {
        self.names.len() as u128
    }

    fn pairs(&self) -> u128 {
        self.singles() * (self.singles() - 1)
    }

    fn len(&self) -> u128 {
        match self.count {
            LastNameCount::One => self.singles(),
            LastNameCount::Two => self.pairs(),
            LastNameCount::OneOrTwo => self.singles() + self.pairs(),
        }
    }

    fn at(&self, index: u128) -> String {
        match self.count {
            LastNameCount::One => self.single(index),
            LastNameCount::Two => self.pair(index),
            LastNameCount::OneOrTwo if index < self.singles() => self.single(index),
            LastNameCount::OneOrTwo => self.pair(index - self.singles()),
        }
    }

    fn single(&self, index: u128) -> String {
        self.names[index as usize].to_string()
    }

    // Two different surnames: the second skips over the first.
    fn pair(&self, index: u128) -> String {
        let others = self.singles() - 1;
        let first = index / others;
        let mut second = index % others;
        if second >= first {
            second += 1;
        }
        format!("{} {}", self.single(first), self.single(second))
    }

    pub(crate) fn sample(&self, rng: &mut Rng) -> String {
        match self.count {
            LastNameCount::OneOrTwo if rng.below(4) == 0 => self.pair(rng.below(self.pairs())),
            LastNameCount::OneOrTwo => self.single(rng.below(self.singles())),
            _ => self.at(rng.below(self.len())),
        }
    }
}

pub(crate) struct WeightedOptions {
    values: Vec<String>,
    cumulative_weights: Vec<u64>,
    total_weight: u64,
}

impl WeightedOptions {
    pub(crate) fn new(options: impl Iterator<Item = (String, u32)>) -> WeightedOptions {
        let mut total = 0u64;
        let (values, cumulative_weights) = options
            .map(|(value, weight)| {
                total += u64::from(weight);
                (literal_input(value), total)
            })
            .unzip();
        WeightedOptions {
            values,
            cumulative_weights,
            total_weight: total,
        }
    }

    fn sample(&self, rng: &mut Rng) -> String {
        let draw = rng.below(u128::from(self.total_weight)) as u64;
        let chosen = self
            .cumulative_weights
            .partition_point(|&reached| reached <= draw);
        self.values[chosen].clone()
    }
}

pub(crate) enum ValueSet {
    FirstName(FirstNames),
    LastName(LastNames),
    FullName(FirstNames, LastNames),
    Code(Pattern),
    StreetAddress(&'static LocaleData),
    City(&'static [&'static str]),
    Company(&'static LocaleData),
    Integer {
        min: i64,
        span: u128,
    },
    Decimal {
        min_scaled: i64,
        span: u128,
        places: u8,
    },
    Date {
        from: Date,
        span: u128,
    },
    Boolean,
    OneOf(WeightedOptions),
    Uuid,
    Lorem {
        min_words: u16,
        max_words: u16,
    },
}

impl ValueSet {
    pub(crate) fn domain(&self) -> u128 {
        match self {
            ValueSet::FirstName(names) => names.len(),
            ValueSet::LastName(names) => names.len(),
            ValueSet::FullName(first, last) => first.len() * last.len(),
            ValueSet::Code(pattern) => pattern.domain(),
            ValueSet::StreetAddress(locale) => locale.streets.len() as u128 * HOUSE_NUMBERS,
            ValueSet::City(cities) => cities.len() as u128,
            ValueSet::Company(locale) => {
                (locale.company_words.len() * locale.company_suffixes.len()) as u128
            }
            ValueSet::Integer { span, .. }
            | ValueSet::Decimal { span, .. }
            | ValueSet::Date { span, .. } => *span,
            ValueSet::Boolean => 2,
            ValueSet::OneOf(options) => options.values.len() as u128,
            ValueSet::Uuid => UUID_DOMAIN,
            ValueSet::Lorem {
                min_words,
                max_words,
            } => (*min_words..=*max_words)
                .map(lorem_sentences)
                .fold(0u128, u128::saturating_add),
        }
    }

    pub(crate) fn sample(&self, rng: &mut Rng) -> String {
        match self {
            ValueSet::LastName(names) => names.sample(rng),
            ValueSet::FullName(first, last) => {
                format!("{} {}", first.sample(rng), last.sample(rng))
            }
            ValueSet::OneOf(options) => options.sample(rng),
            ValueSet::Lorem {
                min_words,
                max_words,
            } => {
                let words = rng.below(u128::from(max_words - min_words) + 1) as usize
                    + usize::from(*min_words);
                lorem::sentence((0..words).map(|_| lorem::WORDS[rng.index(lorem::WORDS.len())]))
            }
            _ => self.value_at(rng.below(self.domain())),
        }
    }

    pub(crate) fn value_at(&self, index: u128) -> String {
        match self {
            ValueSet::FirstName(names) => names.at(index).to_string(),
            ValueSet::LastName(names) => names.at(index),
            ValueSet::FullName(first, last) => {
                let first_count = first.len();
                format!(
                    "{} {}",
                    first.at(index % first_count),
                    last.at(index / first_count)
                )
            }
            ValueSet::Code(pattern) => literal_input(pattern.value_at(index)),
            ValueSet::StreetAddress(locale) => {
                let street = locale.streets[(index / HOUSE_NUMBERS) as usize];
                let number = index % HOUSE_NUMBERS + 1;
                // A number, a space and words: the engine reads it as text, unquoted.
                match locale.street_order {
                    StreetOrder::NameThenNumber => format!("{street} {number}"),
                    StreetOrder::NumberThenName => format!("{number} {street}"),
                }
            }
            ValueSet::City(cities) => cities[index as usize].to_string(),
            ValueSet::Company(locale) => {
                let suffixes = locale.company_suffixes.len() as u128;
                format!(
                    "{} {}",
                    locale.company_words[(index / suffixes) as usize],
                    locale.company_suffixes[(index % suffixes) as usize]
                )
            }
            ValueSet::Integer { min, .. } => (i128::from(*min) + index as i128).to_string(),
            ValueSet::Decimal {
                min_scaled, places, ..
            } => fixed_point(i128::from(*min_scaled) + index as i128, *places),
            ValueSet::Date { from, .. } => from.plus_days(index as i64).to_string(),
            ValueSet::Boolean if index == 0 => "FALSE".to_string(),
            ValueSet::Boolean => "TRUE".to_string(),
            ValueSet::OneOf(options) => options.values[index as usize].clone(),
            ValueSet::Uuid => literal_input(uuid_v4(index)),
            ValueSet::Lorem { min_words, .. } => lorem_at(index, *min_words),
        }
    }
}

fn lorem_sentences(words: u16) -> u128 {
    (lorem::WORDS.len() as u128).saturating_pow(u32::from(words))
}

fn lorem_at(index: u128, min_words: u16) -> String {
    let mut remaining = index;
    let mut words = min_words;
    while remaining >= lorem_sentences(words) {
        remaining -= lorem_sentences(words);
        words += 1;
    }
    let base = lorem::WORDS.len() as u128;
    lorem::sentence((0..words).map(|_| {
        let word = lorem::WORDS[(remaining % base) as usize];
        remaining /= base;
        word
    }))
}

fn fixed_point(scaled: i128, places: u8) -> String {
    if places == 0 {
        return scaled.to_string();
    }
    let unit = 10u128.pow(u32::from(places));
    let sign = if scaled < 0 { "-" } else { "" };
    let magnitude = scaled.unsigned_abs();
    format!(
        "{sign}{}.{:0width$}",
        magnitude / unit,
        magnitude % unit,
        width = usize::from(places)
    )
}

fn uuid_v4(random_bits: u128) -> String {
    let time_low_and_mid = random_bits >> 74;
    let time_high = (random_bits >> 62) & 0xFFF;
    let rest = random_bits & ((1 << 62) - 1);
    let value = (time_low_and_mid << 80) | (0x4 << 76) | (time_high << 64) | (0b10 << 62) | rest;
    let hex = format!("{value:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::data;
    use crate::spec::Locale;

    #[test]
    fn fixed_point_pads_fractions_and_keeps_signs() {
        assert_eq!(fixed_point(124_990, 2), "1249.90");
        assert_eq!(fixed_point(-5, 2), "-0.05");
        assert_eq!(fixed_point(-1_250, 3), "-1.250");
        assert_eq!(fixed_point(42, 0), "42");
    }

    #[test]
    fn uuid_sets_version_and_variant() {
        assert_eq!(uuid_v4(0), "00000000-0000-4000-8000-000000000000");
        assert_eq!(
            uuid_v4(UUID_DOMAIN - 1),
            "ffffffff-ffff-4fff-bfff-ffffffffffff"
        );
    }

    #[test]
    fn every_index_of_a_small_domain_is_a_different_value() {
        let locale = data(Locale::SpanishArgentina);
        let sets = [
            ValueSet::LastName(LastNames::new(locale, LastNameCount::OneOrTwo)),
            ValueSet::FullName(
                FirstNames::new(locale, Gender::Female),
                LastNames::new(locale, LastNameCount::One),
            ),
            ValueSet::Company(locale),
            ValueSet::Lorem {
                min_words: 1,
                max_words: 2,
            },
        ];
        for set in sets {
            let values: std::collections::HashSet<String> =
                (0..set.domain()).map(|index| set.value_at(index)).collect();
            assert_eq!(values.len() as u128, set.domain());
        }
    }

    #[test]
    fn weighted_options_skip_zero_weights() {
        let options = WeightedOptions::new(
            [
                ("a".to_string(), 0),
                ("b".to_string(), 3),
                ("c".to_string(), 0),
            ]
            .into_iter(),
        );
        let mut rng = Rng::new(3, 0, crate::rng::Stream::Values);
        assert!((0..100).all(|_| options.sample(&mut rng) == "b"));
    }
}
