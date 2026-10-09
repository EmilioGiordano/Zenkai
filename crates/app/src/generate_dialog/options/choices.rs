use zenkai_datagen::{ColumnKind, EmailFormat, Gender, LastNameCount};
use zenkai_i18n::t;

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: &'static str,
    pub selected: bool,
    pub kind: ColumnKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChoiceGroup {
    pub label: &'static str,
    pub choices: Vec<Choice>,
}

fn gender_group(kind: &ColumnKind, current: Gender) -> ChoiceGroup {
    let choice = |label, gender| {
        let mut changed = kind.clone();
        match &mut changed {
            ColumnKind::FirstName { gender: slot } | ColumnKind::FullName { gender: slot, .. } => {
                *slot = gender
            }
            _ => {}
        }
        Choice {
            label,
            selected: current == gender,
            kind: changed,
        }
    };
    ChoiceGroup {
        label: t!("gen.names"),
        choices: vec![
            choice(t!("gen.gender_any"), Gender::Any),
            choice(t!("gen.gender_female"), Gender::Female),
            choice(t!("gen.gender_male"), Gender::Male),
        ],
    }
}

fn last_names_group(kind: &ColumnKind, current: LastNameCount) -> ChoiceGroup {
    let choice = |label, count| {
        let mut changed = kind.clone();
        match &mut changed {
            ColumnKind::LastName { count: slot }
            | ColumnKind::FullName {
                last_names: slot, ..
            } => *slot = count,
            _ => {}
        }
        Choice {
            label,
            selected: current == count,
            kind: changed,
        }
    };
    ChoiceGroup {
        label: t!("gen.last_names"),
        choices: vec![
            choice(t!("gen.last_one"), LastNameCount::One),
            choice(t!("gen.last_two"), LastNameCount::Two),
            choice(t!("gen.last_one_or_two"), LastNameCount::OneOrTwo),
        ],
    }
}

pub fn choice_groups(kind: &ColumnKind) -> Vec<ChoiceGroup> {
    match kind {
        ColumnKind::FirstName { gender } => vec![gender_group(kind, *gender)],
        ColumnKind::LastName { count } => vec![last_names_group(kind, *count)],
        ColumnKind::FullName { gender, last_names } => vec![
            gender_group(kind, *gender),
            last_names_group(kind, *last_names),
        ],
        ColumnKind::Email { format, .. } => {
            let choice = |label, wanted| {
                let mut changed = kind.clone();
                if let ColumnKind::Email { format: slot, .. } = &mut changed {
                    *slot = wanted;
                }
                Choice {
                    label,
                    selected: *format == wanted,
                    kind: changed,
                }
            };
            vec![ChoiceGroup {
                label: t!("gen.email_format"),
                choices: vec![
                    choice(t!("gen.email_first_dot_last"), EmailFormat::FirstDotLast),
                    choice(t!("gen.email_first_last"), EmailFormat::FirstLast),
                    choice(t!("gen.email_initial_last"), EmailFormat::InitialLast),
                ],
            }]
        }
        _ => Vec::new(),
    }
}

pub fn summary(kind: &ColumnKind) -> String {
    let names = |gender: &Gender| match gender {
        Gender::Any => t!("gen.summary_gender_any"),
        Gender::Female => t!("gen.summary_gender_female"),
        Gender::Male => t!("gen.summary_gender_male"),
    };
    let last_names = |count: &LastNameCount| match count {
        LastNameCount::One => t!("gen.summary_last_one"),
        LastNameCount::Two => t!("gen.summary_last_two"),
        LastNameCount::OneOrTwo => t!("gen.summary_last_one_or_two"),
    };
    match kind {
        ColumnKind::FirstName { gender } => names(gender).to_string(),
        ColumnKind::LastName { count } => last_names(count).to_string(),
        ColumnKind::FullName {
            gender,
            last_names: count,
        } => format!("{}, {}", names(gender), last_names(count)),
        ColumnKind::Email {
            format, domains, ..
        } => {
            let format = match format {
                EmailFormat::FirstDotLast => t!("gen.summary_email_first_dot_last"),
                EmailFormat::FirstLast => t!("gen.summary_email_first_last"),
                EmailFormat::InitialLast => t!("gen.summary_email_initial_last"),
            };
            t!("gen.summary_email", count = domains.len(), format = format)
        }
        ColumnKind::Phone { pattern } | ColumnKind::Pattern { pattern } => pattern.clone(),
        ColumnKind::Integer { min, max } => t!("gen.summary_range", min = min, max = max),
        ColumnKind::Decimal { min, max, places } => {
            t!("gen.summary_decimal", min = min, max = max, places = places)
        }
        ColumnKind::Date { from, to } => {
            t!(
                "gen.summary_range",
                min = String::from(*from),
                max = String::from(*to)
            )
        }
        ColumnKind::OneOf { options } => options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        ColumnKind::SequentialId { start, step } => {
            t!("gen.summary_sequence", start = start, step = step)
        }
        ColumnKind::Lorem {
            min_words,
            max_words,
        } => t!("gen.summary_words", min = min_words, max = max_words),
        ColumnKind::StreetAddress {}
        | ColumnKind::City {}
        | ColumnKind::Company {}
        | ColumnKind::Boolean {}
        | ColumnKind::Uuid {} => t!("gen.no_options").to_string(),
    }
}
