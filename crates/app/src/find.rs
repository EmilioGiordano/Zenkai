use gpui_kit::component::input::InputState;
use gpui_kit::*;
use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, ColIdx, RowIdx, SheetId};

pub const MAX_MATCHES: usize = 100_000;

pub struct FindBar {
    pub input: Entity<InputState>,
    pub replace: Option<Entity<InputState>>,
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

// Every filled cell whose content contains `query` (ignoring case), with each occurrence
// replaced, as Excel's Replace All with "Look in: Formulas".
pub fn replacements(
    workbook: &Workbook,
    sheet: SheetId,
    query: &str,
    replacement: &str,
) -> Vec<(CellPos, String)> {
    if query.is_empty() {
        return Vec::new();
    }
    workbook
        .filled_cells(sheet)
        .into_iter()
        .filter_map(|pos| {
            let input = workbook.input(sheet, pos);
            let replaced = replace_ignoring_case(&input, query, replacement);
            (replaced != input).then_some((pos, replaced))
        })
        .collect()
}

fn replace_ignoring_case(text: &str, query: &str, replacement: &str) -> String {
    let lower_text = text.to_lowercase();
    let lower_query = query.to_lowercase();
    // Lowercasing can change a character's byte length outside ASCII, which would shift
    // match offsets (even when the totals happen to agree); fall back to an exact match.
    let same_widths = |s: &str| {
        s.chars()
            .all(|c| c.to_lowercase().map(char::len_utf8).sum::<usize>() == c.len_utf8())
    };
    if !same_widths(text) || !same_widths(query) {
        return text.replace(query, replacement);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, _) in lower_text.match_indices(&lower_query) {
        out.push_str(&text[last..at]);
        out.push_str(replacement);
        last = at + query.len();
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::{FindResults, replace_ignoring_case, search};

    #[test]
    fn replaces_every_occurrence_ignoring_case() {
        // U+0130 grows and U+212A shrinks when lowercased; their sum hides the shift.
        assert_eq!(
            replace_ignoring_case("\u{130}\u{212A}ab", "ab", "x"),
            "\u{130}\u{212A}x"
        );
        assert_eq!(
            replace_ignoring_case("Total total TOTAL", "total", "Sum"),
            "Sum Sum Sum"
        );
        assert_eq!(replace_ignoring_case("=SUM(A1)", "a1", "B2"), "=SUM(B2)");
        assert_eq!(replace_ignoring_case("Ñandú", "ú", "u"), "Ñandu");
    }

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

#[cfg(test)]
mod no_panic {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 2_000, ..ProptestConfig::default() })]

        #[test]
        fn replace_accepts_any_text(text in any::<String>(), query in ".{1,4}", with in ".{0,4}") {
            let _ = super::replace_ignoring_case(&text, &query, &with);
        }
    }
}
