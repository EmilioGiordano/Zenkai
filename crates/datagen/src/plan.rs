use std::collections::HashSet;

use zenkai_types::{MAX_COLS, MAX_ROWS};

use crate::budget::{MAX_CELLS, MAX_OUTPUT_BYTES, estimated_output_bytes};
use crate::date::Date;
use crate::email::{EmailPlan, NamePart, NameSource};
use crate::error::{ColumnProblem, DatagenError, TextField};
use crate::limits::{
    MAX_DOMAIN_CHARS, MAX_DOMAINS, MAX_HEADER_CHARS, MAX_OPTION_CHARS, MAX_OPTIONS,
    MAX_PATTERN_CHARS, check_text,
};
use crate::locale::{self, LocaleData};
use crate::lorem;
use crate::pattern::{Pattern, Placeholders};
use crate::spec::{
    ColumnKind, ColumnSpec, EmailFormat, Gender, GenerationSpec, LastNameCount, ListOption,
};
use crate::value_set::{FirstNames, LastNames, ValueSet, WeightedOptions};

// Above 15 significant digits a spreadsheet cell no longer holds a number exactly.
pub(crate) const PRECISION_LIMIT: i64 = 999_999_999_999_999;
const MAX_DECIMAL_PLACES: u8 = 9;
// The header takes the first row of the sheet.
pub(crate) const MAX_DATA_ROWS: u32 = MAX_ROWS - 1;
// A table instead of powi, whose precision Rust leaves unspecified per platform.
const DECIMAL_SCALES: [f64; MAX_DECIMAL_PLACES as usize + 1] =
    [1.0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Uniqueness {
    Repeats,
    Unique,
}

pub(crate) enum Values {
    Set(ValueSet),
    Sequence { start: i64, step: i64 },
    Email(EmailPlan),
}

pub(crate) struct ColumnPlan {
    pub(crate) values: Values,
    pub(crate) blanks: usize,
    pub(crate) uniqueness: Uniqueness,
}

pub(crate) fn plan(spec: &GenerationSpec) -> Result<Vec<ColumnPlan>, DatagenError> {
    if spec.columns.is_empty() {
        return Err(DatagenError::NoColumns);
    }
    if spec.rows > MAX_DATA_ROWS {
        return Err(DatagenError::TooManyRows {
            requested: spec.rows,
            limit: MAX_DATA_ROWS,
        });
    }
    if spec.columns.len() > usize::from(MAX_COLS) {
        return Err(DatagenError::TooManyColumns {
            requested: spec.columns.len(),
            limit: MAX_COLS,
        });
    }
    let cells = u64::from(spec.rows) * spec.columns.len() as u64;
    if cells > MAX_CELLS {
        return Err(DatagenError::TooManyCells {
            requested: cells,
            limit: MAX_CELLS,
        });
    }
    let plans = spec
        .columns
        .iter()
        .enumerate()
        .map(|(position, column)| {
            plan_column(spec, column).map_err(|problem| DatagenError::Column {
                position,
                header: column.header.clone(),
                problem,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let estimated_bytes = estimated_output_bytes(spec);
    if estimated_bytes > MAX_OUTPUT_BYTES {
        return Err(DatagenError::OutputTooLarge {
            estimated_bytes,
            limit: MAX_OUTPUT_BYTES,
        });
    }
    Ok(plans)
}

fn plan_column(spec: &GenerationSpec, column: &ColumnSpec) -> Result<ColumnPlan, ColumnProblem> {
    check_text(&column.header, TextField::Header, MAX_HEADER_CHARS)?;
    let rows = spec.rows as usize;
    let blanks = column.blanks.of(rows);
    let uniqueness = if column.unique {
        Uniqueness::Unique
    } else {
        Uniqueness::Repeats
    };
    let values = column_values(spec, column, uniqueness)?;
    if let (Values::Set(set), Uniqueness::Unique) = (&values, uniqueness) {
        let needed = (rows - blanks) as u64;
        if set.domain() < u128::from(needed) {
            return Err(ColumnProblem::DomainTooSmall {
                needed,
                available: set.domain(),
            });
        }
    }
    Ok(ColumnPlan {
        values,
        blanks,
        uniqueness,
    })
}

fn column_values(
    spec: &GenerationSpec,
    column: &ColumnSpec,
    uniqueness: Uniqueness,
) -> Result<Values, ColumnProblem> {
    let locale = locale::data(spec.locale);
    let set = match &column.kind {
        ColumnKind::FirstName { gender } => ValueSet::FirstName(FirstNames::new(locale, *gender)),
        ColumnKind::LastName { count } => ValueSet::LastName(LastNames::new(locale, *count)),
        ColumnKind::FullName { gender, last_names } => ValueSet::FullName(
            FirstNames::new(locale, *gender),
            LastNames::new(locale, *last_names),
        ),
        ColumnKind::Email {
            format,
            first_name_from,
            last_name_from,
            domains,
        } => {
            return email_plan(
                spec,
                locale,
                *format,
                first_name_from.as_deref(),
                last_name_from.as_deref(),
                domains,
            )
            .map(Values::Email);
        }
        ColumnKind::Phone { pattern } => code_set(pattern, Placeholders::DigitsOnly)?,
        ColumnKind::Pattern { pattern } => code_set(pattern, Placeholders::LettersAndDigits)?,
        ColumnKind::StreetAddress {} => ValueSet::StreetAddress(locale),
        ColumnKind::City {} => ValueSet::City(locale.cities),
        ColumnKind::Company {} => ValueSet::Company(locale),
        ColumnKind::Integer { min, max } => integer_set(*min, *max)?,
        ColumnKind::Decimal { min, max, places } => decimal_set(*min, *max, *places)?,
        ColumnKind::Date { from, to } => date_set(*from, *to)?,
        ColumnKind::Boolean {} => ValueSet::Boolean,
        ColumnKind::OneOf { options } => one_of_set(options, uniqueness)?,
        ColumnKind::SequentialId { start, step } => {
            check_sequence(*start, *step, spec.rows)?;
            return Ok(Values::Sequence {
                start: *start,
                step: *step,
            });
        }
        ColumnKind::Uuid {} => ValueSet::Uuid,
        ColumnKind::Lorem {
            min_words,
            max_words,
        } => lorem_set(*min_words, *max_words)?,
    };
    Ok(Values::Set(set))
}

fn code_set(pattern: &str, placeholders: Placeholders) -> Result<ValueSet, ColumnProblem> {
    check_text(pattern, TextField::Pattern, MAX_PATTERN_CHARS)?;
    Ok(ValueSet::Code(Pattern::parse(pattern, placeholders)?))
}

fn check_precision(value: i128, shown: impl ToString) -> Result<(), ColumnProblem> {
    if value.abs() > i128::from(PRECISION_LIMIT) {
        return Err(ColumnProblem::BeyondPrecision {
            value: shown.to_string(),
            limit: PRECISION_LIMIT,
        });
    }
    Ok(())
}

fn check_order<T: PartialOrd + ToString>(min: T, max: T) -> Result<(), ColumnProblem> {
    if min > max {
        return Err(ColumnProblem::MinAboveMax {
            min: min.to_string(),
            max: max.to_string(),
        });
    }
    Ok(())
}

fn integer_set(min: i64, max: i64) -> Result<ValueSet, ColumnProblem> {
    check_order(min, max)?;
    check_precision(i128::from(min), min)?;
    check_precision(i128::from(max), max)?;
    Ok(ValueSet::Integer {
        min,
        span: (i128::from(max) - i128::from(min) + 1) as u128,
    })
}

fn decimal_set(min: f64, max: f64, places: u8) -> Result<ValueSet, ColumnProblem> {
    if places > MAX_DECIMAL_PLACES {
        return Err(ColumnProblem::TooManyPlaces {
            places,
            limit: MAX_DECIMAL_PLACES,
        });
    }
    let scale = DECIMAL_SCALES[usize::from(places)];
    let scaled = |value: f64| -> Result<i64, ColumnProblem> {
        if !value.is_finite() {
            return Err(ColumnProblem::NotFinite {
                value: value.to_string(),
            });
        }
        let rounded = (value * scale).round();
        if rounded.abs() > PRECISION_LIMIT as f64 {
            return Err(ColumnProblem::BeyondPrecision {
                value: value.to_string(),
                limit: PRECISION_LIMIT,
            });
        }
        Ok(rounded as i64)
    };
    let min_scaled = scaled(min)?;
    let max_scaled = scaled(max)?;
    check_order(min, max)?;
    Ok(ValueSet::Decimal {
        min_scaled,
        span: (max_scaled - min_scaled + 1) as u128,
        places,
    })
}

fn date_set(from: Date, to: Date) -> Result<ValueSet, ColumnProblem> {
    check_order(from, to)?;
    Ok(ValueSet::Date {
        from,
        span: (from.days_until(to) + 1) as u128,
    })
}

fn one_of_set(options: &[ListOption], uniqueness: Uniqueness) -> Result<ValueSet, ColumnProblem> {
    if options.is_empty() {
        return Err(ColumnProblem::EmptyList);
    }
    if options.len() > MAX_OPTIONS {
        return Err(ColumnProblem::TooManyOptions {
            count: options.len(),
            limit: MAX_OPTIONS,
        });
    }
    let mut seen = HashSet::new();
    for option in options {
        check_text(&option.value, TextField::ListValue, MAX_OPTION_CHARS)?;
        if option.value.is_empty() {
            return Err(ColumnProblem::EmptyOption);
        }
        if !seen.insert(option.value.as_str()) {
            return Err(ColumnProblem::DuplicateOption {
                value: option.value.clone(),
            });
        }
    }
    if options.iter().all(|option| option.weight == 0) {
        return Err(ColumnProblem::AllWeightsZero);
    }
    if uniqueness == Uniqueness::Unique && options.iter().any(|option| option.weight != 1) {
        return Err(ColumnProblem::WeightsWithUnique);
    }
    Ok(ValueSet::OneOf(WeightedOptions::new(
        options
            .iter()
            .map(|option| (option.value.clone(), option.weight)),
    )))
}

fn check_sequence(start: i64, step: i64, rows: u32) -> Result<(), ColumnProblem> {
    if step == 0 {
        return Err(ColumnProblem::ZeroStep);
    }
    check_precision(i128::from(start), start)?;
    let last = i128::from(start) + i128::from(step) * i128::from(rows.saturating_sub(1));
    check_precision(last, last)
}

fn lorem_set(min_words: u16, max_words: u16) -> Result<ValueSet, ColumnProblem> {
    if min_words == 0 || max_words > lorem::MAX_WORDS {
        return Err(ColumnProblem::WordCountOutOfRange {
            limit: lorem::MAX_WORDS,
        });
    }
    check_order(min_words, max_words)?;
    Ok(ValueSet::Lorem {
        min_words,
        max_words,
    })
}

fn email_plan(
    spec: &GenerationSpec,
    locale: &'static LocaleData,
    format: EmailFormat,
    first_name_from: Option<&str>,
    last_name_from: Option<&str>,
    domains: &[String],
) -> Result<EmailPlan, ColumnProblem> {
    if domains.is_empty() {
        return Err(ColumnProblem::NoDomains);
    }
    if domains.len() > MAX_DOMAINS {
        return Err(ColumnProblem::TooManyDomains {
            count: domains.len(),
            limit: MAX_DOMAINS,
        });
    }
    for domain in domains {
        check_text(domain, TextField::Domain, MAX_DOMAIN_CHARS)?;
    }
    if let Some(domain) = domains.iter().find(|domain| !is_valid_domain(domain)) {
        return Err(ColumnProblem::InvalidDomain {
            domain: domain.clone(),
        });
    }
    let first_name_from = first_name_from
        .map(|header| name_source(spec, header, NameRole::First))
        .transpose()?;
    let last_name_from = last_name_from
        .map(|header| name_source(spec, header, NameRole::Last))
        .transpose()?;
    Ok(EmailPlan {
        format,
        first_name_from,
        last_name_from,
        domains: domains.to_vec(),
        fallback_first_names: FirstNames::new(locale, Gender::Any),
        fallback_last_names: LastNames::new(locale, LastNameCount::One),
    })
}

#[derive(Clone, Copy)]
enum NameRole {
    First,
    Last,
}

fn name_source(
    spec: &GenerationSpec,
    header: &str,
    role: NameRole,
) -> Result<NameSource, ColumnProblem> {
    let mut matches = spec
        .columns
        .iter()
        .enumerate()
        .filter(|(_, column)| column.header == header);
    let (column, source) = match (matches.next(), matches.next()) {
        (Some(found), None) => found,
        (None, _) => {
            return Err(ColumnProblem::UnknownSource {
                source_header: header.to_string(),
            });
        }
        (Some(_), Some(_)) => {
            return Err(ColumnProblem::AmbiguousSource {
                source_header: header.to_string(),
            });
        }
    };
    let part = match (role, &source.kind) {
        (NameRole::First, ColumnKind::FirstName { .. })
        | (NameRole::Last, ColumnKind::LastName { .. }) => NamePart::WholeCell,
        (NameRole::First, ColumnKind::FullName { .. }) => NamePart::FirstWord,
        (NameRole::Last, ColumnKind::FullName { .. }) => NamePart::AfterFirstWord,
        (NameRole::First, _) => {
            return Err(ColumnProblem::NotFirstNameSource {
                source_header: header.to_string(),
            });
        }
        (NameRole::Last, _) => {
            return Err(ColumnProblem::NotLastNameSource {
                source_header: header.to_string(),
            });
        }
    };
    Ok(NameSource { column, part })
}

fn is_valid_domain(domain: &str) -> bool {
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}
