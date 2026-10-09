use zenkai_datagen::{ColumnKind, Date, InvalidDate, ListOption};
use zenkai_i18n::t;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum OptionError {
    #[error("{}", t!("gen.error.whole_number", field = .field, text = .text))]
    WholeNumber { field: &'static str, text: String },
    #[error("{}", t!("gen.error.number", field = .field, text = .text))]
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
            vec![field(t!("gen.field.domains"), domains.join(", "))]
        }
        ColumnKind::Phone { pattern } => vec![field(t!("gen.field.phone_pattern"), pattern)],
        ColumnKind::Pattern { pattern } => {
            vec![field(t!("gen.field.pattern"), pattern)]
        }
        ColumnKind::Integer { min, max } => vec![
            field(t!("gen.field.from"), min),
            field(t!("gen.field.to"), max),
        ],
        ColumnKind::Decimal { min, max, places } => vec![
            field(t!("gen.field.from"), min),
            field(t!("gen.field.to"), max),
            field(t!("gen.field.decimal_places"), places),
        ],
        ColumnKind::Date { from, to } => vec![
            field(t!("gen.field.date_from"), String::from(*from)),
            field(t!("gen.field.date_to"), String::from(*to)),
        ],
        ColumnKind::OneOf { options } => {
            let values: Vec<&str> = options.iter().map(|option| option.value.as_str()).collect();
            vec![field(t!("gen.field.values"), values.join(", "))]
        }
        ColumnKind::SequentialId { start, step } => {
            vec![
                field(t!("gen.field.start"), start),
                field(t!("gen.field.step"), step),
            ]
        }
        ColumnKind::Lorem {
            min_words,
            max_words,
        } => vec![
            field(t!("gen.field.fewest_words"), min_words),
            field(t!("gen.field.most_words"), max_words),
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
            min: whole(0, t!("gen.field.from"))?,
            max: whole(1, t!("gen.field.to"))?,
        },
        ColumnKind::Decimal { .. } => ColumnKind::Decimal {
            min: number(0, t!("gen.field.from"))?,
            max: number(1, t!("gen.field.to"))?,
            places: text(2)
                .parse::<u8>()
                .map_err(|_| OptionError::WholeNumber {
                    field: t!("gen.field.decimal_places"),
                    text: text(2).to_string(),
                })?,
        },
        ColumnKind::Date { .. } => ColumnKind::Date {
            from: date(0, t!("gen.field.from"))?,
            to: date(1, t!("gen.field.to"))?,
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
            start: whole(0, t!("gen.field.start"))?,
            step: whole(1, t!("gen.field.step"))?,
        },
        ColumnKind::Lorem { .. } => ColumnKind::Lorem {
            min_words: small(0, t!("gen.field.fewest_words"))?,
            max_words: small(1, t!("gen.field.most_words"))?,
        },
        unchanged => unchanged.clone(),
    })
}
