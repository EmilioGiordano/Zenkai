use std::collections::{HashMap, HashSet};

use crate::rng::Rng;
use crate::spec::EmailFormat;
use crate::text::fold;
use crate::value_set::{FirstNames, LastNames};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NamePart {
    WholeCell,
    FirstWord,
    AfterFirstWord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NameSource {
    pub(crate) column: usize,
    pub(crate) part: NamePart,
}

impl NameSource {
    fn read<'a>(&self, row_cells: &'a [String]) -> &'a str {
        let cell = row_cells[self.column].as_str();
        match self.part {
            NamePart::WholeCell => cell,
            NamePart::FirstWord => cell.split(' ').next().unwrap_or(cell),
            NamePart::AfterFirstWord => cell.split_once(' ').map_or("", |(_, rest)| rest),
        }
    }
}

pub(crate) struct EmailPlan {
    pub(crate) format: EmailFormat,
    pub(crate) first_name_from: Option<NameSource>,
    pub(crate) last_name_from: Option<NameSource>,
    pub(crate) domains: Vec<String>,
    pub(crate) fallback_first_names: FirstNames,
    pub(crate) fallback_last_names: LastNames,
}

#[derive(Default)]
pub(crate) struct TakenAddresses {
    taken: HashSet<String>,
    last_suffix: HashMap<String, u32>,
}

impl TakenAddresses {
    // Names repeat, so a taken address gets the next free number: lucia.fernandez2@...
    fn claim(&mut self, local: &str, domain: &str) -> String {
        let address = format!("{local}@{domain}");
        if self.taken.insert(address.clone()) {
            return address;
        }
        let suffix = self.last_suffix.entry(address).or_insert(1);
        loop {
            *suffix += 1;
            let candidate = format!("{local}{suffix}@{domain}");
            if self.taken.insert(candidate.clone()) {
                return candidate;
            }
        }
    }
}

pub(crate) enum Addresses {
    MayRepeat,
    Unique(TakenAddresses),
}

impl EmailPlan {
    pub(crate) fn address(
        &self,
        row_cells: &[String],
        rng: &mut Rng,
        addresses: &mut Addresses,
    ) -> String {
        let first = self.part(self.first_name_from, row_cells, || {
            self.fallback_first_names.sample(rng).to_string()
        });
        let last = self.part(self.last_name_from, row_cells, || {
            self.fallback_last_names.sample(rng)
        });
        let local = match self.format {
            EmailFormat::FirstDotLast => format!("{first}.{last}"),
            EmailFormat::FirstLast => format!("{first}{last}"),
            EmailFormat::InitialLast => format!("{}{last}", &first[..first.len().min(1)]),
        };
        let domain = &self.domains[rng.index(self.domains.len())];
        match addresses {
            Addresses::MayRepeat => format!("{local}@{domain}"),
            Addresses::Unique(taken) => taken.claim(&local, domain),
        }
    }

    fn part(
        &self,
        source: Option<NameSource>,
        row_cells: &[String],
        generate: impl FnOnce() -> String,
    ) -> String {
        let from_cell = source.map_or("", |source| source.read(row_cells));
        let word = email_word(from_cell);
        if word.is_empty() {
            email_word(&generate())
        } else {
            word
        }
    }
}

fn email_word(name: &str) -> String {
    fold(name)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_words_drop_accents_spaces_and_punctuation() {
        assert_eq!(email_word("Gómez Ruiz"), "gomezruiz");
        assert_eq!(email_word("Ñuñez-Ibáñez"), "nunezibanez");
    }

    #[test]
    fn taken_addresses_get_increasing_suffixes() {
        let mut taken = TakenAddresses::default();
        assert_eq!(taken.claim("ana.paz", "gmail.com"), "ana.paz@gmail.com");
        assert_eq!(taken.claim("ana.paz", "gmail.com"), "ana.paz2@gmail.com");
        assert_eq!(taken.claim("ana.paz", "gmail.com"), "ana.paz3@gmail.com");
        assert_eq!(taken.claim("ana.paz", "yahoo.com"), "ana.paz@yahoo.com");
    }

    #[test]
    fn one_name_on_one_domain_claims_each_suffix_once() {
        let mut taken = TakenAddresses::default();
        for row in 1..=50_000u32 {
            let expected = match row {
                1 => "ana@x.com".to_string(),
                _ => format!("ana{row}@x.com"),
            };
            assert_eq!(taken.claim("ana", "x.com"), expected);
        }
        assert_eq!(taken.claim("ana50001", "x.com"), "ana50001@x.com");
        assert_eq!(taken.claim("ana", "x.com"), "ana50002@x.com");
    }
}
