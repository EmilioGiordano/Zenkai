use crate::date::Date;
use crate::locale;
use crate::spec::{ColumnKind, EmailFormat, Gender, LastNameCount, Locale};
use crate::text::fold;

// The dialog replaces the end with today; the spec itself must stay reproducible.
const DEFAULT_FROM: Date = Date::known(2020, 1, 1);
const DEFAULT_TO: Date = Date::known(2026, 12, 31);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Detected {
    FirstName,
    LastName,
    FullName,
    Email,
    Phone,
    Date,
    Price,
    Id,
    Code,
    City,
    Address,
    Company,
}

// Headers are compared after lowercasing, dropping accents and turning _ and - into spaces.
const SYNONYMS: &[(&str, Detected)] = &[
    ("nombre", Detected::FirstName),
    ("nombres", Detected::FirstName),
    ("name", Detected::FirstName),
    ("first name", Detected::FirstName),
    ("firstname", Detected::FirstName),
    ("apellido", Detected::LastName),
    ("apellidos", Detected::LastName),
    ("last name", Detected::LastName),
    ("lastname", Detected::LastName),
    ("surname", Detected::LastName),
    ("nombre completo", Detected::FullName),
    ("nombre y apellido", Detected::FullName),
    ("full name", Detected::FullName),
    ("email", Detected::Email),
    ("e mail", Detected::Email),
    ("mail", Detected::Email),
    ("correo", Detected::Email),
    ("correo electronico", Detected::Email),
    ("telefono", Detected::Phone),
    ("tel", Detected::Phone),
    ("celular", Detected::Phone),
    ("movil", Detected::Phone),
    ("phone", Detected::Phone),
    ("phone number", Detected::Phone),
    ("mobile", Detected::Phone),
    ("fecha", Detected::Date),
    ("alta", Detected::Date),
    ("fecha de alta", Detected::Date),
    ("date", Detected::Date),
    ("precio", Detected::Price),
    ("importe", Detected::Price),
    ("monto", Detected::Price),
    ("price", Detected::Price),
    ("amount", Detected::Price),
    ("id", Detected::Id),
    ("codigo", Detected::Code),
    ("code", Detected::Code),
    ("sku", Detected::Code),
    ("ciudad", Detected::City),
    ("localidad", Detected::City),
    ("city", Detected::City),
    ("direccion", Detected::Address),
    ("domicilio", Detected::Address),
    ("address", Detected::Address),
    ("street address", Detected::Address),
    ("empresa", Detected::Company),
    ("compania", Detected::Company),
    ("razon social", Detected::Company),
    ("company", Detected::Company),
];

pub fn detect_kind(header: &str, locale: Locale) -> Option<ColumnKind> {
    let normalized = normalize(header);
    let detected = SYNONYMS
        .iter()
        .find(|(synonym, _)| *synonym == normalized)
        .map(|(_, detected)| *detected)?;
    Some(default_kind(detected, locale))
}

// Like detect_kind, and links each email column to the name columns detected beside it.
pub fn detect_kinds(headers: &[&str], locale: Locale) -> Vec<Option<ColumnKind>> {
    let mut kinds: Vec<Option<ColumnKind>> = headers
        .iter()
        .map(|header| detect_kind(header, locale))
        .collect();
    let header_of = |wanted: fn(&ColumnKind) -> bool| {
        let mut found = kinds
            .iter()
            .zip(headers)
            .filter(|(kind, _)| kind.as_ref().is_some_and(wanted))
            .map(|(_, header)| header.to_string());
        let first = found.next();
        let unique_header = first
            .as_ref()
            .is_some_and(|first| headers.iter().filter(|header| *header == first).count() == 1);
        first.filter(|_| unique_header)
    };
    let full_name = header_of(|kind| matches!(kind, ColumnKind::FullName { .. }));
    let first =
        header_of(|kind| matches!(kind, ColumnKind::FirstName { .. })).or(full_name.clone());
    let last = header_of(|kind| matches!(kind, ColumnKind::LastName { .. })).or(full_name);
    for kind in kinds.iter_mut().flatten() {
        if let ColumnKind::Email {
            first_name_from,
            last_name_from,
            ..
        } = kind
        {
            first_name_from.clone_from(&first);
            last_name_from.clone_from(&last);
        }
    }
    kinds
}

fn normalize(header: &str) -> String {
    fold(header)
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn default_kind(detected: Detected, locale: Locale) -> ColumnKind {
    let data = locale::data(locale);
    match detected {
        Detected::FirstName => ColumnKind::FirstName {
            gender: Gender::Any,
        },
        Detected::LastName => ColumnKind::LastName {
            count: match locale {
                Locale::SpanishArgentina => LastNameCount::OneOrTwo,
                Locale::EnglishUnitedStates => LastNameCount::One,
            },
        },
        Detected::FullName => ColumnKind::FullName {
            gender: Gender::Any,
            last_names: LastNameCount::One,
        },
        Detected::Email => ColumnKind::Email {
            format: EmailFormat::FirstDotLast,
            first_name_from: None,
            last_name_from: None,
            domains: data
                .email_domains
                .iter()
                .map(|domain| domain.to_string())
                .collect(),
        },
        Detected::Phone => ColumnKind::Phone {
            pattern: data.phone_pattern.to_string(),
        },
        Detected::Date => ColumnKind::Date {
            from: DEFAULT_FROM,
            to: DEFAULT_TO,
        },
        Detected::Price => ColumnKind::Decimal {
            min: 10.0,
            max: 2_000.0,
            places: 2,
        },
        Detected::Id => ColumnKind::SequentialId { start: 1, step: 1 },
        Detected::Code => ColumnKind::Pattern {
            pattern: "AAA-####".to_string(),
        },
        Detected::City => ColumnKind::City,
        Detected::Address => ColumnKind::StreetAddress,
        Detected::Company => ColumnKind::Company,
    }
}
