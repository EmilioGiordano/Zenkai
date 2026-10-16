// Excel repairs a file whose table header cells differ from the column names the table
// part holds. A patch save copies table parts unchanged, so a sheet whose header cells no
// longer match is written without its relationships, which drops its tables.

use ironcalc::base::types::{Cell, Table, Workbook, Worksheet};

use crate::sheet_elements::cell_position;

pub(crate) fn tables_on<'b>(book: &'b Workbook, sheet: &Worksheet) -> Vec<&'b Table> {
    book.tables
        .values()
        .filter(|table| table.sheet_name == sheet.name)
        .collect()
}

pub(crate) fn headers_match(book: &Workbook, sheet: &Worksheet) -> bool {
    tables_on(book, sheet)
        .into_iter()
        .all(|table| header_matches(book, sheet, table))
}

fn header_matches(book: &Workbook, sheet: &Worksheet, table: &Table) -> bool {
    if table.header_row_count == 0 {
        return true;
    }
    let first = table.reference.split(':').next().and_then(cell_position);
    let Some((row, first_column)) = first else {
        return false;
    };
    table.columns.iter().enumerate().all(|(offset, column)| {
        let cell = u32::try_from(offset)
            .ok()
            .and_then(|offset| first_column.checked_add(offset))
            .and_then(|column| Some((i32::try_from(row).ok()?, i32::try_from(column).ok()?)))
            .and_then(|(row, column)| sheet.sheet_data.get(row, column));
        cell_text(book, cell).as_deref() == Some(column.name.as_str())
    })
}

// The text Excel takes a header cell's column name from; a formula or an empty cell has
// none.
fn cell_text(book: &Workbook, cell: Option<&Cell>) -> Option<String> {
    match cell? {
        Cell::SharedString { si, .. } => usize::try_from(*si)
            .ok()
            .and_then(|si| book.shared_strings.get(si))
            .cloned(),
        Cell::NumberCell { v, .. } => Some(v.to_string()),
        Cell::BooleanCell { v, .. } => Some(if *v { "TRUE" } else { "FALSE" }.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use ironcalc::base::Model;
    use ironcalc::base::types::{Table, TableColumn, TableStyleInfo};

    use super::headers_match;

    fn table(reference: &str, header_row_count: u32, names: &[&str]) -> Table {
        let columns = names
            .iter()
            .zip(1..)
            .map(|(name, id)| TableColumn {
                id,
                name: name.to_string(),
                totals_row_label: None,
                header_row_dxf_id: None,
                data_dxf_id: None,
                totals_row_dxf_id: None,
                totals_row_function: None,
            })
            .collect();
        Table {
            name: "Table1".to_string(),
            display_name: "Table1".to_string(),
            sheet_name: "Sheet1".to_string(),
            reference: reference.to_string(),
            totals_row_count: 0,
            header_row_count,
            header_row_dxf_id: None,
            data_dxf_id: None,
            totals_row_dxf_id: None,
            columns,
            style_info: TableStyleInfo {
                name: None,
                show_first_column: false,
                show_last_column: false,
                show_row_stripes: true,
                show_column_stripes: false,
            },
            has_filters: true,
        }
    }

    fn matches(inputs: &[(i32, i32, &str)], table: Table) -> bool {
        let mut model = Model::new_empty("b", "en", "UTC", "en").unwrap();
        for (row, column, input) in inputs {
            model
                .set_user_input(0, *row, *column, input.to_string())
                .unwrap();
        }
        model.workbook.tables.insert(table.name.clone(), table);
        headers_match(&model.workbook, &model.workbook.worksheets[0])
    }

    #[test]
    fn a_header_matches_only_cells_that_hold_its_column_names() {
        let header = [(2, 2, "Name"), (2, 3, "2020"), (3, 2, "x")];
        assert!(matches(&header, table("B2:C5", 1, &["Name", "2020"])));
        assert!(!matches(&header, table("B2:C5", 1, &["Name", "2021"])));
        assert!(!matches(
            &header,
            table("B2:D5", 1, &["Name", "2020", "Empty"])
        ));
        let formula = [(2, 2, "=\"Name\""), (2, 3, "2020")];
        assert!(!matches(&formula, table("B2:C5", 1, &["Name", "2020"])));
        assert!(matches(&[], table("B2:C5", 0, &["Name", "2020"])));
        assert!(!matches(&header, table("not a range", 1, &["Name"])));
    }
}
