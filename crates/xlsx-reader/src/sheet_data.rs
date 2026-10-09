use std::borrow::Cow;
use std::collections::HashMap;

use ironcalc::base::expressions::utils::{
    column_to_number, is_valid_column_number, is_valid_row, parse_reference_a1,
};
use ironcalc::base::types::{Cell, Row};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

mod values;
mod xml;

use crate::ReadError;
use crate::limits;
use crate::text::parse_bool_false;
use values::formula_cell;

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

// Cells in document order. Cells listed in `string_cells` hold a sheet-local string index
// and every formula cell holds an index into `events`; both are resolved once all sheets
// are read, because IronCalc numbers strings and formulas in document order.
#[derive(Default)]
pub(crate) struct SheetCells<'a> {
    pub(crate) rows: Vec<Row>,
    pub(crate) data: Vec<(i32, Vec<(i32, Cell)>)>,
    pub(crate) strings: Vec<String>,
    pub(crate) string_cells: Vec<Slot>,
    pub(crate) events: Vec<FormulaEvent>,
    pub(crate) jobs: Vec<FormulaJob<'a>>,
    pub(crate) has_arrays: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct Slot {
    pub(crate) row: usize,
    pub(crate) cell: usize,
}

pub(crate) enum FormulaEvent {
    Convert { job: usize },
    SharedAnchor { si: i32, job: usize },
    SharedChild { si: i32 },
}

impl FormulaEvent {
    pub(crate) fn after_jobs(self, offset: usize) -> FormulaEvent {
        match self {
            FormulaEvent::Convert { job } => FormulaEvent::Convert { job: job + offset },
            FormulaEvent::SharedAnchor { si, job } => FormulaEvent::SharedAnchor {
                si,
                job: job + offset,
            },
            child @ FormulaEvent::SharedChild { .. } => child,
        }
    }
}

pub(crate) struct FormulaJob<'a> {
    pub(crate) text: Cow<'a, str>,
    pub(crate) row: i32,
    pub(crate) column: i32,
    pub(crate) array: bool,
}

fn unreadable(error: impl ToString) -> ReadError {
    ReadError::Unreadable(error.to_string())
}

fn unsupported(what: &str) -> ReadError {
    ReadError::Unsupported(what.to_string())
}

// Reads a run of whole <row> elements, the content of <sheetData> or a piece of it.
pub(crate) fn scan_rows<'a>(
    rows: &'a str,
    prefixes: &[Vec<u8>],
    sheet_name: &str,
) -> Result<SheetCells<'a>, ReadError> {
    let mut reader = Reader::from_str(rows);
    let config = reader.config_mut();
    config.check_end_names = true;
    config.check_comments = true;
    let mut scanner = Scanner {
        reader,
        prefixes,
        sheet_name,
        strings: HashMap::new(),
        array_cells: HashMap::new(),
        cells: SheetCells::default(),
    };
    scanner.rows()?;
    let mut cells = scanner.cells;
    let mut strings: Vec<(String, usize)> = scanner.strings.into_iter().collect();
    strings.sort_unstable_by_key(|(_, index)| *index);
    cells.strings = strings.into_iter().map(|(text, _)| text).collect();
    cells.has_arrays = !scanner.array_cells.is_empty();
    Ok(cells)
}

struct Scanner<'a, 's> {
    reader: Reader<&'a [u8]>,
    prefixes: &'s [Vec<u8>],
    sheet_name: &'s str,
    strings: HashMap<String, usize>,
    array_cells: HashMap<(i32, i32), (i32, i32)>,
    cells: SheetCells<'a>,
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
        let slot = self.cells.data.len();
        let mut cells: Vec<(i32, Cell)> = Vec::new();
        if !empty {
            loop {
                match self.next()? {
                    Event::Start(start) if start.local_name().as_ref() == b"c" => {
                        self.cell(&start, false, slot, &mut row_index, &mut cells)?;
                    }
                    Event::Empty(start) if start.local_name().as_ref() == b"c" => {
                        self.cell(&start, true, slot, &mut row_index, &mut cells)?;
                    }
                    Event::End(_) => break,
                    _ => return Err(unsupported("something other than a cell in a row")),
                }
            }
        }
        let row_index = row_index.ok_or_else(|| unreadable("a row without a row index"))?;
        if let Some((last, _)) = self.cells.data.last()
            && *last >= row_index
        {
            return Err(unsupported("rows out of order"));
        }
        self.cells.data.push((row_index, cells));
        Ok(())
    }

    fn cell(
        &mut self,
        start: &BytesStart<'_>,
        empty: bool,
        slot: usize,
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
                    let area = (i64::from(row2) - i64::from(row1) + 1)
                        .saturating_mul(i64::from(column2) - i64::from(column1) + 1);
                    if area > limits::MAX_FORMULA_AREA as i64 {
                        return Err(unsupported("an array formula larger than the limit"));
                    }
                    for r in row1..=row2 {
                        for c in column1..=column2 {
                            if r != row1 || c != column1 {
                                self.array_cells.insert((r, c), (r_index, column_index));
                            }
                        }
                    }
                    let (width, height) = (column2 - column1 + 1, row2 - row1 + 1);
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
            row: slot,
            cell: cells.len(),
        };
        let cell = match formula_event {
            Some(event) => {
                let f = i32::try_from(self.cells.events.len()).map_err(unreadable)?;
                self.cells.events.push(event);
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
            None => self.value_cell(
                cell_type,
                value.as_deref(),
                vm.as_deref(),
                style,
                anchor,
                content.inline,
                place,
            ),
        };
        cells.push((column_index, cell));
        Ok(())
    }

    fn job(&mut self, text: Cow<'a, str>, cell_ref: &str, array: bool) -> Result<usize, ReadError> {
        let (row, column) = formula_context(cell_ref)?;
        self.cells.jobs.push(FormulaJob {
            text,
            row,
            column,
            array,
        });
        Ok(self.cells.jobs.len() - 1)
    }

    fn intern(&mut self, text: String) -> i32 {
        let next = self.strings.len();
        let index = *self.strings.entry(text).or_insert(next);
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
    parse_reference_a1(cell_ref).map(|r| (r.row, r.column))
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
