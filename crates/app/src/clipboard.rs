use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId, ValueKind};

pub const MAX_COPY_CELLS: u64 = 1_000_000;

// What was copied, as values: text that would read as a formula keeps a leading
// apostrophe, which Excel and the engine treat as "this is text".
pub fn values_only(text: &str) -> Vec<Vec<String>> {
    parse_tsv(text)
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|field| {
                    if field.starts_with('=') {
                        format!("'{field}")
                    } else {
                        field
                    }
                })
                .collect()
        })
        .collect()
}

// The values of copied cells: numbers at full precision, everything else as shown.
pub fn cell_values(workbook: &Workbook, sheet: SheetId, range: Range) -> Vec<Vec<String>> {
    (range.start.row.get()..=range.end.row.get())
        .map(|row| {
            (range.start.col.get()..=range.end.col.get())
                .map(|col| {
                    let pos = CellPos::new(
                        RowIdx::clamped(i64::from(row)),
                        ColIdx::clamped(i64::from(col)),
                    );
                    let view = workbook.cell(sheet, pos);
                    match (view.kind, view.number) {
                        (ValueKind::Number, Some(number)) => number.to_string(),
                        _ if view.text.starts_with('=') => format!("'{}", view.text),
                        _ => view.text,
                    }
                })
                .collect()
        })
        .collect()
}

pub fn parse_tsv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut at_field_start = true;
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if at_field_start => {
                quoted = true;
                at_field_start = false;
            }
            '\t' => {
                row.push(std::mem::take(&mut field));
                at_field_start = true;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                at_field_start = true;
            }
            _ => {
                field.push(c);
                at_field_start = false;
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_cells_paste_their_full_precision_values() {
        let mut book = Workbook::new_empty().unwrap();
        let rows = vec![vec![
            "3.14159".to_string(),
            "=A1*2".to_string(),
            "hola".to_string(),
        ]];
        book.set_inputs(SheetId(0), CellPos::default(), &rows)
            .unwrap();
        let range = Range::parse_a1("A1:C1").unwrap();
        book.set_number_format(SheetId(0), range, "0.00").unwrap();
        assert_eq!(
            cell_values(&book, SheetId(0), range),
            vec![vec!["3.14159", "6.28318", "hola"]]
        );
    }

    #[test]
    fn values_keep_formula_looking_text_as_text() {
        let rows = values_only(
            "=A1	5
-3	$4.00
",
        );
        assert_eq!(rows, vec![vec!["'=A1", "5"], vec!["-3", "$4.00"]]);
    }

    #[test]
    fn parses_excel_clipboard() {
        let rows = parse_tsv("a\tb\r\n\"multi\nline\"\t\"q\"\"x\"\r\n");
        assert_eq!(rows, vec![vec!["a", "b"], vec!["multi\nline", "q\"x"]]);
        assert_eq!(parse_tsv("1\t2"), vec![vec!["1", "2"]]);
        assert!(parse_tsv("").is_empty());
    }
}
