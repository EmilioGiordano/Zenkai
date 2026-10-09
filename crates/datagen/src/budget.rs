use crate::limits::{MAX_OPTION_CHARS, MAX_PATTERN_CHARS};
use crate::locale::LocaleData;
use crate::lorem;
use crate::spec::{ColumnKind, GenerationSpec};

// Five times the 100k x 10 table the dialog targets, about a second of generation.
pub(crate) const MAX_CELLS: u64 = 5_000_000;
// Half a GiB: the generated table must fit beside the open workbook on an 8 GB machine.
pub(crate) const MAX_OUTPUT_BYTES: u64 = 512 * 1024 * 1024;
// Excel's limit on the characters in one cell.
pub(crate) const CELL_CHAR_LIMIT: usize = 32_767;

// i64::MIN has 20 characters; decimals hold 15 digits plus sign and point.
const NUMBER_BYTES: usize = 20;
const DATE_BYTES: usize = "yyyy-mm-dd".len();
const BOOLEAN_BYTES: usize = "FALSE".len();
const UUID_BYTES: usize = "'".len() + 36;
const HOUSE_NUMBER_BYTES: usize = " 9999".len();
// The number appended to a repeated address never passes the row count.
const EMAIL_SUFFIX_BYTES: usize = 7;
const CELL_OVERHEAD_BYTES: u64 = size_of::<String>() as u64;
const ROW_OVERHEAD_BYTES: u64 = size_of::<Vec<String>>() as u64;
// The caps on user text keep every generated value inside one cell; names are tested below.
const _: () = assert!(MAX_PATTERN_CHARS < CELL_CHAR_LIMIT);
const _: () = assert!(MAX_OPTION_CHARS < CELL_CHAR_LIMIT);
const _: () = assert!(lorem::MAX_WORDS as usize * (lorem::LONGEST_WORD + 1) <= CELL_CHAR_LIMIT);

pub(crate) fn estimated_output_bytes(spec: &GenerationSpec) -> u64 {
    let locale = crate::locale::data(spec.locale);
    let row_bytes: u64 = spec
        .columns
        .iter()
        .map(|column| max_cell_bytes(&column.kind, locale) as u64 + CELL_OVERHEAD_BYTES)
        .sum::<u64>()
        + ROW_OVERHEAD_BYTES;
    u64::from(spec.rows) * row_bytes
}

// An upper bound in bytes, so also in characters, of one generated cell of a valid kind.
pub(crate) fn max_cell_bytes(kind: &ColumnKind, locale: &LocaleData) -> usize {
    let first_name = longest(locale.female_names).max(longest(locale.male_names));
    let last_names = 2 * longest(locale.last_names) + 1;
    match kind {
        ColumnKind::FirstName { .. } => first_name,
        ColumnKind::LastName { .. } => last_names,
        ColumnKind::FullName { .. } => first_name + 1 + last_names,
        ColumnKind::Email { domains, .. } => {
            let domain = domains.iter().map(String::len).max().unwrap_or(0);
            first_name + ".".len() + last_names + EMAIL_SUFFIX_BYTES + "@".len() + domain
        }
        ColumnKind::Phone { pattern } | ColumnKind::Pattern { pattern } => {
            "'".len() + pattern.len()
        }
        ColumnKind::StreetAddress {} => longest(locale.streets) + HOUSE_NUMBER_BYTES,
        ColumnKind::City {} => longest(locale.cities),
        ColumnKind::Company {} => {
            longest(locale.company_words) + 1 + longest(locale.company_suffixes)
        }
        ColumnKind::Integer { .. }
        | ColumnKind::Decimal { .. }
        | ColumnKind::SequentialId { .. } => NUMBER_BYTES,
        ColumnKind::Date { .. } => DATE_BYTES,
        ColumnKind::Boolean {} => BOOLEAN_BYTES,
        ColumnKind::OneOf { options } => {
            "'".len()
                + options
                    .iter()
                    .map(|option| option.value.len())
                    .max()
                    .unwrap_or(0)
        }
        ColumnKind::Uuid {} => UUID_BYTES,
        ColumnKind::Lorem { max_words, .. } => usize::from(*max_words) * (lorem::LONGEST_WORD + 1),
    }
}

fn longest(list: &[&str]) -> usize {
    list.iter().map(|item| item.len()).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::generate;
    use crate::limits::MAX_DOMAIN_CHARS;
    use crate::spec::{ColumnSpec, EmailFormat, Gender, LastNameCount, ListOption, Locale};
    use crate::{Date, Percent};

    fn widest_columns() -> Vec<ColumnSpec> {
        let long_domain = format!("{}.com", "d".repeat(MAX_DOMAIN_CHARS - 4));
        let kinds = [
            ColumnKind::FirstName {
                gender: Gender::Any,
            },
            ColumnKind::LastName {
                count: LastNameCount::Two,
            },
            ColumnKind::FullName {
                gender: Gender::Any,
                last_names: LastNameCount::Two,
            },
            ColumnKind::Email {
                format: EmailFormat::FirstDotLast,
                first_name_from: None,
                last_name_from: None,
                domains: vec![long_domain],
            },
            ColumnKind::Phone {
                pattern: "#".repeat(MAX_PATTERN_CHARS),
            },
            ColumnKind::Pattern {
                pattern: "ñ#".repeat(MAX_PATTERN_CHARS / 2),
            },
            ColumnKind::StreetAddress {},
            ColumnKind::City {},
            ColumnKind::Company {},
            ColumnKind::Integer {
                min: -999_999_999_999_999,
                max: -999_999_999_999_990,
            },
            ColumnKind::Decimal {
                min: -9_999_999.0,
                max: -9_999_998.0,
                places: 8,
            },
            ColumnKind::SequentialId {
                start: -999_999_999_999_000,
                step: -1,
            },
            ColumnKind::Date {
                from: Date::parse("2020-01-01").unwrap(),
                to: Date::parse("2020-12-31").unwrap(),
            },
            ColumnKind::Boolean {},
            ColumnKind::OneOf {
                options: vec![ListOption {
                    value: "0".repeat(MAX_OPTION_CHARS),
                    weight: 1,
                }],
            },
            ColumnKind::Uuid {},
            ColumnKind::Lorem {
                min_words: lorem::MAX_WORDS,
                max_words: lorem::MAX_WORDS,
            },
        ];
        kinds
            .into_iter()
            .enumerate()
            .map(|(position, kind)| ColumnSpec {
                header: format!("C{position}"),
                kind,
                blanks: Percent::ZERO,
                unique: false,
            })
            .collect()
    }

    #[test]
    fn output_budget_passes_at_the_limit_and_fails_one_row_above() {
        let lorem = ColumnSpec {
            header: "Notas".to_string(),
            kind: ColumnKind::Lorem {
                min_words: 1,
                max_words: lorem::MAX_WORDS,
            },
            blanks: Percent::ZERO,
            unique: false,
        };
        let mut spec = GenerationSpec {
            rows: 1,
            locale: Locale::SpanishArgentina,
            seed: 1,
            columns: vec![lorem],
        };
        let row_bytes = estimated_output_bytes(&spec);
        spec.rows = u32::try_from(MAX_OUTPUT_BYTES / row_bytes).unwrap();
        assert!(crate::plan::plan(&spec).is_ok());
        spec.rows += 1;
        assert!(matches!(
            crate::plan::plan(&spec),
            Err(crate::DatagenError::OutputTooLarge {
                limit: MAX_OUTPUT_BYTES,
                ..
            })
        ));
    }

    #[test]
    fn every_kind_at_its_limits_fits_a_cell_and_its_estimate() {
        for locale in [Locale::SpanishArgentina, Locale::EnglishUnitedStates] {
            let columns = widest_columns();
            let spec = GenerationSpec {
                rows: 300,
                locale,
                seed: 5,
                columns: columns.clone(),
            };
            let rows = generate(&spec).unwrap();
            for (position, column) in columns.iter().enumerate() {
                let bound = max_cell_bytes(&column.kind, crate::locale::data(locale));
                assert!(bound <= CELL_CHAR_LIMIT, "{}", column.header);
                for row in &rows {
                    assert!(
                        row[position].len() <= bound,
                        "{}: {}",
                        column.header,
                        row[position]
                    );
                }
            }
        }
    }

    #[test]
    fn number_estimates_hold_the_longest_i64() {
        let longest = i64::MIN.to_string().len();
        let locale = crate::locale::data(Locale::EnglishUnitedStates);
        for kind in [
            ColumnKind::Integer {
                min: i64::MIN,
                max: i64::MIN + 1,
            },
            ColumnKind::SequentialId {
                start: i64::MIN,
                step: 1,
            },
        ] {
            assert!(max_cell_bytes(&kind, locale) >= longest);
        }
    }

    #[test]
    fn the_email_suffix_estimate_holds_the_largest_row_number() {
        let digits = (crate::plan::MAX_DATA_ROWS).to_string().len();
        assert!(EMAIL_SUFFIX_BYTES >= digits);
    }
}
