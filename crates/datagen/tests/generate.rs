// Test helpers outside #[test] functions; clippy only exempts the functions themselves.
#![allow(clippy::unwrap_used)]

use std::collections::HashSet;
use std::time::{Duration, Instant};

use zenkai_datagen::{
    ColumnKind, ColumnProblem, ColumnSpec, DatagenError, Date, EmailFormat, Gender, GenerationSpec,
    LastNameCount, ListOption, Locale, Percent, generate, validate,
};

fn column(header: &str, kind: ColumnKind) -> ColumnSpec {
    ColumnSpec {
        header: header.to_string(),
        kind,
        blanks: Percent::ZERO,
        unique: false,
    }
}

fn spec(rows: u32, columns: Vec<ColumnSpec>) -> GenerationSpec {
    GenerationSpec {
        rows,
        locale: Locale::SpanishArgentina,
        seed: 48_213,
        columns,
    }
}

fn date(text: &str) -> Date {
    Date::parse(text).unwrap()
}

fn people_columns() -> Vec<ColumnSpec> {
    vec![
        column(
            "Nombre",
            ColumnKind::FirstName {
                gender: Gender::Any,
            },
        ),
        column(
            "Apellido",
            ColumnKind::LastName {
                count: LastNameCount::OneOrTwo,
            },
        ),
        column(
            "Email",
            ColumnKind::Email {
                format: EmailFormat::FirstDotLast,
                first_name_from: Some("Nombre".to_string()),
                last_name_from: Some("Apellido".to_string()),
                domains: vec!["gmail.com".to_string(), "outlook.com".to_string()],
            },
        ),
        column(
            "Teléfono",
            ColumnKind::Phone {
                pattern: "+54 9 11 ####-####".to_string(),
            },
        ),
        column(
            "Alta",
            ColumnKind::Date {
                from: date("2020-01-01"),
                to: date("2024-12-31"),
            },
        ),
    ]
}

fn every_kind() -> Vec<ColumnSpec> {
    let mut columns = people_columns();
    columns.extend([
        column(
            "Nombre completo",
            ColumnKind::FullName {
                gender: Gender::Female,
                last_names: LastNameCount::Two,
            },
        ),
        column("Dirección", ColumnKind::StreetAddress {}),
        column("Ciudad", ColumnKind::City {}),
        column("Empresa", ColumnKind::Company {}),
        column("Edad", ColumnKind::Integer { min: -5, max: 90 }),
        column(
            "Precio",
            ColumnKind::Decimal {
                min: 20.0,
                max: 1_800.0,
                places: 2,
            },
        ),
        column("Activo", ColumnKind::Boolean {}),
        column(
            "Producto",
            ColumnKind::OneOf {
                options: vec![
                    ListOption {
                        value: "Teclado".to_string(),
                        weight: 3,
                    },
                    ListOption {
                        value: "Mouse".to_string(),
                        weight: 1,
                    },
                ],
            },
        ),
        column(
            "Id",
            ColumnKind::SequentialId {
                start: 100,
                step: 5,
            },
        ),
        column("Clave", ColumnKind::Uuid {}),
        column(
            "Notas",
            ColumnKind::Lorem {
                min_words: 3,
                max_words: 6,
            },
        ),
        column(
            "SKU",
            ColumnKind::Pattern {
                pattern: "AAA-####".to_string(),
            },
        ),
    ]);
    columns
}

fn column_values(rows: &[Vec<String>], position: usize) -> Vec<&str> {
    rows.iter().map(|row| row[position].as_str()).collect()
}

#[test]
fn same_spec_gives_the_same_rows_and_another_seed_does_not() {
    let first = generate(&spec(500, every_kind())).unwrap();
    assert_eq!(first, generate(&spec(500, every_kind())).unwrap());
    let mut reseeded = spec(500, every_kind());
    reseeded.seed += 1;
    assert_ne!(first, generate(&reseeded).unwrap());
}

#[test]
fn output_is_pinned_across_platforms_and_versions() {
    let rows = generate(&spec(2, people_columns())).unwrap();
    assert_eq!(rows, PINNED_ROWS);
}

const PINNED_ROWS: [[&str; 5]; 2] = [
    [
        "Rodrigo",
        "Ibáñez Ferrari",
        "rodrigo.ibanezferrari@outlook.com",
        "'+54 9 11 5851-0583",
        "2020-07-08",
    ],
    [
        "Luis",
        "Rossi Romero",
        "luis.rossiromero@gmail.com",
        "'+54 9 11 1331-5751",
        "2021-06-28",
    ],
];

#[test]
fn adding_a_column_leaves_the_others_unchanged() {
    let before = generate(&spec(50, people_columns())).unwrap();
    let mut columns = people_columns();
    columns.push(column("Ciudad", ColumnKind::City {}));
    let after = generate(&spec(50, columns)).unwrap();
    for (old, new) in before.iter().zip(&after) {
        assert_eq!(old[..], new[..5]);
    }
}

#[test]
fn rows_have_one_cell_per_column() {
    let rows = generate(&spec(30, every_kind())).unwrap();
    assert_eq!(rows.len(), 30);
    assert!(rows.iter().all(|row| row.len() == every_kind().len()));
    assert_eq!(generate(&spec(0, every_kind())).unwrap().len(), 0);
}

#[test]
fn blanks_hit_the_requested_share_exactly() {
    let mut columns = every_kind();
    for (position, column) in columns.iter_mut().enumerate() {
        column.blanks = Percent::new((position * 6) as u8).unwrap();
    }
    let rows = generate(&spec(1_000, columns.clone())).unwrap();
    for (position, column) in columns.iter().enumerate() {
        let blanks = column_values(&rows, position)
            .iter()
            .filter(|cell| cell.is_empty())
            .count();
        assert_eq!(
            blanks,
            usize::from(column.blanks.get()) * 10,
            "{}",
            column.header
        );
    }
}

fn unique(mut column: ColumnSpec) -> ColumnSpec {
    column.unique = true;
    column
}

fn distinct_values(rows: &[Vec<String>], position: usize) -> usize {
    column_values(rows, position)
        .into_iter()
        .filter(|cell| !cell.is_empty())
        .collect::<HashSet<_>>()
        .len()
}

#[test]
fn unique_columns_never_repeat_even_when_the_domain_is_exhausted() {
    let es_ar_cities = 50;
    let rows = generate(&spec(
        es_ar_cities,
        vec![unique(column("Ciudad", ColumnKind::City {}))],
    ))
    .unwrap();
    assert_eq!(distinct_values(&rows, 0), es_ar_cities as usize);

    let codes = unique(column(
        "SKU",
        ColumnKind::Pattern {
            pattern: "A#".to_string(),
        },
    ));
    let rows = generate(&spec(260, vec![codes])).unwrap();
    assert_eq!(distinct_values(&rows, 0), 260);

    let mut booleans = unique(column("Activo", ColumnKind::Boolean {}));
    booleans.blanks = Percent::new(50).unwrap();
    let rows = generate(&spec(4, vec![booleans])).unwrap();
    assert_eq!(distinct_values(&rows, 0), 2);

    let mut columns = people_columns();
    columns[2].unique = true;
    let rows = generate(&spec(20_000, columns)).unwrap();
    assert_eq!(distinct_values(&rows, 2), 20_000);
}

#[test]
fn too_small_domains_fail_before_generating() {
    let mut bools = column("Activo", ColumnKind::Boolean {});
    bools.unique = true;
    assert_eq!(
        generate(&spec(3, vec![bools.clone()])),
        Err(DatagenError::Column {
            position: 0,
            header: "Activo".to_string(),
            problem: ColumnProblem::DomainTooSmall {
                needed: 3,
                available: 2
            },
        })
    );
    bools.blanks = Percent::new(40).unwrap();
    assert!(generate(&spec(3, vec![bools.clone()])).is_ok());

    let mut codes = column(
        "SKU",
        ColumnKind::Pattern {
            pattern: "A#".to_string(),
        },
    );
    codes.unique = true;
    assert!(matches!(
        generate(&spec(261, vec![codes])),
        Err(DatagenError::Column {
            problem: ColumnProblem::DomainTooSmall {
                needed: 261,
                available: 260
            },
            ..
        })
    ));
}

fn assert_all(rows: &[Vec<String>], position: usize, check: impl Fn(&str) -> bool) {
    for cell in column_values(rows, position) {
        assert!(check(cell), "unexpected cell {cell:?} in column {position}");
    }
}

fn digits_where(cell: &str, template: &str) -> bool {
    cell.len() == template.len()
        && cell.chars().zip(template.chars()).all(|(c, t)| match t {
            '#' => c.is_ascii_digit(),
            'A' => c.is_ascii_uppercase(),
            _ => c == t,
        })
}

#[test]
fn every_kind_has_its_format() {
    let rows = generate(&spec(400, every_kind())).unwrap();
    let words = |cell: &str| {
        cell.split(' ')
            .all(|word| word.chars().all(char::is_alphabetic))
    };
    assert_all(&rows, 0, |cell| !cell.contains(' ') && words(cell));
    assert_all(&rows, 1, |cell| cell.split(' ').count() <= 2 && words(cell));
    assert!(
        column_values(&rows, 1)
            .iter()
            .any(|cell| cell.contains(' '))
    );
    assert_all(&rows, 2, |cell| cell.contains('@') && !cell.contains(' '));
    assert_all(&rows, 3, |cell| digits_where(cell, "'+54 9 11 ####-####"));
    assert_all(&rows, 4, |cell| {
        Date::parse(cell).is_ok_and(|day| day >= date("2020-01-01") && day <= date("2024-12-31"))
    });
    assert_all(&rows, 5, |cell| cell.split(' ').count() == 3 && words(cell));
    assert_all(&rows, 6, |cell| {
        cell.rsplit_once(' ').is_some_and(|(_, number)| {
            number
                .parse::<u32>()
                .is_ok_and(|n| (1..=9_999).contains(&n))
        })
    });
    assert_all(&rows, 7, |cell| !cell.is_empty());
    assert_all(&rows, 8, |cell| cell.contains(' '));
    assert_all(&rows, 9, |cell| {
        cell.parse::<i64>().is_ok_and(|n| (-5..=90).contains(&n))
    });
    assert_all(&rows, 10, |cell| {
        cell.split_once('.')
            .is_some_and(|(_, cents)| cents.len() == 2)
            && cell
                .parse::<f64>()
                .is_ok_and(|n| (20.0..=1_800.0).contains(&n))
    });
    assert_all(&rows, 11, |cell| cell == "TRUE" || cell == "FALSE");
    let teclados = column_values(&rows, 12)
        .iter()
        .filter(|cell| **cell == "Teclado")
        .count();
    assert!(
        (250..=350).contains(&teclados),
        "{teclados} of 400 with weight 3 to 1"
    );
    assert_all(&rows, 12, |cell| cell == "Teclado" || cell == "Mouse");
    let ids: Vec<String> = (0..400).map(|row| (100 + 5 * row).to_string()).collect();
    assert_eq!(column_values(&rows, 13), ids);
    assert_all(&rows, 14, |cell| {
        cell.strip_prefix('\'').is_some_and(|uuid| {
            uuid.len() == 36
                && uuid.as_bytes()[14] == b'4'
                && b"89ab".contains(&uuid.as_bytes()[19])
        })
    });
    assert_all(&rows, 15, |cell| {
        let words = cell.split(' ').count();
        (3..=6).contains(&words) && cell.ends_with('.') && cell.starts_with(char::is_uppercase)
    });
    assert_all(&rows, 16, |cell| digits_where(cell, "'AAA-####"));
}

#[test]
fn email_comes_from_the_names_in_the_same_row() {
    let mut columns = people_columns();
    columns[0].blanks = Percent::new(30).unwrap();
    let rows = generate(&spec(300, columns)).unwrap();
    for row in &rows {
        let local = row[2].split('@').next().unwrap();
        if row[0].is_empty() {
            continue;
        }
        let expected = format!("{}.{}", plain(&row[0]), plain(&row[1]));
        assert_eq!(local, expected);
    }
}

fn plain(name: &str) -> String {
    name.to_lowercase()
        .replace('á', "a")
        .replace('é', "e")
        .replace('í', "i")
        .replace('ó', "o")
        .replace('ú', "u")
        .replace('ñ', "n")
        .replace(' ', "")
}

#[test]
fn email_formats_and_full_name_sources() {
    let full_name = column(
        "Cliente",
        ColumnKind::FullName {
            gender: Gender::Male,
            last_names: LastNameCount::Two,
        },
    );
    let email = |format| {
        column(
            "Mail",
            ColumnKind::Email {
                format,
                first_name_from: Some("Cliente".to_string()),
                last_name_from: Some("Cliente".to_string()),
                domains: vec!["example.com".to_string()],
            },
        )
    };
    let rows = generate(&spec(
        20,
        vec![
            full_name,
            email(EmailFormat::FirstDotLast),
            email(EmailFormat::FirstLast),
            email(EmailFormat::InitialLast),
        ]
        .into_iter()
        .enumerate()
        .map(|(position, mut column)| {
            if position > 0 {
                column.header = format!("Mail {position}");
            }
            column
        })
        .collect(),
    ))
    .unwrap();
    for row in &rows {
        let (first, last) = row[0].split_once(' ').unwrap();
        let (first, last) = (plain(first), plain(last));
        assert_eq!(row[1], format!("{first}.{last}@example.com"));
        assert_eq!(row[2], format!("{first}{last}@example.com"));
        assert_eq!(row[3], format!("{}{last}@example.com", &first[..1]));
    }
}

fn problem_of(columns: Vec<ColumnSpec>) -> ColumnProblem {
    match validate(&spec(10, columns)) {
        Err(DatagenError::Column { problem, .. }) => problem,
        other => panic!("expected a column problem, got {other:?}"),
    }
}

#[test]
fn invalid_specs_name_the_column_and_the_problem() {
    let error = validate(&spec(
        10,
        vec![column("Edad", ColumnKind::Integer { min: 9, max: 1 })],
    ))
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "column 1 \"Edad\": the minimum 9 is greater than the maximum 1"
    );
    let email_from = |first: &str| {
        column(
            "Mail",
            ColumnKind::Email {
                format: EmailFormat::FirstDotLast,
                first_name_from: Some(first.to_string()),
                last_name_from: None,
                domains: vec!["gmail.com".to_string()],
            },
        )
    };
    assert_eq!(
        problem_of(vec![email_from("Nombre")]),
        ColumnProblem::UnknownSource {
            source_header: "Nombre".to_string()
        }
    );
    assert_eq!(
        problem_of(vec![
            column("Nombre", ColumnKind::City {}),
            email_from("Nombre")
        ]),
        ColumnProblem::NotFirstNameSource {
            source_header: "Nombre".to_string()
        }
    );
    let name = column(
        "Nombre",
        ColumnKind::FirstName {
            gender: Gender::Any,
        },
    );
    assert_eq!(
        problem_of(vec![name.clone(), name, email_from("Nombre")]),
        ColumnProblem::AmbiguousSource {
            source_header: "Nombre".to_string()
        }
    );
    let cases = [
        (
            ColumnKind::Email {
                format: EmailFormat::FirstLast,
                first_name_from: None,
                last_name_from: None,
                domains: vec![],
            },
            ColumnProblem::NoDomains,
        ),
        (
            ColumnKind::Email {
                format: EmailFormat::FirstLast,
                first_name_from: None,
                last_name_from: None,
                domains: vec!["Gmail .com".to_string()],
            },
            ColumnProblem::InvalidDomain {
                domain: "Gmail .com".to_string(),
            },
        ),
        (
            ColumnKind::Decimal {
                min: 1.0,
                max: f64::NAN,
                places: 2,
            },
            ColumnProblem::NotFinite {
                value: "NaN".to_string(),
            },
        ),
        (
            ColumnKind::Decimal {
                min: 0.0,
                max: 1e14,
                places: 2,
            },
            ColumnProblem::BeyondPrecision {
                value: "100000000000000".to_string(),
                limit: 999_999_999_999_999,
            },
        ),
        (
            ColumnKind::Decimal {
                min: 0.0,
                max: 1.0,
                places: 12,
            },
            ColumnProblem::TooManyPlaces {
                places: 12,
                limit: 9,
            },
        ),
        (
            ColumnKind::Date {
                from: date("2024-01-02"),
                to: date("2024-01-01"),
            },
            ColumnProblem::MinAboveMax {
                min: "2024-01-02".to_string(),
                max: "2024-01-01".to_string(),
            },
        ),
        (
            ColumnKind::SequentialId { start: 1, step: 0 },
            ColumnProblem::ZeroStep,
        ),
        (
            ColumnKind::Pattern {
                pattern: "AB-\\".to_string(),
            },
            ColumnProblem::DanglingEscape,
        ),
        (
            ColumnKind::Phone {
                pattern: "+54 11".to_string(),
            },
            ColumnProblem::NoPlaceholders,
        ),
        (
            ColumnKind::OneOf { options: vec![] },
            ColumnProblem::EmptyList,
        ),
        (
            ColumnKind::OneOf {
                options: vec![
                    ListOption {
                        value: "a".to_string(),
                        weight: 1,
                    },
                    ListOption {
                        value: "a".to_string(),
                        weight: 1,
                    },
                ],
            },
            ColumnProblem::DuplicateOption {
                value: "a".to_string(),
            },
        ),
        (
            ColumnKind::OneOf {
                options: vec![ListOption {
                    value: "a".to_string(),
                    weight: 0,
                }],
            },
            ColumnProblem::AllWeightsZero,
        ),
        (
            ColumnKind::Lorem {
                min_words: 0,
                max_words: 3,
            },
            ColumnProblem::WordCountOutOfRange { limit: 200 },
        ),
    ];
    for (kind, expected) in cases {
        assert_eq!(problem_of(vec![column("Columna", kind)]), expected);
    }
    let mut weighted = column(
        "Producto",
        ColumnKind::OneOf {
            options: vec![ListOption {
                value: "a".to_string(),
                weight: 2,
            }],
        },
    );
    weighted.unique = true;
    assert_eq!(problem_of(vec![weighted]), ColumnProblem::WeightsWithUnique);
}

#[test]
fn spec_level_limits() {
    assert_eq!(validate(&spec(10, vec![])), Err(DatagenError::NoColumns));
    assert_eq!(
        validate(&spec(
            1_048_576,
            vec![column("Ciudad", ColumnKind::City {})]
        )),
        Err(DatagenError::TooManyRows {
            requested: 1_048_576,
            limit: 1_048_575
        })
    );
    assert_eq!(
        validate(&spec(
            1_048_575,
            vec![column("Ciudad", ColumnKind::City {})]
        )),
        Ok(())
    );
    let sequence = column(
        "Id",
        ColumnKind::SequentialId {
            start: 999_999_999_999_000,
            step: 1_000,
        },
    );
    assert!(matches!(
        problem_of(vec![sequence]),
        ColumnProblem::BeyondPrecision { .. }
    ));
}

#[test]
#[ignore = "timing; run in release: cargo test --release -p zenkai-datagen -- --ignored"]
fn hundred_thousand_rows_by_ten_columns_well_under_a_second() {
    let columns: Vec<ColumnSpec> = every_kind().into_iter().take(10).collect();
    let started = Instant::now();
    let rows = generate(&spec(100_000, columns)).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(rows.len(), 100_000);
    assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");
}
