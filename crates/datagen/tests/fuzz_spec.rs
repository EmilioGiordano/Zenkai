// Structured fuzzing of the generation spec an agent or the dialog hands over as JSON:
// damaged specs are refused or generate a bounded table, and never panic or hang.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../test-support/json_mutation.rs"]
mod json_mutation;

use std::collections::HashSet;

use proptest::prelude::*;
use zenkai_datagen::{
    ColumnKind, ColumnSpec, Date, EmailFormat, Gender, GenerationSpec, LastNameCount, ListOption,
    Locale, Percent, generate, validate,
};

const CELL_CHARS: usize = 32_767;
const FUZZ_ROW_LIMIT: u32 = 200;

fn column(header: &str, kind: ColumnKind, unique: bool) -> ColumnSpec {
    ColumnSpec {
        header: header.to_string(),
        kind,
        blanks: Percent::new(10).unwrap(),
        unique,
    }
}

fn baseline() -> serde_json::Value {
    let columns = vec![
        column(
            "Name",
            ColumnKind::FullName {
                gender: Gender::Any,
                last_names: LastNameCount::OneOrTwo,
            },
            false,
        ),
        column(
            "Mail",
            ColumnKind::Email {
                format: EmailFormat::FirstDotLast,
                first_name_from: Some("Name".to_string()),
                last_name_from: None,
                domains: vec!["example.com".to_string()],
            },
            true,
        ),
        column(
            "Phone",
            ColumnKind::Phone {
                pattern: "+54 9 11 ####-####".to_string(),
            },
            false,
        ),
        column("Age", ColumnKind::Integer { min: 18, max: 90 }, false),
        column(
            "Price",
            ColumnKind::Decimal {
                min: 0.5,
                max: 99.5,
                places: 2,
            },
            false,
        ),
        column(
            "Born",
            ColumnKind::Date {
                from: Date::parse("1990-01-01").unwrap(),
                to: Date::parse("2000-12-31").unwrap(),
            },
            false,
        ),
        column(
            "Tier",
            ColumnKind::OneOf {
                options: vec![
                    ListOption {
                        value: "gold".to_string(),
                        weight: 2,
                    },
                    ListOption {
                        value: "silver".to_string(),
                        weight: 1,
                    },
                ],
            },
            false,
        ),
        column("Id", ColumnKind::SequentialId { start: 1, step: 1 }, false),
        column(
            "Notes",
            ColumnKind::Lorem {
                min_words: 3,
                max_words: 12,
            },
            false,
        ),
        column(
            "Code",
            ColumnKind::Pattern {
                pattern: "AAA-####".to_string(),
            },
            true,
        ),
        column("Key", ColumnKind::Uuid {}, false),
        column("Active", ColumnKind::Boolean {}, false),
        column("City", ColumnKind::City {}, false),
        column("Street", ColumnKind::StreetAddress {}, false),
        column("Firm", ColumnKind::Company {}, false),
    ];
    serde_json::to_value(GenerationSpec {
        rows: 50,
        locale: Locale::SpanishArgentina,
        seed: 7,
        columns,
    })
    .unwrap()
}

fn check_spec(spec: &GenerationSpec) {
    if validate(spec).is_err() || spec.rows > FUZZ_ROW_LIMIT {
        return;
    }
    let table = generate(spec).expect("a spec that validates must generate");
    assert_eq!(table.len(), spec.rows as usize);
    for row in &table {
        assert_eq!(row.len(), spec.columns.len());
        assert!(row.iter().all(|cell| cell.chars().count() <= CELL_CHARS));
    }
    assert_eq!(generate(spec).unwrap(), table, "the same spec, other rows");
    for (position, column) in spec.columns.iter().enumerate().filter(|(_, c)| c.unique) {
        let filled: Vec<&String> = table
            .iter()
            .map(|row| &row[position])
            .filter(|cell| !cell.is_empty())
            .collect();
        let distinct: HashSet<&&String> = filled.iter().collect();
        assert_eq!(distinct.len(), filled.len(), "{} repeats", column.header);
    }
}

#[test]
fn the_baseline_spec_is_valid_and_deterministic() {
    let spec: GenerationSpec = serde_json::from_value(baseline()).unwrap();
    validate(&spec).unwrap();
    check_spec(&spec);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, ..ProptestConfig::default() })]

    #[test]
    fn damaged_specs_are_refused_or_generate_a_bounded_table(bytes in json_mutation::mutated(baseline())) {
        if let Ok(spec) = serde_json::from_slice::<GenerationSpec>(&bytes) {
            check_spec(&spec);
        }
    }

    #[test]
    fn arbitrary_bytes_never_panic_the_spec_parser(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        if let Ok(spec) = serde_json::from_slice::<GenerationSpec>(&bytes) {
            check_spec(&spec);
        }
    }
}
