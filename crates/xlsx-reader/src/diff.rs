use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Debug;

use ironcalc::base::expressions::utils::number_to_column;
use ironcalc::base::types::{Workbook, Worksheet};

// Every field of two workbooks compared, values by their Debug text so -0.0 and NaN count
// as different, maps in key order. One line per difference, at most `limit` lines.
pub fn workbook_differences(expected: &Workbook, actual: &Workbook, limit: usize) -> Vec<String> {
    let mut found = Differences {
        lines: Vec::new(),
        limit,
        total: 0,
    };
    found.list(
        "shared string",
        &expected.shared_strings,
        &actual.shared_strings,
    );
    found.list(
        "defined name",
        &expected.defined_names,
        &actual.defined_names,
    );
    found.value("styles", &expected.styles, &actual.styles);
    found.value("name", &expected.name, &actual.name);
    found.value("settings", &expected.settings, &actual.settings);
    found.value("metadata", &expected.metadata, &actual.metadata);
    found.map("table", &expected.tables, &actual.tables);
    found.map("workbook view", &expected.views, &actual.views);
    found.value("theme", &expected.theme, &actual.theme);
    found.value(
        "sheet count",
        &expected.worksheets.len(),
        &actual.worksheets.len(),
    );
    for (expected, actual) in expected.worksheets.iter().zip(&actual.worksheets) {
        found.worksheet(expected, actual);
    }
    if found.total > found.lines.len() {
        found
            .lines
            .push(format!("... {} differences in all", found.total));
    }
    found.lines
}

struct Differences {
    lines: Vec<String>,
    limit: usize,
    total: usize,
}

impl Differences {
    fn report(&mut self, line: impl FnOnce() -> String) {
        self.total += 1;
        if self.lines.len() < self.limit {
            self.lines.push(line());
        }
    }

    fn value<T: Debug + ?Sized>(&mut self, what: &str, expected: &T, actual: &T) {
        let (expected, actual) = (format!("{expected:?}"), format!("{actual:?}"));
        if expected != actual {
            self.report(|| format!("{what}: expected {expected}, got {actual}"));
        }
    }

    fn list<T: Debug>(&mut self, what: &str, expected: &[T], actual: &[T]) {
        if expected.len() != actual.len() {
            self.report(|| {
                format!(
                    "{what} count: expected {}, got {}",
                    expected.len(),
                    actual.len()
                )
            });
        }
        for (index, (expected, actual)) in expected.iter().zip(actual).enumerate() {
            self.value(&format!("{what} {index}"), expected, actual);
        }
    }

    fn map<K: Ord + Debug + std::hash::Hash, V: Debug>(
        &mut self,
        what: &str,
        expected: &HashMap<K, V>,
        actual: &HashMap<K, V>,
    ) {
        let expected: BTreeMap<&K, &V> = expected.iter().collect();
        let actual: BTreeMap<&K, &V> = actual.iter().collect();
        let keys: BTreeSet<&&K> = expected.keys().chain(actual.keys()).collect();
        for key in keys {
            self.value(
                &format!("{what} {key:?}"),
                &expected.get(*key),
                &actual.get(*key),
            );
        }
    }

    fn worksheet(&mut self, expected: &Worksheet, actual: &Worksheet) {
        let sheet = &expected.name;
        let field = |name: &str| format!("{sheet}: {name}");
        self.value(&field("name"), &expected.name, &actual.name);
        self.value(&field("dimension"), &expected.dimension, &actual.dimension);
        self.list(&field("column"), &expected.cols, &actual.cols);
        self.list(&field("row"), &expected.rows, &actual.rows);
        self.list(
            &field("formula"),
            &expected.shared_formulas,
            &actual.shared_formulas,
        );
        self.value(&field("sheet id"), &expected.sheet_id, &actual.sheet_id);
        self.value(&field("state"), &expected.state, &actual.state);
        self.value(&field("color"), &expected.color, &actual.color);
        self.list(&field("merge"), &expected.merge_cells, &actual.merge_cells);
        self.list(&field("comment"), &expected.comments, &actual.comments);
        self.value(
            &field("frozen rows"),
            &expected.frozen_rows,
            &actual.frozen_rows,
        );
        self.value(
            &field("frozen columns"),
            &expected.frozen_columns,
            &actual.frozen_columns,
        );
        self.map(&field("view"), &expected.views, &actual.views);
        self.value(
            &field("grid lines"),
            &expected.show_grid_lines,
            &actual.show_grid_lines,
        );
        self.list(
            &field("conditional format"),
            &expected.conditional_formatting,
            &actual.conditional_formatting,
        );
        let rows: BTreeSet<&i32> = expected
            .sheet_data
            .keys()
            .chain(actual.sheet_data.keys())
            .collect();
        for row in rows {
            let (expected_row, actual_row) =
                (expected.sheet_data.get(row), actual.sheet_data.get(row));
            let columns: BTreeSet<&i32> = expected_row
                .into_iter()
                .chain(actual_row)
                .flat_map(|cells| cells.keys())
                .collect();
            if columns.is_empty() && expected_row.is_some() != actual_row.is_some() {
                self.report(|| format!("{sheet}: empty row {row} present in only one"));
            }
            for column in columns {
                let name = number_to_column(*column).unwrap_or_else(|| column.to_string());
                self.value(
                    &format!("{sheet}!{name}{row}"),
                    &expected_row.and_then(|cells| cells.get(column)),
                    &actual_row.and_then(|cells| cells.get(column)),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ironcalc::base::types::Cell;

    use super::*;

    fn fixture() -> Workbook {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/compat/merges-freeze-sizes.xlsx"
        );
        let bytes = std::fs::read(path).unwrap();
        ironcalc::import::load_from_xlsx_bytes(&bytes, "b", "en", "UTC").unwrap()
    }

    #[test]
    fn a_workbook_equals_itself() {
        let book = fixture();
        assert!(workbook_differences(&book, &book.clone(), 10).is_empty());
    }

    #[test]
    fn every_kind_of_change_is_reported() {
        let book = fixture();
        let mut changed = book.clone();
        changed.shared_strings.push("extra".to_string());
        changed.worksheets[0].frozen_rows += 1;
        changed.worksheets[0]
            .shared_formulas
            .push("R[1]C".to_string());
        changed.worksheets[0]
            .sheet_data
            .entry(7)
            .or_default()
            .insert(3, Cell::NumberCell { v: -0.0, s: 0 });
        changed.styles.cell_xfs[0].num_fmt_id += 1;
        let found = workbook_differences(&book, &changed, 10).join("\n");
        for expected in [
            "shared string count",
            "frozen rows",
            "formula count",
            "!C7",
            "styles",
        ] {
            assert!(
                found.contains(expected),
                "{expected} missing from:\n{found}"
            );
        }
        let mut negative_zero = changed.clone();
        negative_zero.worksheets[0]
            .sheet_data
            .entry(7)
            .or_default()
            .insert(3, Cell::NumberCell { v: 0.0, s: 0 });
        assert_eq!(workbook_differences(&changed, &negative_zero, 10).len(), 1);
        assert_eq!(workbook_differences(&book, &changed, 2).len(), 3);
    }
}
