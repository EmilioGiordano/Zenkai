use zenkai_datagen::{ColumnKind, EmailFormat, Gender, LastNameCount};

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
        label: "Names",
        choices: vec![
            choice("Female and male", Gender::Any),
            choice("Female", Gender::Female),
            choice("Male", Gender::Male),
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
        label: "Last names",
        choices: vec![
            choice("One", LastNameCount::One),
            choice("Two", LastNameCount::Two),
            choice("One or two", LastNameCount::OneOrTwo),
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
                label: "Format",
                choices: vec![
                    choice("first.last@domain", EmailFormat::FirstDotLast),
                    choice("firstlast@domain", EmailFormat::FirstLast),
                    choice("flast@domain", EmailFormat::InitialLast),
                ],
            }]
        }
        _ => Vec::new(),
    }
}

pub fn summary(kind: &ColumnKind) -> String {
    let names = |gender: &Gender| match gender {
        Gender::Any => "female and male",
        Gender::Female => "female",
        Gender::Male => "male",
    };
    let last_names = |count: &LastNameCount| match count {
        LastNameCount::One => "one last name",
        LastNameCount::Two => "two last names",
        LastNameCount::OneOrTwo => "one or two last names",
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
                EmailFormat::FirstDotLast => "first.last",
                EmailFormat::FirstLast => "firstlast",
                EmailFormat::InitialLast => "flast",
            };
            format!("{format}, {} domains", domains.len())
        }
        ColumnKind::Phone { pattern } | ColumnKind::Pattern { pattern } => pattern.clone(),
        ColumnKind::Integer { min, max } => format!("{min} to {max}"),
        ColumnKind::Decimal { min, max, places } => {
            format!("{min} to {max}, {places} decimals")
        }
        ColumnKind::Date { from, to } => {
            format!("{} to {}", String::from(*from), String::from(*to))
        }
        ColumnKind::OneOf { options } => options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        ColumnKind::SequentialId { start, step } => format!("from {start}, step {step}"),
        ColumnKind::Lorem {
            min_words,
            max_words,
        } => format!("{min_words} to {max_words} words"),
        ColumnKind::StreetAddress {}
        | ColumnKind::City {}
        | ColumnKind::Company {}
        | ColumnKind::Boolean {}
        | ColumnKind::Uuid {} => "No options".to_string(),
    }
}
