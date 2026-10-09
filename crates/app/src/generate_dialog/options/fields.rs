use zenkai_datagen::{ColumnKind, Date, InvalidDate, ListOption};

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum OptionError {
    #[error("{field} must be a whole number, not \"{text}\"")]
    WholeNumber { field: &'static str, text: String },
    #[error("{field} must be a number, not \"{text}\"")]
    Number { field: &'static str, text: String },
    #[error("{field}: {source}")]
    Date {
        field: &'static str,
        #[source]
        source: InvalidDate,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub label: &'static str,
    pub value: String,
}

fn field(label: &'static str, value: impl ToString) -> Field {
    Field {
        label,
        value: value.to_string(),
    }
}

pub fn fields(kind: &ColumnKind) -> Vec<Field> {
    match kind {
        ColumnKind::Email { domains, .. } => {
            vec![field("Domains, separated by commas", domains.join(", "))]
        }
        ColumnKind::Phone { pattern } => vec![field("Pattern, # is a digit", pattern)],
        ColumnKind::Pattern { pattern } => {
            vec![field("Pattern, A is a letter and # a digit", pattern)]
        }
        ColumnKind::Integer { min, max } => vec![field("From", min), field("To", max)],
        ColumnKind::Decimal { min, max, places } => vec![
            field("From", min),
            field("To", max),
            field("Decimal places", places),
        ],
        ColumnKind::Date { from, to } => vec![
            field("From, yyyy-mm-dd", String::from(*from)),
            field("To, yyyy-mm-dd", String::from(*to)),
        ],
        ColumnKind::OneOf { options } => {
            let values: Vec<&str> = options.iter().map(|option| option.value.as_str()).collect();
            vec![field("Values, separated by commas", values.join(", "))]
        }
        ColumnKind::SequentialId { start, step } => {
            vec![field("Start", start), field("Step", step)]
        }
        ColumnKind::Lorem {
            min_words,
            max_words,
        } => vec![
            field("Fewest words", min_words),
            field("Most words", max_words),
        ],
        ColumnKind::FirstName { .. }
        | ColumnKind::LastName { .. }
        | ColumnKind::FullName { .. }
        | ColumnKind::StreetAddress {}
        | ColumnKind::City {}
        | ColumnKind::Company {}
        | ColumnKind::Boolean {}
        | ColumnKind::Uuid {} => Vec::new(),
    }
}

pub fn with_fields(kind: &ColumnKind, values: &[String]) -> Result<ColumnKind, OptionError> {
    let text = |index: usize| values.get(index).map_or("", |value| value.trim());
    let whole = |index: usize, field: &'static str| {
        text(index)
            .parse::<i64>()
            .map_err(|_| OptionError::WholeNumber {
                field,
                text: text(index).to_string(),
            })
    };
    let small = |index: usize, field: &'static str| {
        text(index)
            .parse::<u16>()
            .map_err(|_| OptionError::WholeNumber {
                field,
                text: text(index).to_string(),
            })
    };
    let number = |index: usize, field: &'static str| {
        let typed = text(index);
        typed
            .parse::<f64>()
            .or_else(|_| typed.replace(',', ".").parse::<f64>())
            .map_err(|_| OptionError::Number {
                field,
                text: typed.to_string(),
            })
    };
    let date = |index: usize, field: &'static str| {
        Date::parse(text(index)).map_err(|source| OptionError::Date { field, source })
    };
    let list = |index: usize| -> Vec<String> {
        text(index)
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect()
    };
    Ok(match kind {
        ColumnKind::Email {
            format,
            first_name_from,
            last_name_from,
            ..
        } => ColumnKind::Email {
            format: *format,
            first_name_from: first_name_from.clone(),
            last_name_from: last_name_from.clone(),
            domains: list(0),
        },
        ColumnKind::Phone { .. } => ColumnKind::Phone {
            pattern: values.first().cloned().unwrap_or_default(),
        },
        ColumnKind::Pattern { .. } => ColumnKind::Pattern {
            pattern: values.first().cloned().unwrap_or_default(),
        },
        ColumnKind::Integer { .. } => ColumnKind::Integer {
            min: whole(0, "From")?,
            max: whole(1, "To")?,
        },
        ColumnKind::Decimal { .. } => ColumnKind::Decimal {
            min: number(0, "From")?,
            max: number(1, "To")?,
            places: text(2)
                .parse::<u8>()
                .map_err(|_| OptionError::WholeNumber {
                    field: "Decimal places",
                    text: text(2).to_string(),
                })?,
        },
        ColumnKind::Date { .. } => ColumnKind::Date {
            from: date(0, "From")?,
            to: date(1, "To")?,
        },
        ColumnKind::OneOf { options } => ColumnKind::OneOf {
            options: list(0)
                .into_iter()
                .map(|value| {
                    let weight = options
                        .iter()
                        .find(|option| option.value == value)
                        .map_or(1, |option| option.weight);
                    ListOption { value, weight }
                })
                .collect(),
        },
        ColumnKind::SequentialId { .. } => ColumnKind::SequentialId {
            start: whole(0, "Start")?,
            step: whole(1, "Step")?,
        },
        ColumnKind::Lorem { .. } => ColumnKind::Lorem {
            min_words: small(0, "Fewest words")?,
            max_words: small(1, "Most words")?,
        },
        unchanged => unchanged.clone(),
    })
}
