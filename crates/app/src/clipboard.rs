use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, SheetId, ValueKind};

use crate::visible_cells::CopyShape;

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
pub fn cell_values(workbook: &Workbook, sheet: SheetId, shape: &CopyShape) -> Vec<Vec<String>> {
    shape
        .rows()
        .map(|row| {
            shape
                .cols()
                .map(|col| {
                    let view = workbook.cell(sheet, CellPos::new(row, col));
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

pub fn shown_text(workbook: &Workbook, sheet: SheetId, shape: &CopyShape) -> String {
    let mut text = String::new();
    for (index, row) in shape.rows().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let line_start = text.len();
        for (offset, col) in shape.cols().enumerate() {
            if offset > 0 {
                text.push('\t');
            }
            push_field(
                &mut text,
                &workbook.cell(sheet, CellPos::new(row, col)).text,
            );
        }
        if text.len() == line_start {
            text.push_str("\"\"");
        }
    }
    text
}

// Quotes like the engine's own clipboard text, so `parse_tsv` reads it back.
fn push_field(text: &mut String, field: &str) {
    if field.contains(['\t', '\n', '\r', '"']) {
        text.push('"');
        text.push_str(&field.replace('"', "\"\""));
        text.push('"');
    } else {
        text.push_str(field);
    }
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
    use zenkai_types::Range;

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
            cell_values(&book, SheetId(0), &CopyShape::whole(range)),
            vec![vec!["3.14159", "6.28318", "hola"]]
        );
    }

    #[test]
    fn shown_text_joins_the_cells_of_a_shape_with_tabs_and_lines() {
        let mut book = Workbook::new_empty().unwrap();
        let rows = vec![
            vec!["1".to_string(), "a".to_string()],
            vec!["2".to_string(), "b".to_string()],
        ];
        book.set_inputs(SheetId(0), CellPos::default(), &rows)
            .unwrap();
        let shape = CopyShape::whole(Range::parse_a1("A1:B2").unwrap());
        assert_eq!(shown_text(&book, SheetId(0), &shape), "1\ta\n2\tb");
    }

    #[test]
    fn a_field_with_a_tab_a_line_break_or_a_quote_is_quoted_and_parses_back() {
        let mut text = String::new();
        for field in ["a\tb", "two\nlines", "say \"hi\"", "plain"] {
            push_field(&mut text, field);
            text.push('\t');
        }
        assert_eq!(
            text,
            "\"a\tb\"\t\"two\nlines\"\t\"say \"\"hi\"\"\"\tplain\t"
        );
        assert_eq!(
            parse_tsv(&text),
            vec![vec!["a\tb", "two\nlines", "say \"hi\"", "plain", ""]]
        );
    }

    #[test]
    fn a_blank_line_of_one_column_is_written_as_an_empty_quoted_field() {
        let mut book = Workbook::new_empty().unwrap();
        let rows = vec![
            vec!["a".to_string()],
            vec![String::new()],
            vec!["b".to_string()],
        ];
        book.set_inputs(SheetId(0), CellPos::default(), &rows)
            .unwrap();
        let shape = CopyShape::whole(Range::parse_a1("A1:A3").unwrap());
        let text = shown_text(&book, SheetId(0), &shape);
        assert_eq!(text, "a\n\"\"\nb");
        assert_eq!(parse_tsv(&text), vec![vec!["a"], vec![""], vec!["b"]]);
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
