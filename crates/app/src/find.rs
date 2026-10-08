use gpui_kit::component::input::InputState;
use gpui_kit::*;
use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};

pub const MAX_MATCHES: usize = 100_000;

pub struct FindBar {
    pub input: Entity<InputState>,
    pub results: FindResults,
}

#[derive(Default)]
pub struct FindResults {
    pub query: String,
    pub matches: Vec<CellPos>,
    pub current: usize,
}

impl FindResults {
    pub fn status(&self) -> String {
        match (self.query.is_empty(), self.matches.len()) {
            (true, _) => "Type and press Enter".to_string(),
            (false, 0) => "No matches".to_string(),
            (false, n) if n >= MAX_MATCHES => format!("{} of {MAX_MATCHES}+", self.current + 1),
            (false, n) => format!("{} of {n}", self.current + 1),
        }
    }

    pub fn step(&mut self, backwards: bool) -> Option<CellPos> {
        let count = self.matches.len();
        if count == 0 {
            return None;
        }
        self.current = if backwards {
            (self.current + count - 1) % count
        } else {
            (self.current + 1) % count
        };
        self.matches.get(self.current).copied()
    }
}

// Row by row like Excel's default "By Rows" search, matching displayed text
// or the formula, case-insensitively.
pub fn search(workbook: &Workbook, sheet: SheetId, query: &str) -> Vec<CellPos> {
    let needle = query.to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let end = workbook.used_end(sheet);
    let mut found = Vec::new();
    for row in 0..=end.row.get() {
        for col in 0..=end.col.get() {
            let pos = CellPos::new(
                RowIdx::clamped(i64::from(row)),
                ColIdx::clamped(i64::from(col)),
            );
            let text = workbook.cell(sheet, pos).text;
            let hit = text.to_lowercase().contains(&needle)
                || (!text.is_empty()
                    && workbook.input(sheet, pos).to_lowercase().contains(&needle));
            if hit {
                found.push(pos);
                if found.len() >= MAX_MATCHES {
                    return found;
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::{FindResults, search};
    use zenkai_engine::{Engine, Workbook};
    use zenkai_types::{CellPos, SheetId};

    #[test]
    fn finds_values_and_formulas_case_insensitively() {
        let mut book = Workbook::new_empty().unwrap();
        let sheet = SheetId(0);
        let rows = vec![
            vec!["Apple".to_string(), "banana".to_string()],
            vec!["=SUM(1,2)".to_string(), "pineapple".to_string()],
        ];
        book.set_inputs(sheet, CellPos::default(), &rows).unwrap();
        let hits: Vec<String> = search(&book, sheet, "APPLE")
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(hits, ["A1", "B2"]);
        let sum: Vec<String> = search(&book, sheet, "sum(")
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(sum, ["A2"]);
        assert!(search(&book, sheet, "").is_empty());
    }

    #[test]
    fn steps_wrap_both_ways() {
        let mut results = FindResults {
            query: "x".to_string(),
            matches: vec![
                CellPos::parse_a1("A1").unwrap(),
                CellPos::parse_a1("B2").unwrap(),
            ],
            current: 0,
        };
        assert_eq!(results.step(false).unwrap().to_string(), "B2");
        assert_eq!(results.step(false).unwrap().to_string(), "A1");
        assert_eq!(results.step(true).unwrap().to_string(), "B2");
        assert_eq!(results.status(), "2 of 2");
    }
}
