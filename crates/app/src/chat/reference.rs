use zenkai_types::Range;

// Excel quotes a sheet name unless it is a plain identifier that cannot be read as a cell.
fn needs_quotes(sheet: &str) -> bool {
    let plain = !sheet.is_empty()
        && sheet.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !sheet.starts_with(|c: char| c.is_ascii_digit());
    !plain || zenkai_types::CellPos::parse_a1(sheet).is_some() || Range::parse_a1(sheet).is_some()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub sheet: Option<String>,
    pub range: Range,
}

impl Reference {
    pub fn text(&self) -> String {
        match &self.sheet {
            Some(sheet) => sheet_range(sheet, self.range),
            None => self.range.to_string(),
        }
    }
}

pub fn sheet_range(sheet: &str, range: Range) -> String {
    if needs_quotes(sheet) {
        format!("'{}'!{range}", sheet.replace('\'', "''"))
    } else {
        format!("{sheet}!{range}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(text: &str) -> Range {
        Range::parse_a1(text).unwrap()
    }

    #[test]
    fn plain_names_stay_bare() {
        assert_eq!(sheet_range("Ventas", range("B2:B20")), "Ventas!B2:B20");
        assert_eq!(sheet_range("Q3_2024", range("A1")), "Q3_2024!A1");
    }

    #[test]
    fn names_with_spaces_or_symbols_are_quoted() {
        assert_eq!(sheet_range("My Sheet", range("A1")), "'My Sheet'!A1");
        assert_eq!(
            sheet_range("Costs-2024", range("A1:B2")),
            "'Costs-2024'!A1:B2"
        );
    }

    #[test]
    fn a_quote_inside_the_name_is_doubled() {
        assert_eq!(sheet_range("Bob's", range("A1")), "'Bob''s'!A1");
    }

    #[test]
    fn names_that_look_like_cells_or_start_with_a_digit_are_quoted() {
        assert_eq!(sheet_range("A1", range("B2")), "'A1'!B2");
        assert_eq!(sheet_range("2024", range("B2")), "'2024'!B2");
        assert_eq!(sheet_range("", range("B2")), "''!B2");
    }
}
