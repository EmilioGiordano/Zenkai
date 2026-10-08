use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, Range, RowIdx, SheetId};

pub const MAX_COPY_CELLS: u64 = 1_000_000;

pub fn copy_tsv(workbook: &Workbook, sheet: SheetId, range: Range) -> Option<String> {
    let end = workbook.used_end(sheet);
    let clipped = Range::new(
        range.start,
        CellPos::new(range.end.row.min(end.row), range.end.col.min(end.col)),
    );
    if clipped.cell_count() > MAX_COPY_CELLS {
        return None;
    }
    let mut out = String::new();
    for row in clipped.start.row.get()..=clipped.end.row.get() {
        for col in clipped.start.col.get()..=clipped.end.col.get() {
            if col > clipped.start.col.get() {
                out.push('\t');
            }
            let pos = CellPos::new(
                RowIdx::clamped(i64::from(row)),
                ColIdx::clamped(i64::from(col)),
            );
            out.push_str(&quote(&workbook.cell(sheet, pos).text));
        }
        out.push_str("\r\n");
    }
    Some(out)
}

fn quote(text: &str) -> String {
    if text.contains(['\t', '\n', '\r', '"']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
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

    #[test]
    fn parses_excel_clipboard() {
        let rows = parse_tsv("a\tb\r\n\"multi\nline\"\t\"q\"\"x\"\r\n");
        assert_eq!(rows, vec![vec!["a", "b"], vec!["multi\nline", "q\"x"]]);
        assert_eq!(parse_tsv("1\t2"), vec![vec!["1", "2"]]);
        assert!(parse_tsv("").is_empty());
    }

    #[test]
    fn quotes_round_trip() {
        let text = format!("{}\t{}\r\n", quote("a\tb"), quote("say \"hi\""));
        assert_eq!(parse_tsv(&text), vec![vec!["a\tb", "say \"hi\""]]);
    }
}
