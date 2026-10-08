mod en_us;
mod es_ar;

use crate::spec::Locale;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreetOrder {
    NameThenNumber,
    NumberThenName,
}

// Common names are public facts: en-US follows the SSA and 2010 Census rankings, es-AR is
// common Argentine names. Cities and streets are real places; company words are invented.
pub(crate) struct LocaleData {
    pub(crate) female_names: &'static [&'static str],
    pub(crate) male_names: &'static [&'static str],
    pub(crate) last_names: &'static [&'static str],
    pub(crate) cities: &'static [&'static str],
    pub(crate) streets: &'static [&'static str],
    pub(crate) street_order: StreetOrder,
    pub(crate) company_words: &'static [&'static str],
    pub(crate) company_suffixes: &'static [&'static str],
    pub(crate) phone_pattern: &'static str,
    pub(crate) email_domains: &'static [&'static str],
}

pub(crate) fn data(locale: Locale) -> &'static LocaleData {
    match locale {
        Locale::SpanishArgentina => &es_ar::DATA,
        Locale::EnglishUnitedStates => &en_us::DATA,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn all_locales() -> [&'static LocaleData; 2] {
        [
            data(Locale::SpanishArgentina),
            data(Locale::EnglishUnitedStates),
        ]
    }

    fn assert_distinct(list: &[&str]) {
        let mut seen = HashSet::new();
        for item in list {
            assert!(seen.insert(*item), "{item} is listed twice");
        }
    }

    #[test]
    fn lists_are_distinct_and_names_are_single_words() {
        for locale in all_locales() {
            let mut names = locale.female_names.to_vec();
            names.extend_from_slice(locale.male_names);
            for list in [
                names.as_slice(),
                locale.last_names,
                locale.cities,
                locale.streets,
                locale.company_words,
                locale.company_suffixes,
                locale.email_domains,
            ] {
                assert!(list.len() >= 4);
                assert_distinct(list);
            }
            for name in names.iter().chain(locale.last_names) {
                assert!(name.chars().all(char::is_alphabetic), "{name}");
                assert!(
                    crate::text::fold(name)
                        .chars()
                        .all(|c| c.is_ascii_lowercase()),
                    "{name} has a letter the email folding does not cover"
                );
            }
        }
    }
}
