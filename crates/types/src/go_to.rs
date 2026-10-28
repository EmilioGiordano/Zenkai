use crate::{CellPos, ColIdx, Range, RowIdx};

const MAX_NAME_CHARS: usize = 255;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefinedName {
    pub name: String,
    pub scope: Option<crate::SheetId>,
    pub refers_to: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoToTarget {
    Cells { sheet: Option<String>, range: Range },
    Name { sheet: Option<String>, name: String },
}

// What Excel's Go To accepts: A1, A1:C4, C:C, 3:5, a defined name, each optionally
// prefixed by a sheet name, quoted when it has spaces or apostrophes.
pub fn parse_go_to(text: &str) -> Option<GoToTarget> {
    let (sheet, reference) = split_sheet(text.trim())?;
    let reference = reference.trim();
    if let Some(range) = parse_area(reference) {
        return Some(GoToTarget::Cells { sheet, range });
    }
    is_name(reference).then(|| GoToTarget::Name {
        sheet,
        name: reference.to_string(),
    })
}

fn split_sheet(text: &str) -> Option<(Option<String>, &str)> {
    if let Some(quoted) = text.strip_prefix('\'') {
        let mut name = String::new();
        let mut chars = quoted.char_indices().peekable();
        while let Some((index, c)) = chars.next() {
            if c != '\'' {
                name.push(c);
            } else if chars.next_if(|(_, next)| *next == '\'').is_some() {
                name.push('\'');
            } else {
                let rest = quoted[index + 1..].strip_prefix('!')?;
                return (!name.is_empty()).then_some((Some(name), rest));
            }
        }
        return None;
    }
    match text.split_once('!') {
        None => Some((None, text)),
        Some((sheet, rest)) => {
            let valid = !sheet.is_empty() && !sheet.contains('\'') && !rest.contains('!');
            valid.then(|| (Some(sheet.to_string()), rest))
        }
    }
}

fn parse_area(text: &str) -> Option<Range> {
    if let Some(range) = Range::parse_a1(text) {
        return Some(range);
    }
    let (first, second) = text.split_once(':')?;
    if let (Some(a), Some(b)) = (parse_column(first), parse_column(second)) {
        return Some(Range::new(
            CellPos::new(RowIdx::default(), a),
            CellPos::new(RowIdx::LAST, b),
        ));
    }
    let (a, b) = (parse_row(first)?, parse_row(second)?);
    Some(Range::new(
        CellPos::new(a, ColIdx::default()),
        CellPos::new(b, ColIdx::LAST),
    ))
}

fn parse_column(text: &str) -> Option<ColIdx> {
    let letters = text.strip_prefix('$').unwrap_or(text);
    if letters.is_empty() || letters.len() > 3 || !letters.bytes().all(|b| b.is_ascii_alphabetic())
    {
        return None;
    }
    let number = letters.bytes().fold(0u32, |acc, b| {
        acc * 26 + u32::from(b.to_ascii_uppercase() - b'A' + 1)
    });
    ColIdx::new(u16::try_from(number - 1).ok()?)
}

fn parse_row(text: &str) -> Option<RowIdx> {
    let digits = text.strip_prefix('$').unwrap_or(text);
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    RowIdx::new(digits.parse::<u32>().ok()?.checked_sub(1)?)
}

fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_' || first == '\\')
        && text.chars().count() <= MAX_NAME_CHARS
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '\\'))
        && CellPos::parse_a1(text).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(sheet: Option<&str>, range: &str) -> Option<GoToTarget> {
        Some(GoToTarget::Cells {
            sheet: sheet.map(str::to_string),
            range: Range::parse_a1(range).unwrap(),
        })
    }

    fn name(sheet: Option<&str>, name: &str) -> Option<GoToTarget> {
        Some(GoToTarget::Name {
            sheet: sheet.map(str::to_string),
            name: name.to_string(),
        })
    }

    #[test]
    fn plain_references_have_no_sheet() {
        assert_eq!(parse_go_to("b2"), cells(None, "B2"));
        assert_eq!(parse_go_to(" $A$1:C4 "), cells(None, "A1:C4"));
    }

    #[test]
    fn sheet_prefix_plain_and_quoted() {
        assert_eq!(parse_go_to("Sheet2!A1"), cells(Some("Sheet2"), "A1"));
        assert_eq!(
            parse_go_to("'My sheet'!B2:C4"),
            cells(Some("My sheet"), "B2:C4")
        );
        assert_eq!(parse_go_to("'It''s'!A1"), cells(Some("It's"), "A1"));
        assert_eq!(parse_go_to("'a!b'!A1"), cells(Some("a!b"), "A1"));
    }

    #[test]
    fn whole_columns_and_rows() {
        let columns = parse_go_to("C:C").unwrap();
        assert_eq!(
            columns,
            GoToTarget::Cells {
                sheet: None,
                range: Range::new(
                    CellPos::new(RowIdx::default(), ColIdx::new(2).unwrap()),
                    CellPos::new(RowIdx::LAST, ColIdx::new(2).unwrap()),
                ),
            }
        );
        let rows = parse_go_to("Data!3:5").unwrap();
        assert_eq!(
            rows,
            GoToTarget::Cells {
                sheet: Some("Data".to_string()),
                range: Range::new(
                    CellPos::new(RowIdx::new(2).unwrap(), ColIdx::default()),
                    CellPos::new(RowIdx::new(4).unwrap(), ColIdx::LAST),
                ),
            }
        );
        assert!(parse_go_to("$B:$D").is_some());
        assert_eq!(parse_go_to("5:3"), parse_go_to("3:5"));
    }

    #[test]
    fn names_with_and_without_sheet() {
        assert_eq!(parse_go_to("Total"), name(None, "Total"));
        assert_eq!(
            parse_go_to("Sheet1!tax_rate"),
            name(Some("Sheet1"), "tax_rate")
        );
        assert_eq!(parse_go_to("my.name"), name(None, "my.name"));
    }

    #[test]
    fn invalid_input_is_refused() {
        for text in [
            "",
            "   ",
            "!A1",
            "''!A1",
            "'open!A1",
            "'a'b'!A1",
            "A:3",
            "3:C",
            "0:2",
            "1048577:1048578",
            "XFE:XFE",
            "A1:",
            "1abc",
            "a b",
            "S!T!A1",
            "Sheet1!",
            "A1:B2:C3",
            "-5",
            "=A1",
        ] {
            assert_eq!(parse_go_to(text), None, "{text:?}");
        }
    }
}
