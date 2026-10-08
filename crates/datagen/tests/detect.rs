use zenkai_datagen::{
    ColumnKind, ColumnSpec, EmailFormat, Gender, GenerationSpec, LastNameCount, Locale, Percent,
    detect_kind, detect_kinds, generate,
};

#[test]
fn detects_spanish_and_english_synonyms_ignoring_case_and_accents() {
    let detected = |header: &str| detect_kind(header, Locale::SpanishArgentina);
    let first_name = Some(ColumnKind::FirstName {
        gender: Gender::Any,
    });
    for header in ["Nombre", "NAME", "first name", "First_Name", "  nombre  "] {
        assert_eq!(detected(header), first_name, "{header}");
    }
    let is = |header: &str, check: fn(&ColumnKind) -> bool| {
        assert!(detected(header).as_ref().is_some_and(check), "{header}");
    };
    for header in ["Apellido", "Last Name", "SURNAME"] {
        is(header, |kind| matches!(kind, ColumnKind::LastName { .. }));
    }
    for header in ["Nombre completo", "Full name"] {
        is(header, |kind| matches!(kind, ColumnKind::FullName { .. }));
    }
    for header in ["Email", "Mail", "Correo", "e-mail", "Correo electrónico"] {
        is(header, |kind| matches!(kind, ColumnKind::Email { .. }));
    }
    for header in ["Teléfono", "telefono", "Phone", "Celular"] {
        is(header, |kind| matches!(kind, ColumnKind::Phone { .. }));
    }
    for header in ["Fecha", "Date", "Alta"] {
        is(header, |kind| matches!(kind, ColumnKind::Date { .. }));
    }
    for header in ["Precio", "Price", "Importe"] {
        is(header, |kind| matches!(kind, ColumnKind::Decimal { .. }));
    }
    is("ID", |kind| matches!(kind, ColumnKind::SequentialId { .. }));
    for header in ["Código", "codigo", "SKU"] {
        is(header, |kind| matches!(kind, ColumnKind::Pattern { .. }));
    }
    for header in ["Ciudad", "City"] {
        is(header, |kind| matches!(kind, ColumnKind::City));
    }
    for header in ["Dirección", "direccion", "Address"] {
        is(header, |kind| matches!(kind, ColumnKind::StreetAddress));
    }
    for header in ["Empresa", "Company", "Compañía"] {
        is(header, |kind| matches!(kind, ColumnKind::Company));
    }
    for header in ["", "Notas", "Apellido materno", "nombre de usuario"] {
        assert_eq!(detected(header), None, "{header}");
    }
}

#[test]
fn detected_defaults_follow_the_locale() {
    assert_eq!(
        detect_kind("Phone", Locale::EnglishUnitedStates),
        Some(ColumnKind::Phone {
            pattern: "(###) ###-####".to_string()
        })
    );
    assert_eq!(
        detect_kind("Apellido", Locale::SpanishArgentina),
        Some(ColumnKind::LastName {
            count: LastNameCount::OneOrTwo
        })
    );
}

#[test]
fn detected_emails_link_to_the_name_columns() {
    let kinds = detect_kinds(
        &["Nombre", "Apellido", "Mail", "Teléfono", "Alta"],
        Locale::SpanishArgentina,
    );
    assert_eq!(
        kinds[2],
        Some(ColumnKind::Email {
            format: EmailFormat::FirstDotLast,
            first_name_from: Some("Nombre".to_string()),
            last_name_from: Some("Apellido".to_string()),
            domains: ["gmail.com", "hotmail.com", "outlook.com", "yahoo.com.ar"]
                .map(String::from)
                .to_vec(),
        })
    );
    let kinds = detect_kinds(&["Full name", "Email"], Locale::EnglishUnitedStates);
    assert!(matches!(
        &kinds[1],
        Some(ColumnKind::Email { first_name_from: Some(first), last_name_from: Some(last), .. })
            if first == "Full name" && last == "Full name"
    ));
    let kinds = detect_kinds(&["Nombre", "Nombre", "Email"], Locale::SpanishArgentina);
    assert!(matches!(
        &kinds[2],
        Some(ColumnKind::Email {
            first_name_from: None,
            last_name_from: None,
            ..
        })
    ));
}

#[test]
fn detected_specs_generate_without_errors() {
    let headers = [
        "Nombre",
        "Apellido",
        "Mail",
        "Teléfono",
        "Alta",
        "Precio",
        "SKU",
        "Id",
    ];
    let columns = detect_kinds(&headers, Locale::SpanishArgentina)
        .into_iter()
        .zip(headers)
        .filter_map(|(kind, header)| {
            kind.map(|kind| ColumnSpec {
                header: header.to_string(),
                kind,
                blanks: Percent::ZERO,
                unique: false,
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(columns.len(), headers.len());
    let spec = GenerationSpec {
        rows: 10,
        locale: Locale::SpanishArgentina,
        seed: 1,
        columns,
    };
    assert!(generate(&spec).is_ok_and(|rows| rows.len() == 10));
}
