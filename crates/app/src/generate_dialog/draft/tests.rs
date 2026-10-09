use zenkai_datagen::{LastNameCount, generate, validate};
use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId};

use super::*;

fn today() -> Date {
    Date::from_ymd(2026, 10, 8).unwrap()
}

fn pos(a1: &str) -> CellPos {
    CellPos::parse_a1(a1).unwrap()
}

fn layout(headers: &[&str], data_rows: u32) -> Layout {
    let last = CellPos::new(
        RowIdx::clamped(i64::from(data_rows)),
        ColIdx::clamped(headers.len() as i64 - 1),
    );
    Layout::from_range(Range::new(pos("A1"), last), |at| {
        if at.row.get() == 0 {
            headers
                .get(usize::from(at.col.get()))
                .map_or_else(String::new, |header| header.to_string())
        } else {
            String::new()
        }
    })
}

fn draft(headers: &[&str], data_rows: u32) -> Draft {
    Draft::new(
        layout(headers, data_rows),
        Locale::SpanishArgentina,
        7,
        today(),
    )
}

#[test]
fn headers_decide_the_detected_types_and_the_badge() {
    let draft = draft(&["Nombre", "Apellido", "Mail", "", "Total"], 0);
    let found: Vec<(KindChoice, KindSource)> = draft
        .columns()
        .iter()
        .map(|column| (KindChoice::of(&column.kind), column.kind_source))
        .collect();
    assert_eq!(
        found,
        [
            (KindChoice::FirstName, KindSource::Detected),
            (KindChoice::LastName, KindSource::Detected),
            (KindChoice::Email, KindSource::Detected),
            (KindChoice::Lorem, KindSource::Fallback),
            (KindChoice::Lorem, KindSource::Fallback),
        ]
    );
    assert_eq!(draft.rows(), DEFAULT_ROWS);
}

#[test]
fn selected_rows_fix_the_row_count() {
    let mut draft = draft(&["Nombre"], 100);
    assert_eq!(draft.rows(), 100);
    assert!(draft.rows_locked());
    draft.set_placement(Placement::AfterData);
    assert!(!draft.rows_locked());
    draft.set_rows_text("2.500");
    assert_eq!(draft.rows(), 2500);
    draft.set_placement(Placement::BelowHeaders);
    assert_eq!(draft.rows(), 100);
}

#[test]
fn renaming_an_empty_header_detects_the_type_and_discard_restores_the_cell() {
    let mut draft = draft(&["Nombre", ""], 0);
    draft.rename(1, "Teléfono");
    assert_eq!(draft.changed_headers(), 1);
    assert_eq!(KindChoice::of(&draft.columns()[1].kind), KindChoice::Phone);
    assert_eq!(draft.columns()[1].kind_source, KindSource::Detected);
    draft.discard(1);
    assert_eq!(draft.changed_headers(), 0);
    assert_eq!(draft.columns()[1].header, "");
    assert_eq!(draft.columns()[1].kind_source, KindSource::Fallback);
}

#[test]
fn a_chosen_type_survives_a_rename_and_customised_options_survive_the_same_type() {
    let mut draft = draft(&["Nombre", "Mail"], 0);
    draft.set_kind_choice(0, KindChoice::Boolean);
    draft.rename(0, "Apellido");
    assert_eq!(
        KindChoice::of(&draft.columns()[0].kind),
        KindChoice::Boolean
    );
    draft.set_option_fields(1, &["a.com".to_string()]);
    draft.rename(1, "Correo");
    let ColumnKind::Email { domains, .. } = &draft.columns()[1].kind else {
        panic!("not an email column");
    };
    assert_eq!(domains, &["a.com"]);
    draft.rename(1, "Presupuesto");
    assert_eq!(draft.columns()[1].kind_source, KindSource::Chosen);
    assert_eq!(KindChoice::of(&draft.columns()[1].kind), KindChoice::Email);
}

#[test]
fn the_locale_redetects_automatic_columns_only() {
    let mut draft = draft(&["Apellido", "Mail"], 0);
    draft.set_kind_choice(1, KindChoice::Email);
    let chosen = draft.columns()[1].kind.clone();
    draft.set_locale(Locale::EnglishUnitedStates);
    assert_eq!(
        draft.columns()[0].kind,
        ColumnKind::LastName {
            count: LastNameCount::One
        }
    );
    assert_eq!(draft.columns()[1].kind, chosen);
}

#[test]
fn email_sources_follow_renamed_columns() {
    let mut draft = draft(&["Nombre", "Apellido", "Mail"], 0);
    draft.rename(0, "Given");
    let spec = draft.spec();
    let ColumnKind::Email {
        first_name_from,
        last_name_from,
        ..
    } = &spec.columns[2].kind
    else {
        panic!("not an email column");
    };
    assert_eq!(first_name_from.as_deref(), Some("Given"));
    assert_eq!(last_name_from.as_deref(), Some("Apellido"));
    draft.set_source(2, NameRole::First, Source::NoColumn);
    assert_eq!(draft.resolved_source(2, NameRole::First), None);
    assert_eq!(draft.source_candidates(2, NameRole::Last), [1]);
    assert!(validate(&draft.spec()).is_ok());
}

#[test]
fn a_full_name_column_is_the_second_choice_for_both_roles() {
    let draft = draft(&["Full name", "Mail"], 0);
    assert_eq!(draft.resolved_source(1, NameRole::First), Some(0));
    assert_eq!(draft.resolved_source(1, NameRole::Last), Some(0));
}

#[test]
fn the_spec_carries_the_dialog_settings() {
    let mut draft = draft(&["Nombre", "Id"], 0);
    draft.set_rows_text("50");
    draft.set_seed_text("99");
    draft.set_blanks(0, "20%");
    draft.set_unique(1, true);
    let spec = draft.spec();
    assert_eq!((spec.rows, spec.seed), (50, 99));
    assert_eq!(spec.columns[0].blanks.get(), 20);
    assert!(spec.columns[1].unique);
    let table = generate(&spec).unwrap();
    assert_eq!(table.len(), 50);
}

#[test]
fn bad_numbers_are_reported_and_keep_the_previous_value() {
    let mut draft = draft(&["Nombre"], 0);
    draft.set_rows_text("many");
    assert_eq!(draft.rows(), DEFAULT_ROWS);
    assert!(matches!(draft.field_issue(), Some(FieldIssue::Rows(_))));
    draft.set_rows_text("10");
    assert!(draft.field_issue().is_none());
    draft.set_blanks(0, "150");
    assert!(draft.has_local_issue());
    draft.set_blanks(0, "");
    assert!(!draft.has_local_issue());
}

#[test]
fn editing_the_range_keeps_edited_columns_and_refreshes_the_rest() {
    let mut draft = draft(&["Nombre", "Mail"], 0);
    draft.rename(0, "Apellido");
    draft.set_kind_choice(1, KindChoice::Boolean);
    draft.relayout(layout(&["Name", "Phone", "City"], 0));
    let headers: Vec<&str> = draft.columns().iter().map(|c| c.header.as_str()).collect();
    assert_eq!(headers, ["Apellido", "Phone", "City"]);
    assert_eq!(draft.columns()[0].original, "Name");
    assert_eq!(KindChoice::of(&draft.columns()[1].kind), KindChoice::Phone);
    assert_eq!(draft.changed_headers(), 1);
}

#[test]
fn added_columns_can_be_removed_but_sheet_columns_cannot() {
    let mut draft = draft(&["Nombre"], 0);
    draft.add_column();
    assert!(draft.is_added(1));
    draft.remove_added_column(0);
    assert_eq!(draft.columns().len(), 2);
    draft.remove_added_column(1);
    assert_eq!(draft.columns().len(), 1);
}

#[test]
fn range_text_accepts_a_sheet_prefix() {
    assert_eq!(parse_range("Personas!A1:E1").unwrap().to_string(), "A1:E1");
    assert!(matches!(parse_range("nope"), Err(FieldIssue::Range(_))));
}

#[test]
fn labels_use_thousands_separators_and_singulars() {
    assert_eq!(count_label(1000, "row"), "1,000 rows");
    assert_eq!(count_label(1, "header"), "1 header");
    assert_eq!(count_label(1_048_575, "row"), "1,048,575 rows");
    let mut draft = draft(&["Nombre", "Mail"], 0);
    assert_eq!(draft.summary(), "Generates 1,000 rows");
    draft.rename(1, "Correo");
    assert_eq!(draft.summary(), "Writes 1 header and generates 1,000 rows");
    assert_eq!(draft.generate_label(), "Generate 1,000 rows");
}

#[test]
fn the_preview_shows_headers_and_four_rows_without_literal_quotes() {
    let draft = draft(&["Telefono"], 0);
    let table: Vec<Vec<String>> = (0..9).map(|n| vec![format!("'+54 {n}")]).collect();
    let preview = draft.preview(&table);
    assert_eq!(preview.len(), 1 + PREVIEW_ROWS);
    assert_eq!(preview[0], ["Telefono"]);
    assert_eq!(preview[1], ["+54 0"]);
}

#[test]
fn a_block_writes_changed_headers_as_text_and_unchanged_ones_as_they_were() {
    let mut draft = draft(&["Nombre", "=A1"], 0);
    draft.rename(0, "=Total");
    let block = draft
        .block(Some(vec![vec!["x".into(), "y".into()]]))
        .unwrap();
    assert_eq!(block.origin, pos("A1"));
    assert_eq!(block.rows[0], ["'=Total", "=A1"]);
    assert_eq!(block.selection, Range::parse_a1("A2:B2").unwrap());
    let headers_only = draft.block(None).unwrap();
    assert_eq!(headers_only.rows.len(), 1);
    assert_eq!(headers_only.selection, Range::parse_a1("A1:B1").unwrap());
}

#[test]
fn rows_after_the_data_start_below_it_and_refuse_unsaved_headers() {
    let mut draft = draft(&["Nombre"], 3);
    draft.set_placement(Placement::AfterData);
    let block = draft.block(Some(vec![vec!["x".into()]])).unwrap();
    assert_eq!(block.origin, pos("A5"));
    draft.rename(0, "Otro");
    assert_eq!(
        draft.block(Some(Vec::new())).unwrap_err(),
        WriteIssue::HeadersWithAppendedRows
    );
}

#[test]
fn a_block_that_leaves_the_sheet_is_refused() {
    let draft = Draft::new(
        Layout::from_range(
            Range::single(CellPos::new(RowIdx::LAST, ColIdx::default())),
            |_| String::new(),
        ),
        Locale::SpanishArgentina,
        1,
        today(),
    );
    assert_eq!(
        draft.block(Some(vec![vec![String::new()]])).unwrap_err(),
        WriteIssue::PastLastRow
    );
}

#[test]
fn generating_writes_headers_and_rows_in_one_undo_step() {
    let mut workbook = Workbook::new_empty().unwrap();
    let sheet = SheetId(0);
    workbook
        .set_inputs(
            sheet,
            pos("A1"),
            &[vec![
                "Nombre".to_string(),
                "=1+1".to_string(),
                "".to_string(),
            ]],
        )
        .unwrap();
    let mut draft = Draft::new(
        Layout::from_range(Range::parse_a1("A1:C1").unwrap(), |at| {
            workbook.input(sheet, at)
        }),
        Locale::SpanishArgentina,
        5,
        today(),
    );
    draft.rename(0, "Apellido");
    draft.rename(2, "Teléfono");
    draft.set_rows_text("20");
    let table = generate(&draft.spec()).unwrap();
    let block = draft.block(Some(table)).unwrap();
    workbook
        .set_inputs(sheet, block.origin, &block.rows)
        .unwrap();
    assert_eq!(workbook.input(sheet, pos("A1")), "Apellido");
    assert_eq!(workbook.input(sheet, pos("B1")), "=1+1");
    assert_eq!(workbook.input(sheet, pos("C1")), "Teléfono");
    assert!(!workbook.input(sheet, pos("A21")).is_empty());
    assert!(workbook.input(sheet, pos("A22")).is_empty());
    workbook.undo().unwrap();
    assert_eq!(workbook.input(sheet, pos("A1")), "Nombre");
    assert_eq!(workbook.input(sheet, pos("B1")), "=1+1");
    assert_eq!(workbook.input(sheet, pos("C1")), "");
    assert!(workbook.input(sheet, pos("A2")).is_empty());
    assert!(workbook.input(sheet, pos("A21")).is_empty());
}

#[test]
fn a_formula_header_that_is_renamed_stays_text() {
    let mut workbook = Workbook::new_empty().unwrap();
    let sheet = SheetId(0);
    let mut draft = draft(&["Total"], 0);
    draft.rename(0, "=SUM(A1)");
    draft.set_kind_choice(0, KindChoice::Integer);
    draft.set_rows_text("2");
    let table = generate(&draft.spec()).unwrap();
    let block = draft.block(Some(table)).unwrap();
    workbook
        .set_inputs(sheet, block.origin, &block.rows)
        .unwrap();
    assert_eq!(workbook.cell(sheet, pos("A1")).text, "=SUM(A1)");
}
