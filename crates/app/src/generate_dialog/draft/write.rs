use std::iter;

use zenkai_types::{CellPos, MAX_COLS, MAX_ROWS, Range, RowIdx};

use super::{Draft, FieldIssue, PREVIEW_ROWS, Placement, WriteIssue};

#[derive(Clone, Debug)]
pub struct Block {
    pub origin: CellPos,
    pub rows: Vec<Vec<String>>,
    pub selection: Range,
}

impl Draft {
    // Unchanged headers are written back as they were, so a formula or a number header
    // is not turned into text.
    fn header_cells(&self) -> Vec<String> {
        self.columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                if column.header_changed() {
                    self.column_spec(index).header_input()
                } else {
                    column.original.clone()
                }
            })
            .collect()
    }

    // The first row written and how many rows the block holds, or why it cannot be written.
    fn fit(&self, generated: Option<usize>) -> Result<(RowIdx, usize), WriteIssue> {
        let (first_row, written) = match (generated, self.placement) {
            (None, _) => (u64::from(self.layout.header_row().get()), 1),
            (Some(rows), Placement::BelowHeaders) => {
                (u64::from(self.layout.header_row().get()), rows + 1)
            }
            (Some(rows), Placement::AfterData) => {
                if self.changed_headers() > 0 {
                    return Err(WriteIssue::HeadersWithAppendedRows);
                }
                (u64::from(self.layout.range.end.row.get()) + 1, rows)
            }
        };
        let first_row = u32::try_from(first_row)
            .ok()
            .and_then(RowIdx::new)
            .filter(|first| u64::from(first.get()) + written as u64 <= u64::from(MAX_ROWS))
            .ok_or(WriteIssue::PastLastRow)?;
        let last_col = u32::from(self.layout.first_col().get()) + self.columns.len() as u32;
        if last_col > u32::from(MAX_COLS) {
            return Err(WriteIssue::PastLastColumn);
        }
        Ok((first_row, written))
    }

    pub fn write_issue(&self) -> Option<WriteIssue> {
        self.fit(Some(self.rows as usize)).err()
    }

    pub fn block(&self, table: Option<Vec<Vec<String>>>) -> Result<Block, WriteIssue> {
        let generated = table.as_ref().map(Vec::len);
        let (first_row, written) = self.fit(generated)?;
        let rows: Vec<Vec<String>> = match table {
            None => vec![self.header_cells()],
            Some(table) if self.placement == Placement::AfterData => table,
            Some(table) => iter::once(self.header_cells()).chain(table).collect(),
        };
        let first_col = self.layout.first_col();
        let last_col = first_col.offset(self.columns.len() as i64 - 1);
        let selected = generated.unwrap_or(written);
        let selection = Range::new(
            CellPos::new(first_row.offset((written - selected) as i64), first_col),
            CellPos::new(first_row.offset(written as i64 - 1), last_col),
        );
        Ok(Block {
            origin: CellPos::new(first_row, first_col),
            rows,
            selection,
        })
    }

    pub fn preview(&self, table: &[Vec<String>]) -> Vec<Vec<String>> {
        let headers = self
            .columns
            .iter()
            .map(|column| column.header.clone())
            .collect();
        let shown = table.iter().take(PREVIEW_ROWS).map(|row| {
            row.iter()
                .map(|cell| cell.strip_prefix('\'').unwrap_or(cell).to_string())
                .collect()
        });
        iter::once(headers).chain(shown).collect()
    }

    pub fn summary(&self) -> String {
        let rows = count_label(self.rows, "row");
        match self.changed_headers() {
            0 => format!("Generates {rows}"),
            changed => format!(
                "Writes {} and generates {rows}",
                count_label(changed as u32, "header")
            ),
        }
    }

    pub fn generate_label(&self) -> String {
        format!("Generate {}", count_label(self.rows, "row"))
    }
}

pub fn parse_range(text: &str) -> Result<Range, FieldIssue> {
    let reference = text.rsplit_once('!').map_or(text, |(_, range)| range);
    Range::parse_a1(reference).ok_or_else(|| FieldIssue::Range(text.to_string()))
}

pub fn count_label(count: u32, noun: &str) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let plural = if count == 1 { "" } else { "s" };
    format!("{grouped} {noun}{plural}")
}
