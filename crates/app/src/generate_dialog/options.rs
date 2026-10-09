use zenkai_datagen::{
    ColumnKind, Date, EmailFormat, Gender, LastNameCount, ListOption, Locale, detect_kind,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindChoice {
    FirstName,
    LastName,
    FullName,
    Email,
    Phone,
    StreetAddress,
    City,
    Company,
    Integer,
    Decimal,
    Date,
    Boolean,
    OneOf,
    SequentialId,
    Uuid,
    Lorem,
    Pattern,
}

impl KindChoice {
    pub const ALL: [KindChoice; 17] = [
        KindChoice::FirstName,
        KindChoice::LastName,
        KindChoice::FullName,
        KindChoice::Email,
        KindChoice::Phone,
        KindChoice::StreetAddress,
        KindChoice::City,
        KindChoice::Company,
        KindChoice::Integer,
        KindChoice::Decimal,
        KindChoice::Date,
        KindChoice::Boolean,
        KindChoice::OneOf,
        KindChoice::SequentialId,
        KindChoice::Uuid,
        KindChoice::Lorem,
        KindChoice::Pattern,
    ];

    pub fn label(self) -> &'static str {
        match self {
            KindChoice::FirstName => "First name",
            KindChoice::LastName => "Last name",
            KindChoice::FullName => "Full name",
            KindChoice::Email => "Email",
            KindChoice::Phone => "Phone",
            KindChoice::StreetAddress => "Street address",
            KindChoice::City => "City",
            KindChoice::Company => "Company",
            KindChoice::Integer => "Whole number",
            KindChoice::Decimal => "Decimal number",
            KindChoice::Date => "Date",
            KindChoice::Boolean => "TRUE or FALSE",
            KindChoice::OneOf => "List of values",
            KindChoice::SequentialId => "Sequential id",
            KindChoice::Uuid => "UUID",
            KindChoice::Lorem => "Text",
            KindChoice::Pattern => "Pattern",
        }
    }

    pub fn of(kind: &ColumnKind) -> KindChoice {
        match kind {
            ColumnKind::FirstName { .. } => KindChoice::FirstName,
            ColumnKind::LastName { .. } => KindChoice::LastName,
            ColumnKind::FullName { .. } => KindChoice::FullName,
            ColumnKind::Email { .. } => KindChoice::Email,
            ColumnKind::Phone { .. } => KindChoice::Phone,
            ColumnKind::StreetAddress {} => KindChoice::StreetAddress,
            ColumnKind::City {} => KindChoice::City,
            ColumnKind::Company {} => KindChoice::Company,
            ColumnKind::Integer { .. } => KindChoice::Integer,
            ColumnKind::Decimal { .. } => KindChoice::Decimal,
            ColumnKind::Date { .. } => KindChoice::Date,
            ColumnKind::Boolean {} => KindChoice::Boolean,
            ColumnKind::OneOf { .. } => KindChoice::OneOf,
            ColumnKind::SequentialId { .. } => KindChoice::SequentialId,
            ColumnKind::Uuid {} => KindChoice::Uuid,
            ColumnKind::Lorem { .. } => KindChoice::Lorem,
            ColumnKind::Pattern { .. } => KindChoice::Pattern,
        }
    }

    // Email and phone take the locale's own domains and phone pattern from the detector;
    // an empty fallback is reported by the generator as a missing option, never hidden.
    pub fn default_kind(self, locale: Locale, today: Date) -> ColumnKind {
        match self {
            KindChoice::FirstName => ColumnKind::FirstName {
                gender: Gender::Any,
            },
            KindChoice::LastName => ColumnKind::LastName {
                count: match locale {
                    Locale::SpanishArgentina => LastNameCount::OneOrTwo,
                    Locale::EnglishUnitedStates => LastNameCount::One,
                },
            },
            KindChoice::FullName => ColumnKind::FullName {
                gender: Gender::Any,
                last_names: LastNameCount::One,
            },
            KindChoice::Email => detect_kind("email", locale).unwrap_or(ColumnKind::Email {
                format: EmailFormat::FirstDotLast,
                first_name_from: None,
                last_name_from: None,
                domains: Vec::new(),
            }),
            KindChoice::Phone => detect_kind("phone", locale).unwrap_or(ColumnKind::Phone {
                pattern: String::new(),
            }),
            KindChoice::StreetAddress => ColumnKind::StreetAddress {},
            KindChoice::City => ColumnKind::City {},
            KindChoice::Company => ColumnKind::Company {},
            KindChoice::Integer => ColumnKind::Integer { min: 1, max: 1000 },
            KindChoice::Decimal => ColumnKind::Decimal {
                min: 10.0,
                max: 2000.0,
                places: 2,
            },
            KindChoice::Date => with_end_today(
                detect_kind("date", locale).unwrap_or(ColumnKind::Date {
                    from: today,
                    to: today,
                }),
                today,
            ),
            KindChoice::Boolean => ColumnKind::Boolean {},
            KindChoice::OneOf => ColumnKind::OneOf {
                options: ["Option 1", "Option 2", "Option 3"]
                    .map(|value| ListOption {
                        value: value.to_string(),
                        weight: 1,
                    })
                    .to_vec(),
            },
            KindChoice::SequentialId => ColumnKind::SequentialId { start: 1, step: 1 },
            KindChoice::Uuid => ColumnKind::Uuid {},
            KindChoice::Lorem => ColumnKind::Lorem {
                min_words: 3,
                max_words: 8,
            },
            KindChoice::Pattern => ColumnKind::Pattern {
                pattern: "AAA-####".to_string(),
            },
        }
    }
}

// The generator keeps its date range reproducible; the dialog ends it today.
pub fn with_end_today(kind: ColumnKind, today: Date) -> ColumnKind {
    match kind {
        ColumnKind::Date { from, .. } => ColumnKind::Date { from, to: today },
        other => other,
    }
}

pub fn detect(header: &str, locale: Locale, today: Date) -> Option<ColumnKind> {
    detect_kind(header, locale).map(|kind| with_end_today(kind, today))
}

mod choices;
mod fields;
#[cfg(test)]
mod tests;

pub use choices::{choice_groups, summary};
pub use fields::{OptionError, fields, with_fields};
