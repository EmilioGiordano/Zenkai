use super::*;

const TODAY_YEAR: i64 = 2026;

fn today() -> Date {
    Date::from_ymd(TODAY_YEAR, 10, 8).unwrap()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn every_kind_round_trips_through_its_choice() {
    for choice in KindChoice::ALL {
        let kind = choice.default_kind(Locale::SpanishArgentina, today());
        assert_eq!(KindChoice::of(&kind), choice);
    }
}

#[test]
fn email_and_phone_defaults_come_from_the_locale() {
    for locale in [Locale::SpanishArgentina, Locale::EnglishUnitedStates] {
        let ColumnKind::Email { domains, .. } = KindChoice::Email.default_kind(locale, today())
        else {
            panic!("not an email column");
        };
        assert!(!domains.is_empty());
        let ColumnKind::Phone { pattern } = KindChoice::Phone.default_kind(locale, today()) else {
            panic!("not a phone column");
        };
        assert!(!pattern.is_empty());
    }
}

#[test]
fn date_ranges_end_today() {
    let kind = KindChoice::Date.default_kind(Locale::SpanishArgentina, today());
    let ColumnKind::Date { to, .. } = kind else {
        panic!("not a date column");
    };
    assert_eq!(to, today());
    let detected = detect("Alta", Locale::SpanishArgentina, today()).unwrap();
    assert!(matches!(detected, ColumnKind::Date { to, .. } if to == today()));
}

#[test]
fn text_fields_build_the_kind() {
    let kind = ColumnKind::Decimal {
        min: 0.0,
        max: 1.0,
        places: 2,
    };
    let built = with_fields(&kind, &strings(&["20", "1800,5", "3"])).unwrap();
    assert_eq!(
        built,
        ColumnKind::Decimal {
            min: 20.0,
            max: 1800.5,
            places: 3
        }
    );
}

#[test]
fn a_bad_number_names_the_field() {
    let kind = ColumnKind::Integer { min: 1, max: 2 };
    let error = with_fields(&kind, &strings(&["1", "ten"])).unwrap_err();
    assert_eq!(
        error,
        OptionError::WholeNumber {
            field: "To",
            text: "ten".into()
        }
    );
    let kind = KindChoice::Date.default_kind(Locale::SpanishArgentina, today());
    let error = with_fields(&kind, &strings(&["2020-01-01", "31/12/2020"])).unwrap_err();
    assert!(matches!(error, OptionError::Date { field: "To", .. }));
}

#[test]
fn list_values_keep_their_weights_and_drop_empty_items() {
    let kind = ColumnKind::OneOf {
        options: vec![ListOption {
            value: "Mouse".into(),
            weight: 5,
        }],
    };
    let built = with_fields(&kind, &strings(&["Mouse, Teclado,, Monitor"])).unwrap();
    let ColumnKind::OneOf { options } = built else {
        panic!("not a list");
    };
    let found: Vec<(&str, u32)> = options
        .iter()
        .map(|option| (option.value.as_str(), option.weight))
        .collect();
    assert_eq!(found, [("Mouse", 5), ("Teclado", 1), ("Monitor", 1)]);
}

#[test]
fn email_domains_are_edited_as_a_list_and_sources_stay() {
    let kind = ColumnKind::Email {
        format: EmailFormat::InitialLast,
        first_name_from: Some("Name".into()),
        last_name_from: None,
        domains: Vec::new(),
    };
    let built = with_fields(&kind, &strings(&["a.com, b.com"])).unwrap();
    assert_eq!(
        built,
        ColumnKind::Email {
            format: EmailFormat::InitialLast,
            first_name_from: Some("Name".into()),
            last_name_from: None,
            domains: strings(&["a.com", "b.com"]),
        }
    );
}

#[test]
fn choices_replace_one_option_and_mark_the_current_one() {
    let kind = KindChoice::FullName.default_kind(Locale::SpanishArgentina, today());
    let groups = choice_groups(&kind);
    assert_eq!(groups.len(), 2);
    let female = &groups[0].choices[1];
    assert!(!female.selected);
    assert_eq!(
        female.kind,
        ColumnKind::FullName {
            gender: Gender::Female,
            last_names: LastNameCount::One
        }
    );
    assert!(groups[1].choices[0].selected);
}

#[test]
fn summaries_never_use_a_middle_dot() {
    for choice in KindChoice::ALL {
        let kind = choice.default_kind(Locale::SpanishArgentina, today());
        assert!(!summary(&kind).contains('·'));
    }
}
