// Test helpers outside #[test] functions; clippy only exempts the functions themselves.
#![allow(clippy::unwrap_used)]

use zenkai_datagen::{ColumnKind, ColumnSpec, GenerationSpec, Percent, generate};

const AGENT_SPEC: &str = r#"{
  "rows": 1000,
  "locale": "es-AR",
  "seed": 48213,
  "columns": [
    { "header": "Nombre", "kind": { "type": "first_name" } },
    { "header": "Apellido", "kind": { "type": "last_name", "count": "one_or_two" } },
    {
      "header": "Email",
      "kind": {
        "type": "email",
        "format": "first_dot_last",
        "first_name_from": "Nombre",
        "last_name_from": "Apellido",
        "domains": ["gmail.com", "hotmail.com", "outlook.com"]
      },
      "blanks": 5,
      "unique": true
    },
    { "header": "Teléfono", "kind": { "type": "phone", "pattern": "+54 9 11 ####-####" }, "blanks": 20 },
    { "header": "Alta", "kind": { "type": "date", "from": "2020-01-01", "to": "2026-10-08" } },
    {
      "header": "Producto",
      "kind": {
        "type": "one_of",
        "options": [{ "value": "Teclado", "weight": 3 }, { "value": "Mouse" }]
      }
    },
    { "header": "Precio", "kind": { "type": "decimal", "min": 20, "max": 1800, "places": 2 } },
    { "header": "SKU", "kind": { "type": "pattern", "pattern": "AAA-####" }, "unique": true },
    { "header": "Id", "kind": { "type": "sequential_id" } },
    { "header": "Ciudad", "kind": { "type": "city" } }
  ]
}"#;

#[test]
fn agent_json_parses_generates_and_round_trips() {
    let spec: GenerationSpec = serde_json::from_str(AGENT_SPEC).unwrap();
    assert_eq!(spec.columns[2].blanks, Percent::new(5).unwrap());
    assert_eq!(
        spec.columns[8].kind,
        ColumnKind::SequentialId { start: 1, step: 1 }
    );
    assert_eq!(generate(&spec).unwrap().len(), 1000);
    let json = serde_json::to_string(&spec).unwrap();
    assert_eq!(serde_json::from_str::<GenerationSpec>(&json).unwrap(), spec);
}

fn parse_error(json: &str) -> String {
    serde_json::from_str::<GenerationSpec>(json)
        .unwrap_err()
        .to_string()
}

fn one_column(column: &str) -> String {
    format!(r#"{{ "rows": 1, "locale": "en-US", "seed": 1, "columns": [{column}] }}"#)
}

#[test]
fn malformed_json_is_rejected_with_a_reason() {
    let unknown_option = one_column(
        r#"{ "header": "A", "kind": { "type": "integer", "min": 1, "max": 2, "step": 1 } }"#,
    );
    assert!(parse_error(&unknown_option).contains("unknown field `step`"));
    let unknown_column_field =
        one_column(r#"{ "header": "A", "kind": { "type": "city" }, "nulls": 5 }"#);
    assert!(parse_error(&unknown_column_field).contains("unknown field `nulls`"));
    for kind in ["street_address", "city", "company", "boolean", "uuid"] {
        let unknown_field = one_column(&format!(
            r#"{{ "header": "A", "kind": {{ "type": "{kind}", "unique": true }} }}"#
        ));
        assert!(
            parse_error(&unknown_field).contains("unknown field `unique`"),
            "{kind}"
        );
    }
    let unknown_kind = one_column(r#"{ "header": "A", "kind": { "type": "ssn" } }"#);
    assert!(parse_error(&unknown_kind).contains("unknown variant `ssn`"));
    let bad_percent = one_column(r#"{ "header": "A", "kind": { "type": "city" }, "blanks": 120 }"#);
    assert!(parse_error(&bad_percent).contains("120 is not a percentage"));
    let bad_date = one_column(
        r#"{ "header": "A", "kind": { "type": "date", "from": "14/03/2022", "to": "2024-01-01" } }"#,
    );
    assert!(parse_error(&bad_date).contains("\"14/03/2022\" is not a date"));
    let bad_locale = r#"{ "rows": 1, "locale": "es-ES", "seed": 1, "columns": [] }"#;
    assert!(parse_error(bad_locale).contains("unknown variant `es-ES`"));
}

#[test]
fn schema_matches_the_committed_snapshot() {
    let generated = serde_json::to_value(schemars::schema_for!(GenerationSpec)).unwrap();
    let committed: serde_json::Value =
        serde_json::from_str(include_str!("../generation-spec.schema.json")).unwrap();
    assert_eq!(
        generated, committed,
        "the spec changed: update crates/datagen/generation-spec.schema.json"
    );
}

#[test]
fn header_input_never_becomes_a_formula() {
    let header_input = |header: &str| {
        ColumnSpec {
            header: header.to_string(),
            kind: ColumnKind::City {},
            blanks: Percent::ZERO,
            unique: false,
        }
        .header_input()
    };
    for header in ["=SUM(A1:A9)", "+54", "-1+2", "@cmd", "TRUE", "2024"] {
        assert_eq!(header_input(header), format!("'{header}"));
    }
    for header in ["Nombre", "Teléfono", "Precio 2024", ""] {
        assert_eq!(header_input(header), header);
    }
}
