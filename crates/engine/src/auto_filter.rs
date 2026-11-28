// A sheet's AutoFilter lives in the model (`ironcalc::base::types::AutoFilter`), with the
// rows it hides listed apart from rows hidden by hand, which no filter command shows.

use std::collections::{BTreeMap, HashMap, HashSet};

use ironcalc::base::Model;
use ironcalc::base::cell::CellValue;
use ironcalc::base::types::{
    AutoFilter, CellType, FilterColumn, FilterCriteria, SortCondition, SortState,
};
use zenkai_types::{
    CellPos, ColIdx, ColumnCriteria, FilterItem, FilterList, FilterView, MAX_FILTER_ITEMS, Range,
    RowIdx, SheetId, ShownValues,
};

use crate::error::EngineError;
use crate::shown_cells::UNREADABLE_CELL;
use crate::workbook::{Engine, Workbook, col_i32, row_i32};

fn rejected(message: &str) -> EngineError {
    EngineError::Rejected(message.to_string())
}

fn model_error(message: String) -> EngineError {
    EngineError::Rejected(message)
}

fn row_index(row: i32) -> RowIdx {
    RowIdx::clamped(i64::from(row) - 1)
}

fn col_index(column: i32) -> ColIdx {
    ColIdx::clamped(i64::from(column) - 1)
}

// The hidden rows, read as IronCalc reads them: the first entry of a row counts.
fn hidden_rows(model: &Model, sheet: SheetId) -> Result<HashSet<i32>, EngineError> {
    let worksheet = model
        .workbook
        .worksheet(sheet.0)
        .map_err(|_| EngineError::UnknownSheet(sheet))?;
    let mut seen = HashSet::with_capacity(worksheet.rows.len());
    let mut hidden = HashSet::new();
    for row in &worksheet.rows {
        if seen.insert(row.r) && row.hidden {
            hidden.insert(row.r);
        }
    }
    Ok(hidden)
}

// Excel's order for a filter list, the order of an ascending sort.
#[derive(Clone, Debug)]
struct ItemOrder {
    rank: u8,
    number: f64,
    text: String,
}

impl PartialEq for ItemOrder {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for ItemOrder {}

impl PartialOrd for ItemOrder {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ItemOrder {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank
            .cmp(&other.rank)
            .then(self.number.total_cmp(&other.number))
            .then_with(|| self.text.cmp(&other.text))
    }
}

// What a filter reads of a cell: the text it shows, which Excel's criteria match ignoring
// case, and where it goes in the list.
struct Shown {
    text: String,
    order: ItemOrder,
}

// Read as the grid reads a cell, so the list shows what the cells show.
fn shown(book: &Workbook, sheet: SheetId, row: i32, column: i32) -> Shown {
    let model = book.model.get_model();
    let readable = book.formatted_text(sheet, CellPos::new(row_index(row), col_index(column)));
    let is_error = readable.is_none()
        || model
            .workbook
            .worksheet(sheet.0)
            .ok()
            .and_then(|worksheet| worksheet.cell(row, column))
            .is_some_and(|cell| matches!(cell.get_type(), CellType::ErrorValue));
    let text = readable.unwrap_or_else(|| UNREADABLE_CELL.to_string());
    let (rank, number) = match model.get_cell_value_by_index(sheet.0, row, column) {
        _ if is_error => (3, 0.0),
        Ok(CellValue::Number(number)) => (0, number),
        Ok(CellValue::Boolean(value)) => (2, f64::from(u8::from(value))),
        _ => (1, 0.0),
    };
    let lower = text.to_lowercase();
    Shown {
        order: ItemOrder {
            rank,
            number,
            text: lower,
        },
        text,
    }
}

// The criteria of a filter's columns, ready to test rows against.
enum ColumnTest {
    Values {
        values: HashSet<String>,
        blank: bool,
    },
    // A rule kept from the file: a row passes it while the filter does not hide it.
    Kept,
}

struct RowCriteria {
    columns: Vec<(i32, ColumnTest)>,
}

impl RowCriteria {
    fn of(filter: &AutoFilter, skipped: Option<i32>) -> RowCriteria {
        let columns = filter
            .columns
            .iter()
            .filter(|column| Some(column.column) != skipped)
            .filter_map(|column| {
                let test = match &column.criteria {
                    FilterCriteria::None => return None,
                    FilterCriteria::Values { values, blank } => ColumnTest::Values {
                        values: values.iter().map(|v| v.to_lowercase()).collect(),
                        blank: *blank,
                    },
                    FilterCriteria::Kept(_) => ColumnTest::Kept,
                };
                Some((filter.first_column + column.column, test))
            })
            .collect();
        RowCriteria { columns }
    }

    // `hidden_now` is whether the filter hides the row before this test.
    fn passes(&self, book: &Workbook, sheet: SheetId, row: i32, hidden_now: bool) -> bool {
        self.columns.iter().all(|(column, test)| match test {
            ColumnTest::Kept => !hidden_now,
            ColumnTest::Values { values, blank } => {
                let text = shown(book, sheet, row, *column).order.text;
                if text.is_empty() {
                    *blank
                } else {
                    values.contains(&text)
                }
            }
        })
    }
}

fn criteria_view(criteria: &FilterCriteria) -> Option<ColumnCriteria> {
    match criteria {
        FilterCriteria::None => None,
        FilterCriteria::Values { values, blank } => Some(ColumnCriteria::Values(ShownValues {
            values: values.clone(),
            blanks: *blank,
        })),
        FilterCriteria::Kept(_) => Some(ColumnCriteria::Custom),
    }
}

// Sorted rows as runs of consecutive rows.
fn runs(rows: impl IntoIterator<Item = i32>) -> Vec<(i32, i32)> {
    let mut runs: Vec<(i32, i32)> = Vec::new();
    for row in rows {
        match runs.last_mut() {
            Some((_, last)) if *last + 1 == row => *last = row,
            _ => runs.push((row, row)),
        }
    }
    runs
}

impl Workbook {
    pub(crate) fn sheet_filter(&self, sheet: SheetId) -> Result<Option<AutoFilter>, EngineError> {
        self.model
            .get_auto_filter(sheet.0)
            .map(Option::<&AutoFilter>::cloned)
            .map_err(|_| EngineError::UnknownSheet(sheet))
    }

    fn existing_filter(&self, sheet: SheetId) -> Result<AutoFilter, EngineError> {
        self.sheet_filter(sheet)?
            .ok_or_else(|| rejected("the sheet has no filter"))
    }

    // Writes `filter` and the hidden flags of `rows` as one undo step.
    fn write_filter(
        &mut self,
        sheet: SheetId,
        filter: Option<AutoFilter>,
        rows: &[(i32, bool)],
    ) -> Result<(), EngineError> {
        let written = self.model.edit_keeping_cells(|model| {
            model.begin_edit_group()?;
            let written = model
                .set_auto_filter(sheet.0, filter)
                .and_then(|()| model.set_scattered_rows_hidden(sheet.0, rows));
            let ended = model.end_edit_group();
            written.and(ended)
        });
        written
            .map_err(model_error)
            .inspect(|()| self.model.count_style_edit())
    }

    // Hides the data rows `filter`'s criteria reject and shows the others the filter hid,
    // leaving rows hidden by hand hidden, and lists the rows it hides in the filter.
    fn refilter(&mut self, sheet: SheetId, mut filter: AutoFilter) -> Result<(), EngineError> {
        let hidden = hidden_rows(self.model.get_model(), sheet)?;
        let tests = RowCriteria::of(&filter, None);
        let mut changes = Vec::new();
        let mut filtered_rows = Vec::new();
        for row in filter.data_rows() {
            let hidden_now = hidden.contains(&row);
            let by_filter = hidden_now && filter.is_filtered(row);
            let by_hand = hidden_now && !by_filter;
            let fails = !tests.passes(self, sheet, row, by_filter);
            if fails && !by_hand {
                filtered_rows.push(row);
            }
            if (by_hand || fails) != hidden_now {
                changes.push((row, by_hand || fails));
            }
        }
        filter.filtered_rows = filtered_rows;
        self.write_filter(sheet, Some(filter), &changes)
    }

    // The rows of `range` an edit over it skips: in a filter that hides rows, as in Excel,
    // every hidden row of its data. 0-based.
    pub(crate) fn skipped_rows(
        &self,
        sheet: SheetId,
        range: Range,
    ) -> Result<HashSet<u32>, EngineError> {
        let Some(filter) = self.sheet_filter(sheet)? else {
            return Ok(HashSet::new());
        };
        if filter.filtered_rows.is_empty() {
            return Ok(HashSet::new());
        }
        let hidden = hidden_rows(self.model.get_model(), sheet)?;
        let first = (*filter.data_rows().start()).max(row_i32(range.start.row));
        let last = (*filter.data_rows().end()).min(row_i32(range.end.row));
        Ok((first..=last)
            .filter(|row| hidden.contains(row))
            .filter_map(|row| u32::try_from(row - 1).ok())
            .collect())
    }

    // Excel refuses to paste several rows over rows a filter hides: they would land in the
    // hidden rows too.
    pub(crate) fn check_paste_rows(
        &self,
        sheet: SheetId,
        target: Range,
    ) -> Result<(), EngineError> {
        if target.rows() > 1 && !self.skipped_rows(sheet, target)?.is_empty() {
            return Err(EngineError::FilteredPaste);
        }
        Ok(())
    }

    pub(crate) fn filter_view(&self, sheet: SheetId) -> Option<FilterView> {
        let filter = self.sheet_filter(sheet).ok()??;
        let hidden = hidden_rows(self.model.get_model(), sheet).ok()?;
        let data_rows = filter.data_rows();
        let total = u32::try_from(data_rows.clone().count()).unwrap_or(0);
        let hidden_data = hidden.iter().filter(|row| data_rows.contains(*row)).count();
        let filtered_out = runs(
            filter
                .filtered_rows
                .iter()
                .copied()
                .filter(|row| hidden.contains(row)),
        );
        let column = |offset: i32| col_index(filter.first_column + offset);
        Some(FilterView {
            range: Range::new(
                CellPos::new(row_index(filter.first_row), col_index(filter.first_column)),
                CellPos::new(row_index(filter.last_row), col_index(filter.last_column)),
            ),
            criteria: filter
                .columns
                .iter()
                .filter_map(|c| Some((column(c.column), criteria_view(&c.criteria)?)))
                .collect(),
            without_button: filter
                .columns
                .iter()
                .filter(|c| c.hidden_button || !c.show_button)
                .map(|c| column(c.column))
                .collect(),
            filtered_out: filtered_out
                .into_iter()
                .map(|(first, last)| (row_index(first), row_index(last)))
                .collect(),
            found: total.saturating_sub(u32::try_from(hidden_data).unwrap_or(u32::MAX)),
            total,
        })
    }

    // Turning the filter off shows every row it hid; turning it on hides nothing.
    pub(crate) fn toggle_filter(
        &mut self,
        sheet: SheetId,
        range: Range,
    ) -> Result<(), EngineError> {
        if let Some(filter) = self.sheet_filter(sheet)? {
            let hidden = hidden_rows(self.model.get_model(), sheet)?;
            let shown: Vec<(i32, bool)> = filter
                .filtered_rows
                .iter()
                .filter(|row| hidden.contains(*row))
                .map(|row| (*row, false))
                .collect();
            return self.write_filter(sheet, None, &shown);
        }
        let filter = AutoFilter {
            first_row: row_i32(range.start.row),
            first_column: col_i32(range.start.col),
            last_row: row_i32(range.end.row),
            last_column: col_i32(range.end.col),
            columns: Vec::new(),
            filtered_rows: Vec::new(),
            sort_state: None,
        };
        self.write_filter(sheet, Some(filter), &[])
    }

    fn column_offset(filter: &AutoFilter, col: ColIdx) -> Result<i32, EngineError> {
        let column = col_i32(col);
        if !(filter.first_column..=filter.last_column).contains(&column) {
            return Err(rejected("the column is outside the filter"));
        }
        Ok(column - filter.first_column)
    }

    pub(crate) fn set_column_criteria(
        &mut self,
        sheet: SheetId,
        col: ColIdx,
        shown: Option<ShownValues>,
    ) -> Result<(), EngineError> {
        let mut filter = self.existing_filter(sheet)?;
        let offset = Workbook::column_offset(&filter, col)?;
        let criteria = match shown {
            Some(shown) => FilterCriteria::Values {
                values: shown.values,
                blank: shown.blanks,
            },
            None => FilterCriteria::None,
        };
        match filter.columns.iter_mut().find(|c| c.column == offset) {
            Some(column) => column.criteria = criteria,
            None => {
                filter.columns.push(FilterColumn {
                    column: offset,
                    criteria,
                    hidden_button: false,
                    show_button: true,
                });
                filter.columns.sort_by_key(|c| c.column);
            }
        }
        filter
            .columns
            .retain(|c| c.criteria != FilterCriteria::None || c.hidden_button || !c.show_button);
        self.refilter(sheet, filter)
    }

    // Excel's Clear: every criterion goes, the buttons stay.
    pub(crate) fn clear_criteria(&mut self, sheet: SheetId) -> Result<(), EngineError> {
        let mut filter = self.existing_filter(sheet)?;
        for column in &mut filter.columns {
            column.criteria = FilterCriteria::None;
        }
        filter.columns.retain(|c| c.hidden_button || !c.show_button);
        self.refilter(sheet, filter)
    }

    // The column's distinct shown texts, merged ignoring case, from the data rows the other
    // columns' criteria let through, those containing `search` (ignoring case) only. Only
    // the first MAX_FILTER_ITEMS in Excel's order are kept, so the memory stays bounded
    // however many values the column holds.
    pub(crate) fn filter_list(
        &self,
        sheet: SheetId,
        col: ColIdx,
        search: &str,
    ) -> Result<FilterList, EngineError> {
        let filter = self.existing_filter(sheet)?;
        let offset = Workbook::column_offset(&filter, col)?;
        let criteria = filter
            .columns
            .iter()
            .find(|c| c.column == offset)
            .map_or(FilterCriteria::None, |c| c.criteria.clone());
        if matches!(criteria, FilterCriteria::Kept(_)) {
            return Ok(FilterList {
                custom: true,
                ..FilterList::default()
            });
        }
        let hidden = hidden_rows(self.model.get_model(), sheet)?;
        let others = RowCriteria::of(&filter, Some(offset));
        let search = search.to_lowercase();
        let column = filter.first_column + offset;
        let mut items: BTreeMap<ItemOrder, (String, u32)> = BTreeMap::new();
        let mut by_text: HashMap<String, ItemOrder> = HashMap::new();
        let mut blanks = 0u32;
        let mut truncated = false;
        for row in filter.data_rows() {
            let by_filter = hidden.contains(&row) && filter.is_filtered(row);
            if !others.passes(self, sheet, row, by_filter) {
                continue;
            }
            let Shown { text, order } = shown(self, sheet, row, column);
            if text.is_empty() {
                blanks = blanks.saturating_add(1);
                continue;
            }
            if !order.text.contains(&search) {
                continue;
            }
            if let Some(known) = by_text.get(&order.text) {
                if let Some((_, count)) = items.get_mut(known) {
                    *count = count.saturating_add(1);
                }
                continue;
            }
            by_text.insert(order.text.clone(), order.clone());
            items.insert(order, (text, 1));
            if items.len() > MAX_FILTER_ITEMS {
                truncated = true;
                if let Some((last, _)) = items.pop_last() {
                    by_text.remove(&last.text);
                }
            }
        }
        let (shown_values, blank_shown) = match &criteria {
            FilterCriteria::Values { values, blank } => (
                Some(
                    values
                        .iter()
                        .map(|v| v.to_lowercase())
                        .collect::<HashSet<_>>(),
                ),
                *blank,
            ),
            _ => (None, true),
        };
        let shows = |lower: &str| {
            shown_values
                .as_ref()
                .is_none_or(|values| values.contains(lower))
        };
        Ok(FilterList {
            items: items
                .into_iter()
                .map(|(order, (text, count))| FilterItem {
                    shown: shows(&order.text),
                    text,
                    count,
                })
                .collect(),
            blanks: (blanks > 0 && search.is_empty()).then_some(FilterItem {
                text: String::new(),
                count: blanks,
                shown: blank_shown,
            }),
            truncated,
            custom: false,
        })
    }

    // Excel sorts only the visible data rows of a filter; the rows it hides stay where they
    // are. The sort is remembered in the filter, as Excel does for Reapply.
    pub(crate) fn sort_visible_rows(
        &mut self,
        sheet: SheetId,
        col: ColIdx,
        descending: bool,
    ) -> Result<(), EngineError> {
        let filter = self.existing_filter(sheet)?;
        Workbook::column_offset(&filter, col)?;
        let hidden = hidden_rows(self.model.get_model(), sheet)?;
        let visible: Vec<i32> = filter
            .data_rows()
            .filter(|row| !hidden.contains(row))
            .collect();
        let blocks: Vec<Range> = runs(visible.iter().copied())
            .into_iter()
            .map(|(first, last)| {
                Range::new(
                    CellPos::new(row_index(first), col_index(filter.first_column)),
                    CellPos::new(row_index(last), col_index(filter.last_column)),
                )
            })
            .collect();
        if visible.is_empty() {
            return Ok(());
        }
        let sorted = Range::new(
            CellPos::new(
                row_index(filter.first_row + 1),
                col_index(filter.first_column),
            ),
            CellPos::new(row_index(filter.last_row), col_index(filter.last_column)),
        );
        let rows: Vec<RowIdx> = visible.iter().map(|row| row_index(*row)).collect();
        let moved = self.sorted_rows(sheet, sorted, &rows, col, descending)?;
        let mut remembered = filter.clone();
        remembered.sort_state = Some(SortState {
            first_row: filter.first_row + 1,
            first_column: filter.first_column,
            last_row: filter.last_row,
            last_column: filter.last_column,
            conditions: vec![SortCondition {
                first_row: filter.first_row + 1,
                first_column: col_i32(col),
                last_row: filter.last_row,
                last_column: col_i32(col),
                descending,
                attributes: String::new(),
            }],
            attributes: String::new(),
        });
        self.as_one_undo_step(|book| {
            if let Some(moved) = &moved {
                let mut rows = moved.iter();
                for block in &blocks {
                    let texts: Vec<Vec<String>> =
                        rows.by_ref().take(block.rows() as usize).cloned().collect();
                    book.set_inputs(sheet, block.start, &texts)?;
                }
                book.model.count_moved_edit();
            }
            book.model
                .edit_keeping_cells(|model| model.set_auto_filter(sheet.0, Some(remembered)))
                .map_err(model_error)
        })
    }
}

// Each fill line cut around `skipped` rows (0-based), so a fill touches only the rows
// left.
pub(crate) fn lines_around(
    lines: Vec<(Option<CellPos>, Range)>,
    skipped: &HashSet<u32>,
) -> Vec<(Option<CellPos>, Range)> {
    if skipped.is_empty() {
        return lines;
    }
    lines
        .into_iter()
        .flat_map(|(source, line)| {
            let rows = (line.start.row.get()..=line.end.row.get())
                .filter(|row| !skipped.contains(row))
                .filter_map(|row| i32::try_from(row).ok());
            runs(rows).into_iter().map(move |(first, last)| {
                let row = |index: i32| RowIdx::clamped(i64::from(index));
                (
                    source,
                    Range::new(
                        CellPos::new(row(first), line.start.col),
                        CellPos::new(row(last), line.end.col),
                    ),
                )
            })
        })
        .collect()
}

// The rows of `range` outside `skipped` (0-based), as runs of first and last.
pub(crate) fn rows_around(range: Range, skipped: &HashSet<u32>) -> Vec<(RowIdx, RowIdx)> {
    let rows = (range.start.row.get()..=range.end.row.get())
        .filter(|row| !skipped.contains(row))
        .filter_map(|row| i32::try_from(row).ok());
    runs(rows)
        .into_iter()
        .map(|(first, last)| {
            (
                RowIdx::clamped(i64::from(first)),
                RowIdx::clamped(i64::from(last)),
            )
        })
        .collect()
}
