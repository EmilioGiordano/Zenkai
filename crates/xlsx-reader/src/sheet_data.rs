use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use ironcalc::base::cell_store::CellStore;
use ironcalc::base::expressions::utils::{
    column_to_number, is_valid_column_number, is_valid_row, parse_reference_a1,
};
use ironcalc::base::types::{Cell, Row};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

mod values;
mod xml;

use crate::ReadError;
use crate::limits::{self, BYTES_PER_UNIQUE_STRING};
use crate::text::parse_bool_false;
use values::{ValueCell, formula_cell};

// Anything the checker in the engine's preflight looks at; inside <sheetData> only <f> is
// expected, so any other one means a file this reader does not handle.
const CHECKED_ELEMENTS: [&[u8]; 8] = [
    b"formula",
    b"formula1",
    b"formula2",
    b"definedName",
    b"hyperlinks",
    b"dataValidations",
    b"autoFilter",
    b"sheetProtection",
];

const DEFAULT_ROW_HEIGHT: f64 = 14.5;

// The cells of a sheet in document order, built straight into the model's cell store as
// the pieces of its XML are read. Cells listed in `string_cells` hold a sheet-local string
// index and every formula cell, listed in `formula_cells` in the order of `events`, holds
// an index into `events`; both are resolved once all sheets are read, because IronCalc
// numbers strings and formulas in document order. Formula texts are owned so row pieces
// can be dropped as the worksheet streams in.
#[derive(Default)]
pub(crate) struct SheetCells {
    pub(crate) rows: Vec<Row>,
    pub(crate) data: CellStore,
    pub(crate) strings: Vec<Arc<str>>,
    pub(crate) string_cells: Vec<Slot>,
    pub(crate) formula_cells: Vec<Slot>,
    pub(crate) events: Vec<FormulaEvent>,
    pub(crate) jobs: Vec<FormulaJob>,
    string_index: HashMap<Arc<str>, usize>,
    last_row: i32,
    cell_count: u64,
    string_bytes: u64,
    job_bytes: u64,
}

// What the memory budget is charged for, so a piece pays for the difference it added.
#[derive(Clone, Copy, Default)]
pub(crate) struct Totals {
    pub(crate) cells: u64,
    pub(crate) formulas: u64,
    pub(crate) rows: u64,
    pub(crate) bytes: u64,
}

impl SheetCells {
    pub(crate) fn totals(&self) -> Totals {
        Totals {
            cells: self.cell_count,
            formulas: self.events.len() as u64,
            rows: self.data.row_count() as u64,
            bytes: self.string_bytes.saturating_add(self.job_bytes),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Slot {
    pub(crate) row: i32,
    pub(crate) column: i32,
}

pub(crate) enum FormulaEvent {
    Convert { job: usize },
    SharedAnchor { si: i32, job: usize },
    SharedChild { si: i32 },
}

pub(crate) struct FormulaJob {
    pub(crate) text: String,
    pub(crate) row: i32,
    pub(crate) column: i32,
    pub(crate) array: bool,
}

pub(crate) fn unreadable(error: impl ToString) -> ReadError {
    ReadError::Unreadable(error.to_string())
}

fn unsupported(what: &str) -> ReadError {
    ReadError::Unsupported(what.to_string())
}

// Reads a run of whole <row> elements, the content of <sheetData> or a piece of it, into
// `cells`. True when an array formula claims rows past the last one read: a later piece
// would read those cells without knowing their anchor.
pub(crate) fn scan_rows_into(
    cells: &mut SheetCells,
    rows: &str,
    prefixes: &[Vec<u8>],
    sheet_name: &str,
) -> Result<bool, ReadError> {
    let mut reader = Reader::from_str(rows);
    let config = reader.config_mut();
    config.check_end_names = true;
    config.check_comments = true;
    let mut scanner = Scanner {
        reader,
        prefixes,
        sheet_name,
        array_cells: HashMap::new(),
        array_cells_claimed: 0,
        spill_end: 0,
        row_buffer: Vec::new(),
        cells,
    };
    scanner.rows()?;
    Ok(scanner.spill_end > scanner.cells.last_row)
}

#[cfg(test)]
pub(crate) fn scan_rows(
    rows: &str,
    prefixes: &[Vec<u8>],
    sheet_name: &str,
) -> Result<SheetCells, ReadError> {
    let mut cells = SheetCells::default();
    scan_rows_into(&mut cells, rows, prefixes, sheet_name)?;
    Ok(cells)
}

struct Scanner<'a, 's> {
    reader: Reader<&'a [u8]>,
    prefixes: &'s [Vec<u8>],
    sheet_name: &'s str,
    array_cells: HashMap<(i32, i32), (i32, i32)>,
    array_cells_claimed: u64,
    // The last row an array formula of this piece reaches.
    spill_end: i32,
    row_buffer: Vec<(i32, Cell)>,
    cells: &'s mut SheetCells,
}

struct CellXml<'a> {
    values: usize,
    value: Option<Cow<'a, str>>,
    inline: Option<String>,
    formulas: usize,
    formula: Option<FormulaXml<'a>>,
}

struct FormulaXml<'a> {
    kind: Option<String>,
    reference: Option<String>,
    si: Option<String>,
    calculate_always: bool,
    text: Option<Cow<'a, str>>,
}

enum CellArray {
    None,
    Dynamic(i32, i32),
    Cse(i32, i32),
}

impl<'a> Scanner<'a, '_> {
    fn rows(&mut self) -> Result<(), ReadError> {
        loop {
            match self.next()? {
                Event::Start(start) if start.local_name().as_ref() == b"row" => {
                    self.row(&start, false)?;
                }
                Event::Empty(start) if start.local_name().as_ref() == b"row" => {
                    self.row(&start, true)?;
                }
                // Blank text between rows, including carriage returns from
                // Windows line endings, is not a cell.
                Event::Text(text) if Self::is_blank(&text) => {}
                Event::Eof => return Ok(()),
                _ => return Err(unsupported("something other than a row in sheetData")),
            }
        }
    }

    fn row(&mut self, start: &BytesStart<'_>, empty: bool) -> Result<(), ReadError> {
        let [r, ht, custom_height, s, custom_format, hidden] = self.attributes(
            start,
            [
                b"r",
                b"ht",
                b"customHeight",
                b"s",
                b"customFormat",
                b"hidden",
            ],
        )?;
        let mut row_index = match r {
            Some(r) => Some(r.parse::<i32>().map_err(unreadable)?),
            None => None,
        };
        if let Some(r) = row_index
            && !is_valid_row(r)
        {
            return Err(unreadable("a row index outside the sheet"));
        }
        let (has_height, height) = match ht {
            Some(ht) => (true, ht.parse::<f64>().unwrap_or(DEFAULT_ROW_HEIGHT)),
            None => (false, DEFAULT_ROW_HEIGHT),
        };
        let custom_height = parse_bool_false(custom_height.as_deref());
        let style = s.map_or(0, |s| s.parse::<i32>().unwrap_or(0));
        let custom_format = parse_bool_false(custom_format.as_deref());
        let hidden = parse_bool_false(hidden.as_deref());
        if let Some(r) = row_index
            && (custom_height || custom_format || style != 0 || has_height || hidden)
        {
            self.cells.rows.push(Row {
                r,
                height,
                s: style,
                custom_height,
                custom_format,
                hidden,
            });
        }
        let mut cells = std::mem::take(&mut self.row_buffer);
        cells.clear();
        if !empty {
            loop {
                match self.next()? {
                    Event::Start(start) if start.local_name().as_ref() == b"c" => {
                        self.cell(&start, false, &mut row_index, &mut cells)?;
                    }
                    Event::Empty(start) if start.local_name().as_ref() == b"c" => {
                        self.cell(&start, true, &mut row_index, &mut cells)?;
                    }
                    Event::End(_) => break,
                    _ => return Err(unsupported("something other than a cell in a row")),
                }
            }
        }
        let row_index = row_index.ok_or_else(|| unreadable("a row without a row index"))?;
        if self.cells.last_row >= row_index {
            return Err(unsupported("rows out of order"));
        }
        self.cells.last_row = row_index;
        self.cells.cell_count += cells.len() as u64;
        self.cells.data.insert_row(row_index, cells.drain(..));
        self.row_buffer = cells;
        Ok(())
    }

    fn cell(
        &mut self,
        start: &BytesStart<'_>,
        empty: bool,
        row_index: &mut Option<i32>,
        cells: &mut Vec<(i32, Cell)>,
    ) -> Result<(), ReadError> {
        let [r, s, t, vm, cm] = self.attributes(start, [b"r", b"s", b"t", b"vm", b"cm"])?;
        let cell_ref = r.ok_or_else(|| unreadable("a cell without a reference"))?;
        let (r_index, column_index) = cell_position(&cell_ref)
            .ok_or_else(|| unreadable(format!("invalid cell reference {cell_ref}")))?;
        if row_index.is_none() {
            *row_index = Some(r_index);
        }
        let stored_row = row_index.unwrap_or(r_index);
        if let Some((last, _)) = cells.last()
            && *last >= column_index
        {
            return Err(unsupported("cells out of order"));
        }
        let content = if empty {
            CellXml {
                values: 0,
                value: None,
                inline: None,
                formulas: 0,
                formula: None,
            }
        } else {
            self.cell_content()?
        };
        let value = if content.values == 1 {
            Some(content.value.unwrap_or(Cow::Borrowed("")))
        } else {
            None
        };
        let cell_type = match t.as_deref() {
            Some(t) => t,
            None if value.is_none() => "empty",
            None => "n",
        };
        let style = s.map_or(0, |s| s.parse::<i32>().unwrap_or(0));

        let mut formula_event = None;
        let mut array = CellArray::None;
        if content.formulas == 1
            && let Some(formula) = content.formula
        {
            let mut kind = formula.kind.as_deref().unwrap_or("normal");
            if kind == "normal" && formula.calculate_always && formula.text.is_none() {
                kind = "hint-volatile";
            }
            let text = formula.text.unwrap_or(Cow::Borrowed(""));
            match kind {
                "shared" => {
                    let si = formula
                        .si
                        .ok_or_else(|| unreadable("a shared formula without si"))?
                        .parse::<i32>()
                        .map_err(unreadable)?;
                    formula_event = Some(match formula.reference {
                        Some(_) => FormulaEvent::SharedAnchor {
                            si,
                            job: self.job(text, &cell_ref, false)?,
                        },
                        None => FormulaEvent::SharedChild { si },
                    });
                }
                "dataTable" => return Err(unreadable("data table formulas")),
                "array" => {
                    let range = formula
                        .reference
                        .ok_or_else(|| unreadable("an array formula without a ref"))?;
                    let (row1, column1, row2, column2) = parse_range(&range)
                        .ok_or_else(|| unreadable(format!("invalid range {range}")))?;
                    if row1 != r_index || column1 != column_index {
                        return Err(unreadable("an array formula outside its anchor"));
                    }
                    if row2 < row1 || column2 < column1 {
                        return Err(unreadable("an array formula range runs backwards"));
                    }
                    let area = (i64::from(row2) - i64::from(row1) + 1)
                        .saturating_mul(i64::from(column2) - i64::from(column1) + 1);
                    if area > limits::MAX_FORMULA_AREA as i64 {
                        return Err(unsupported("an array formula larger than the limit"));
                    }
                    self.array_cells_claimed += area as u64;
                    if self.array_cells_claimed > limits::MAX_ARRAY_CELLS_PER_SHEET {
                        return Err(ReadError::Unsafe(format!(
                            "the array formulas of sheet {} claim more than {} cells",
                            self.sheet_name,
                            limits::MAX_ARRAY_CELLS_PER_SHEET
                        )));
                    }
                    for r in row1..=row2 {
                        for c in column1..=column2 {
                            if r != row1 || c != column1 {
                                self.array_cells.insert((r, c), (r_index, column_index));
                            }
                        }
                    }
                    let (width, height) = (column2 - column1 + 1, row2 - row1 + 1);
                    self.spill_end = self.spill_end.max(stored_row.saturating_add(height - 1));
                    array = if cm.as_deref() == Some("1") {
                        CellArray::Dynamic(width, height)
                    } else {
                        CellArray::Cse(width, height)
                    };
                    formula_event = Some(FormulaEvent::Convert {
                        job: self.job(text, &cell_ref, true)?,
                    });
                }
                "normal" => {
                    formula_event = Some(FormulaEvent::Convert {
                        job: self.job(text, &cell_ref, false)?,
                    });
                }
                "hint-volatile" => {}
                other => return Err(unreadable(format!("invalid formula type {other}"))),
            }
        }
        let anchor = if self.array_cells.is_empty() {
            None
        } else {
            self.array_cells.get(&(r_index, column_index)).copied()
        };
        let place = Slot {
            row: stored_row,
            column: column_index,
        };
        let cell = match formula_event {
            Some(event) => {
                let f = i32::try_from(self.cells.events.len()).map_err(unreadable)?;
                self.cells.events.push(event);
                self.cells.formula_cells.push(place);
                formula_cell(
                    f,
                    style,
                    array,
                    self.formula_value(
                        cell_type,
                        value.as_deref(),
                        vm.as_deref(),
                        &cell_ref,
                        content.inline,
                    ),
                )
            }
            None => self.value_cell(ValueCell {
                cell_type,
                value: value.as_deref(),
                metadata: vm.as_deref(),
                style,
                anchor,
                inline: content.inline,
                place,
            }),
        };
        cells.push((column_index, cell));
        Ok(())
    }

    fn job(&mut self, text: Cow<'_, str>, cell_ref: &str, array: bool) -> Result<usize, ReadError> {
        // The preflight checker used to refuse hostile formulas before the rows were
        // read; with the rows streaming, each formula is checked as it arrives, with
        // the same refusal instead of a fallback.
        limits::check_formula_text(&text).map_err(ReadError::Unsafe)?;
        let (row, column) = formula_context(cell_ref)?;
        self.cells.job_bytes += text.len() as u64;
        self.cells.jobs.push(FormulaJob {
            text: text.into_owned(),
            row,
            column,
            array,
        });
        Ok(self.cells.jobs.len() - 1)
    }

    fn intern(&mut self, text: String) -> i32 {
        let index = match self.cells.string_index.get(text.as_str()) {
            Some(index) => *index,
            None => {
                let next = self.cells.strings.len();
                let shared: Arc<str> = Arc::from(text);
                self.cells.string_bytes += shared.len() as u64 + BYTES_PER_UNIQUE_STRING;
                self.cells.string_index.insert(shared.clone(), next);
                self.cells.strings.push(shared);
                next
            }
        };
        i32::try_from(index).unwrap_or(i32::MAX)
    }

    fn string_cell(&mut self, text: String, s: i32, place: Slot) -> Cell {
        self.cells.string_cells.push(place);
        Cell::SharedString {
            si: self.intern(text),
            s,
        }
    }
}

// IronCalc reads a formula's own cell from "<sheet>!<cell>": the leading letters are the
// column and everything after them the row.
fn formula_context(cell_ref: &str) -> Result<(i32, i32), ReadError> {
    let split = cell_ref
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(cell_ref.len());
    let (column, row) = cell_ref.split_at(split);
    let row = row.parse::<i32>().map_err(unreadable)?;
    let column = column_to_number(column).map_err(unreadable)?;
    Ok((row, column))
}

// IronCalc's `parse_reference_a1`, without its allocations for the plain "B12" form.
// Both spellings are checked against the sheet limits, so no out-of-range row or
// column ever reaches the model.
fn cell_position(cell_ref: &str) -> Option<(i32, i32)> {
    let bytes = cell_ref.as_bytes();
    let letters = bytes.iter().take_while(|b| b.is_ascii_uppercase()).count();
    let digits = &bytes[letters..];
    if (1..=3).contains(&letters) && !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) {
        let column = bytes[..letters]
            .iter()
            .fold(0i32, |n, b| n * 26 + i32::from(b - b'A' + 1));
        let row = cell_ref.get(letters..)?.parse::<i32>().ok()?;
        if is_valid_column_number(column) && is_valid_row(row) {
            return Some((row, column));
        }
        return None;
    }
    parse_reference_a1(cell_ref)
        .filter(|r| is_valid_row(r.row) && is_valid_column_number(r.column))
        .map(|r| (r.row, r.column))
}

fn parse_range(range: &str) -> Option<(i32, i32, i32, i32)> {
    let parts: Vec<&str> = range.split(':').collect();
    match parts.as_slice() {
        [single] => parse_reference_a1(single).map(|r| (r.row, r.column, r.row, r.column)),
        [left, right] => {
            let (left, right) = (parse_reference_a1(left)?, parse_reference_a1(right)?);
            Some((left.row, left.column, right.row, right.column))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(rows: &str) -> Result<SheetCells, ReadError> {
        scan_rows(rows, &[Vec::new()], "Sheet1")
    }

    fn array_row(row: u32, reference: &str) -> String {
        format!(r#"<row r="{row}"><c r="A{row}"><f t="array" ref="{reference}">1</f></c></row>"#)
    }

    #[test]
    fn each_distinct_string_is_charged_once_beyond_its_text() {
        let rows = r#"<row r="1"><c r="A1" t="inlineStr"><is><t>ab</t></is></c><c r="B1" t="inlineStr"><is><t>ab</t></is></c><c r="C1" t="inlineStr"><is><t>cde</t></is></c></row>"#;
        let cells = scan(rows).unwrap();
        assert_eq!(cells.totals().bytes, 2 + 3 + 2 * BYTES_PER_UNIQUE_STRING);
    }

    #[test]
    fn many_array_formulas_together_are_refused() {
        let rows: String = (1..=5)
            .map(|n| array_row(n, &format!("A{n}:A{}", n + 900_000)))
            .collect();
        assert!(matches!(scan(&rows), Err(ReadError::Unsafe(_))));
    }

    #[test]
    fn an_array_range_running_backwards_is_left_to_ironcalc() {
        assert!(matches!(
            scan(&array_row(5, "A5:A1")),
            Err(ReadError::Unreadable(_))
        ));
    }

    #[test]
    fn a_few_array_formulas_are_read() {
        let rows: String = (1..=3)
            .map(|n| array_row(n, &format!("A{n}:B{n}")))
            .collect();
        assert!(scan(&rows).is_ok());
    }

    #[test]
    fn a_shared_formula_covering_a_whole_column_is_read() {
        let rows = r#"<row r="1"><c r="A1"><f t="shared" ref="A1:A1048576" si="0">B1*2</f><v>0</v></c></row>"#;
        assert!(scan(rows).is_ok());
    }

    #[test]
    fn blank_text_between_rows_is_ignored() {
        let rows = "<row r=\"1\"><c r=\"A1\"><v>1</v></c></row>\n  \r\n <row r=\"2\"><c r=\"A2\"><v>2</v></c></row>\r\n";
        let cells = scan(rows).unwrap();
        assert_eq!(cells.data.row_count(), 2);
    }

    #[test]
    fn row_indexes_outside_the_sheet_are_left_to_ironcalc() {
        for rows in [
            r#"<row r="0"><c r="A1"><v>1</v></c></row>"#,
            r#"<row r="1048577"><c r="A1"><v>1</v></c></row>"#,
            r#"<row r="2147483647"><c r="A1"><v>1</v></c></row>"#,
        ] {
            assert!(
                matches!(scan(rows), Err(ReadError::Unreadable(_))),
                "{rows}"
            );
        }
    }

    #[test]
    fn cell_references_outside_the_sheet_are_left_to_ironcalc() {
        for rows in [
            r#"<row r="1"><c r="A1048577"><v>1</v></c></row>"#,
            r#"<row r="1"><c r="XFE1"><v>1</v></c></row>"#,
            r#"<row r="1"><c r="A1"><f t="array" ref="A1:B1048577">1</f></c></row>"#,
        ] {
            assert!(
                matches!(scan(rows), Err(ReadError::Unreadable(_))),
                "{rows}"
            );
        }
    }
}
