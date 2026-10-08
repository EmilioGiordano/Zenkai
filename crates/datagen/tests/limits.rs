use zenkai_datagen::{
    ColumnKind, ColumnProblem, ColumnSpec, DatagenError, EmailFormat, GenerationSpec, ListOption,
    Locale, Percent, TextField, validate,
};

fn column(header: &str, kind: ColumnKind) -> ColumnSpec {
    ColumnSpec {
        header: header.to_string(),
        kind,
        blanks: Percent::ZERO,
        unique: false,
    }
}

fn check(header: &str, kind: ColumnKind) -> Result<(), DatagenError> {
    validate(&GenerationSpec {
        rows: 10,
        locale: Locale::EnglishUnitedStates,
        seed: 1,
        columns: vec![column(header, kind)],
    })
}

fn problem(header: &str, kind: ColumnKind) -> Option<ColumnProblem> {
    match check(header, kind) {
        Err(DatagenError::Column { problem, .. }) => Some(problem),
        _ => None,
    }
}

fn list(count: usize, value_chars: usize) -> ColumnKind {
    ColumnKind::OneOf {
        options: (0..count)
            .map(|index| ListOption {
                value: format!("{index:0>value_chars$}"),
                weight: 1,
            })
            .collect(),
    }
}

fn list_of(value: &str) -> ColumnKind {
    ColumnKind::OneOf {
        options: vec![ListOption {
            value: value.to_string(),
            weight: 1,
        }],
    }
}

fn pattern(text: String) -> ColumnKind {
    ColumnKind::Pattern { pattern: text }
}

fn emails_to(domains: Vec<String>) -> ColumnKind {
    ColumnKind::Email {
        format: EmailFormat::FirstDotLast,
        first_name_from: None,
        last_name_from: None,
        domains,
    }
}

fn numbered_domains(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("d{index}.com")).collect()
}

fn too_long(field: TextField, chars: usize, limit: usize) -> Option<ColumnProblem> {
    Some(ColumnProblem::TextTooLong {
        field,
        chars,
        limit,
    })
}

#[test]
fn sizes_pass_at_the_limit() {
    assert_eq!(check(&"h".repeat(255), ColumnKind::City {}), Ok(()));
    assert_eq!(check("p", pattern("#".repeat(256))), Ok(()));
    assert_eq!(check("o", list(10_000, 5)), Ok(()));
    assert_eq!(check("o", list(2, 255)), Ok(()));
    assert_eq!(check("e", emails_to(numbered_domains(100))), Ok(()));
    let longest_domain = format!("{}.com", "a".repeat(249));
    assert_eq!(check("e", emails_to(vec![longest_domain])), Ok(()));
}

#[test]
fn sizes_fail_one_above_the_limit() {
    assert_eq!(
        problem(&"h".repeat(256), ColumnKind::City {}),
        too_long(TextField::Header, 256, 255)
    );
    assert_eq!(
        problem("p", pattern("#".repeat(257))),
        too_long(TextField::Pattern, 257, 256)
    );
    assert_eq!(
        problem("o", list(10_001, 5)),
        Some(ColumnProblem::TooManyOptions {
            count: 10_001,
            limit: 10_000
        })
    );
    assert_eq!(
        problem("o", list(2, 256)),
        too_long(TextField::ListValue, 256, 255)
    );
    assert_eq!(
        problem("e", emails_to(numbered_domains(101))),
        Some(ColumnProblem::TooManyDomains {
            count: 101,
            limit: 100
        })
    );
    let domain = format!("{}.com", "a".repeat(250));
    assert_eq!(
        problem("e", emails_to(vec![domain])),
        too_long(TextField::Domain, 254, 253)
    );
}

#[test]
fn control_characters_are_rejected_except_tab_and_line_break() {
    let control = |field| Some(ColumnProblem::ControlCharacter { field });
    assert_eq!(
        problem("Nom\u{1b}bre", ColumnKind::City {}),
        control(TextField::Header)
    );
    assert_eq!(
        problem("SKU", pattern("AA\r##".to_string())),
        control(TextField::Pattern)
    );
    assert_eq!(
        problem("L", list_of("a\u{0}b")),
        control(TextField::ListValue)
    );
    assert_eq!(check("L", list_of("a\tb\nc")), Ok(()));
}

fn table(rows: u32, columns: usize, kind: ColumnKind) -> Result<(), DatagenError> {
    validate(&GenerationSpec {
        rows,
        locale: Locale::SpanishArgentina,
        seed: 1,
        columns: (0..columns)
            .map(|position| column(&format!("C{position}"), kind.clone()))
            .collect(),
    })
}

#[test]
fn cell_budget_passes_at_the_limit_and_fails_above_it() {
    assert_eq!(table(1_000_000, 5, ColumnKind::City {}), Ok(()));
    assert_eq!(
        table(1_000_001, 5, ColumnKind::City {}),
        Err(DatagenError::TooManyCells {
            requested: 5_000_005,
            limit: 5_000_000
        })
    );
}

#[test]
fn a_small_spec_cannot_ask_for_gigabytes() {
    let lorem = ColumnKind::Lorem {
        min_words: 200,
        max_words: 200,
    };
    assert!(matches!(
        table(1_000_000, 4, lorem),
        Err(DatagenError::OutputTooLarge { .. })
    ));
    let long_pattern = ColumnKind::Pattern {
        pattern: "A".repeat(256),
    };
    assert!(matches!(
        table(1_000_000, 4, long_pattern),
        Err(DatagenError::OutputTooLarge { .. })
    ));
}
