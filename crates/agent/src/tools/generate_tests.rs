use serde_json::json;
use zenkai_datagen::{DatagenError, GenerationSpec};
use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, Range, SheetId};

use super::*;

const BOOK: WorkbookId = WorkbookId(5);

fn spec(rows: u32) -> GenerationSpec {
    serde_json::from_value(json!({
        "rows": rows,
        "locale": "en-US",
        "seed": 42,
        "columns": [
            { "header": "Id", "kind": { "type": "sequential_id", "start": 100 } },
            { "header": "Size", "kind": { "type": "one_of", "options": [{ "value": "Small" }, { "value": "Large" }] } },
            { "header": "Units", "kind": { "type": "integer", "min": 1, "max": 9 } }
        ]
    }))
    .unwrap()
}

fn generate_at(start: &str, spec: GenerationSpec) -> GenerateData {
    GenerateData {
        workbook: BOOK,
        sheet: "Sheet1".to_string(),
        start: start.to_string(),
        spec,
    }
}

fn host() -> LocalHost {
    LocalHost {
        id: BOOK,
        name: "Fake.xlsx".to_string(),
        workbook: Workbook::new_empty().unwrap(),
        selection: (SheetId(0), Range::parse_a1("A1").unwrap()),
        access: AgentAccess::Editable,
    }
}

#[test]
fn specs_over_the_budget_or_the_sheet_are_refused_before_generating() {
    let mut host = host();
    let mut too_many_rows = spec(1);
    too_many_rows.rows = u32::MAX;
    assert!(matches!(
        host.handle(&generate_at("A1", too_many_rows).into()),
        Err(ToolError::Generation(DatagenError::TooManyRows { .. }))
    ));
    let mut too_many_cells = spec(1_000_000);
    too_many_cells.columns = vec![too_many_cells.columns[0].clone(); 6];
    for (position, column) in too_many_cells.columns.iter_mut().enumerate() {
        column.header = format!("C{position}");
    }
    assert!(matches!(
        host.handle(&generate_at("A1", too_many_cells).into()),
        Err(ToolError::Generation(DatagenError::TooManyCells { .. }))
    ));
    let long_text: GenerationSpec = serde_json::from_value(json!({
        "rows": 1_000_000,
        "locale": "en-US",
        "seed": 1,
        "columns": [{ "header": "Notes", "kind": { "type": "lorem", "min_words": 200, "max_words": 200 } }]
    }))
    .unwrap();
    assert!(matches!(
        host.handle(&generate_at("A1", long_text).into()),
        Err(ToolError::Generation(DatagenError::OutputTooLarge { .. }))
    ));
    let mut no_columns = spec(3);
    no_columns.columns.clear();
    assert_eq!(
        host.handle(&generate_at("A1", no_columns).into()),
        Err(ToolError::Generation(DatagenError::NoColumns))
    );
    assert_eq!(
        host.handle(&generate_at("A1048575", spec(2)).into()),
        Err(ToolError::OutsideSheet)
    );
    assert_eq!(
        host.handle(&generate_at("XFC1", spec(2)).into()),
        Err(ToolError::OutsideSheet)
    );
    assert_eq!(
        host.workbook
            .input(SheetId(0), CellPos::parse_a1("A1").unwrap()),
        ""
    );
}

#[test]
fn approval_text_names_the_table_size_and_target() {
    let request = WriteRequest::GenerateData(generate_at("B2", spec(20)));
    let plan = plan_write(&request, &host().workbook.sheets()).unwrap();
    assert_eq!(
        plan.headline(),
        "Generate 20 rows x 3 columns of synthetic data in Sheet1!B2:D22, under a header row"
    );
    assert_eq!(
        plan.sample().as_deref(),
        Some("\"Id\", \"Size\", \"Units\", …")
    );
    assert_eq!(plan.input_block(), Some(Range::parse_a1("B2:D22").unwrap()));
    let single = WriteRequest::GenerateData(generate_at("A1", {
        let mut one = spec(1);
        one.columns.truncate(1);
        one
    }));
    let plan = plan_write(&single, &host().workbook.sheets()).unwrap();
    assert_eq!(
        plan.headline(),
        "Generate 1 row x 1 column of synthetic data in Sheet1!A1:A2, under a header row"
    );
}

#[test]
fn a_generated_table_lands_as_one_undo_step() {
    let mut host = host();
    let sheet = SheetId(0);
    let start = CellPos::parse_a1("C3").unwrap();
    let reply = host.handle(&generate_at("C3", spec(5)).into()).unwrap();
    assert_eq!(
        reply,
        ToolReply::Written(WriteSummary {
            range: Range::parse_a1("C3:E8").unwrap(),
            cells: 18,
        })
    );
    let expected = zenkai_datagen::generate(&spec(5)).unwrap();
    let headers = ["Id", "Size", "Units"];
    for (col, header) in headers.iter().enumerate() {
        let pos = CellPos::new(start.row, start.col.offset(col as i64));
        assert_eq!(host.workbook.input(sheet, pos), *header);
    }
    for (row, values) in expected.iter().enumerate() {
        for (col, value) in values.iter().enumerate() {
            let pos = CellPos::new(
                start.row.offset(row as i64 + 1),
                start.col.offset(col as i64),
            );
            assert_eq!(&host.workbook.input(sheet, pos), value);
        }
    }
    assert_eq!(
        host.workbook.input(sheet, CellPos::parse_a1("C4").unwrap()),
        "100"
    );
    host.workbook.undo().unwrap();
    for corner in ["C3", "E8"] {
        assert_eq!(
            host.workbook
                .input(sheet, CellPos::parse_a1(corner).unwrap()),
            ""
        );
    }
}
