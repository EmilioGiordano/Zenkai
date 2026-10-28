use zenkai_types::{
    DefinedName, GoToTarget, Range, SheetId, SheetInfo, SheetVisibility, parse_go_to,
};

#[derive(Debug, PartialEq, Eq)]
pub enum GoToError {
    NotAReference,
    UnknownSheet(String),
    UnknownName(String),
}

pub fn needs_names(text: &str) -> bool {
    matches!(parse_go_to(text), Some(GoToTarget::Name { .. }))
}

pub fn resolve(
    text: &str,
    current: SheetId,
    sheets: &[SheetInfo],
    names: &[DefinedName],
) -> Result<(SheetId, Range), GoToError> {
    match parse_go_to(text).ok_or(GoToError::NotAReference)? {
        GoToTarget::Cells { sheet, range } => {
            let sheet = match sheet {
                Some(name) => find_sheet(sheets, &name)?,
                None => current,
            };
            Ok((sheet, range))
        }
        GoToTarget::Name { sheet, name } => {
            let qualifier = sheet.map(|sheet| find_sheet(sheets, &sheet)).transpose()?;
            let defined = find_name(names, &name, qualifier, current)
                .ok_or_else(|| GoToError::UnknownName(name.clone()))?;
            let scope = defined.scope.or(qualifier).unwrap_or(current);
            let refers_to = defined.refers_to.trim();
            let refers_to = refers_to.strip_prefix('=').unwrap_or(refers_to);
            match parse_go_to(refers_to) {
                Some(GoToTarget::Cells { sheet, range }) => {
                    let sheet = match sheet {
                        Some(name) => find_sheet(sheets, &name)?,
                        None => scope,
                    };
                    Ok((sheet, range))
                }
                _ => Err(GoToError::NotAReference),
            }
        }
    }
}

// Excel matches sheet names regardless of case, and cannot go to a hidden sheet.
fn find_sheet(sheets: &[SheetInfo], name: &str) -> Result<SheetId, GoToError> {
    let wanted = name.to_lowercase();
    sheets
        .iter()
        .find(|sheet| {
            sheet.visibility == SheetVisibility::Visible && sheet.name.to_lowercase() == wanted
        })
        .map(|sheet| sheet.id)
        .ok_or_else(|| GoToError::UnknownSheet(name.to_string()))
}

// A sheet-scoped name wins on its own sheet or behind its sheet prefix; the workbook-wide
// name is the fallback, as in Excel.
fn find_name<'a>(
    names: &'a [DefinedName],
    name: &str,
    qualifier: Option<SheetId>,
    current: SheetId,
) -> Option<&'a DefinedName> {
    let wanted = name.to_lowercase();
    let matching = || names.iter().filter(|n| n.name.to_lowercase() == wanted);
    let local = qualifier.unwrap_or(current);
    matching()
        .find(|n| n.scope == Some(local))
        .or_else(|| matching().find(|n| n.scope.is_none()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(id: u32, name: &str, visibility: SheetVisibility) -> SheetInfo {
        SheetInfo {
            id: SheetId(id),
            name: name.to_string(),
            visibility,
        }
    }

    fn sheets() -> Vec<SheetInfo> {
        vec![
            sheet(0, "Sheet1", SheetVisibility::Visible),
            sheet(1, "My sheet", SheetVisibility::Visible),
            sheet(2, "It's", SheetVisibility::Visible),
            sheet(3, "Secret", SheetVisibility::Hidden),
        ]
    }

    fn name(name: &str, scope: Option<u32>, refers_to: &str) -> DefinedName {
        DefinedName {
            name: name.to_string(),
            scope: scope.map(SheetId),
            refers_to: refers_to.to_string(),
        }
    }

    fn go(text: &str, names: &[DefinedName]) -> Result<(SheetId, String), GoToError> {
        resolve(text, SheetId(0), &sheets(), names).map(|(sheet, range)| (sheet, range.to_string()))
    }

    #[test]
    fn local_references_stay_on_the_current_sheet() {
        assert_eq!(go("b2:c4", &[]), Ok((SheetId(0), "B2:C4".to_string())));
    }

    #[test]
    fn sheet_references_switch_sheet_ignoring_case() {
        assert_eq!(go("sheet1!A1", &[]), Ok((SheetId(0), "A1".to_string())));
        assert_eq!(
            go("'MY SHEET'!B2:C4", &[]),
            Ok((SheetId(1), "B2:C4".to_string()))
        );
        assert_eq!(go("'It''s'!D4", &[]), Ok((SheetId(2), "D4".to_string())));
    }

    #[test]
    fn whole_columns_and_rows_select_the_full_line() {
        assert_eq!(
            go("'My sheet'!C:C", &[]),
            Ok((SheetId(1), "C1:C1048576".to_string()))
        );
        assert_eq!(go("3:5", &[]), Ok((SheetId(0), "A3:XFD5".to_string())));
    }

    #[test]
    fn unknown_or_hidden_sheets_are_refused() {
        assert_eq!(
            go("Nope!A1", &[]),
            Err(GoToError::UnknownSheet("Nope".to_string()))
        );
        assert_eq!(
            go("Secret!A1", &[]),
            Err(GoToError::UnknownSheet("Secret".to_string()))
        );
    }

    #[test]
    fn names_resolve_to_their_sheet_and_range() {
        let names = [name("Total", None, "'My sheet'!$B$2:$B$9")];
        assert_eq!(go("total", &names), Ok((SheetId(1), "B2:B9".to_string())));
        assert_eq!(
            go("Sheet1!TOTAL", &names),
            Ok((SheetId(1), "B2:B9".to_string()))
        );
    }

    #[test]
    fn sheet_scoped_names_win_on_their_sheet_only() {
        let names = [
            name("Rate", None, "Sheet1!$A$1"),
            name("Rate", Some(1), "'My sheet'!$C$3"),
        ];
        assert_eq!(go("Rate", &names), Ok((SheetId(0), "A1".to_string())));
        assert_eq!(
            resolve("Rate", SheetId(1), &sheets(), &names).map(|(s, r)| (s, r.to_string())),
            Ok((SheetId(1), "C3".to_string()))
        );
        assert_eq!(
            go("'My sheet'!Rate", &names),
            Ok((SheetId(1), "C3".to_string()))
        );
        let other = [name("Rate", Some(1), "'My sheet'!$C$3")];
        assert_eq!(
            go("Rate", &other),
            Err(GoToError::UnknownName("Rate".to_string()))
        );
    }

    #[test]
    fn names_scoped_to_a_hidden_or_missing_sheet_are_unknown() {
        let hidden = [name("Rate", Some(3), "Sheet1!$A$1")];
        let missing = [name("Rate", Some(9), "Sheet1!$A$1")];
        for names in [&hidden, &missing] {
            assert_eq!(
                go("Rate", names),
                Err(GoToError::UnknownName("Rate".to_string()))
            );
        }
    }

    #[test]
    fn names_that_are_not_ranges_are_refused() {
        let names = [
            name("Pi", None, "3.14159"),
            name("Union", None, "Sheet1!$A$1,Sheet1!$B$1"),
            name("Loop", None, "Loop"),
            name("Gone", None, "#REF!"),
        ];
        for text in ["Pi", "Union", "Loop", "Gone"] {
            assert_eq!(go(text, &names), Err(GoToError::NotAReference), "{text}");
        }
        assert_eq!(
            go("Missing", &names),
            Err(GoToError::UnknownName("Missing".to_string()))
        );
    }

    #[test]
    fn invalid_text_is_not_a_reference() {
        for text in ["", "A:3", "Sheet1!", "1abc", "a b"] {
            assert_eq!(go(text, &[]), Err(GoToError::NotAReference), "{text:?}");
        }
    }

    #[test]
    fn only_names_need_the_workbook_names() {
        assert!(needs_names("Total"));
        assert!(!needs_names("A1"));
        assert!(!needs_names("Sheet2!C:C"));
    }
}
