use super::mention_parse::Mention;
use super::plain_name;
use super::reference::Reference;
use crate::assets::{RANGE_REFERENCE_MARK, SHEET_REFERENCE_MARK, WORKBOOK_REFERENCE_MARK};

const MAX_WORKBOOK_CHARS: usize = 28;
const SEPARATOR: &str = " / ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Workbook,
    Sheet,
    Range,
}

impl Kind {
    pub(super) fn icon(self) -> &'static str {
        match self {
            Kind::Workbook => WORKBOOK_REFERENCE_MARK,
            Kind::Sheet => SHEET_REFERENCE_MARK,
            Kind::Range => RANGE_REFERENCE_MARK,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Label {
    pub kind: Kind,
    pub workbook: Option<String>,
    pub name: String,
    pub full: String,
}

fn middle_truncated(text: &str, limit: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit || limit < 3 {
        return text.to_string();
    }
    let kept = limit - 1;
    let head = kept.div_ceil(2);
    let tail = kept - head;
    let mut shortened: String = chars[..head].iter().collect();
    shortened.push('…');
    shortened.extend(&chars[chars.len() - tail..]);
    shortened
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

impl Label {
    fn within(kind: Kind, workbook: Option<&str>, name: String) -> Label {
        let workbook = workbook.map(plain_name).filter(|name| !name.is_empty());
        let full = match &workbook {
            Some(workbook) => format!("{workbook}{SEPARATOR}{name}"),
            None => name.clone(),
        };
        Label {
            kind,
            workbook: workbook.map(|name| middle_truncated(&name, MAX_WORKBOOK_CHARS)),
            name,
            full,
        }
    }

    pub(super) fn of_reference(reference: &Reference, workbook: Option<&str>) -> Label {
        let workbook = reference.workbook.as_deref().or(workbook);
        Label::within(Kind::Range, workbook, plain_name(&reference.local_text()))
    }

    pub(super) fn of_mention(mention: &Mention, workbook: Option<&str>) -> Label {
        match mention {
            Mention::Selection => Label::within(Kind::Range, workbook, "selection".to_string()),
            Mention::Sheet(name) => Label::within(Kind::Sheet, workbook, plain_name(name)),
            Mention::BookSheet { workbook, name } => {
                Label::within(Kind::Sheet, Some(workbook), plain_name(name))
            }
            Mention::Cells(reference) => Label::of_reference(reference, workbook),
            Mention::Workbook(path) => {
                let shown = plain_name(file_name(path));
                Label {
                    kind: Kind::Workbook,
                    workbook: None,
                    full: plain_name(path),
                    name: shown,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use zenkai_types::Range;

    use super::*;

    fn reference(sheet: &str, range: &str) -> Reference {
        Reference {
            workbook: None,
            sheet: Some(sheet.to_string()),
            range: Range::parse_a1(range).unwrap(),
        }
    }

    #[test]
    fn short_names_are_not_truncated() {
        assert_eq!(middle_truncated("budget-2026.xlsx", 28), "budget-2026.xlsx");
        assert_eq!(middle_truncated("abcde", 5), "abcde");
    }

    #[test]
    fn long_names_keep_both_ends() {
        let shortened = middle_truncated("quarterly-budget-forecast-2026-final.xlsx", 20);
        assert_eq!(shortened.chars().count(), 20);
        assert!(shortened.starts_with("quarterly-"));
        assert!(shortened.ends_with("l.xlsx"));
        assert!(shortened.contains('…'));
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        let shortened = middle_truncated("ééééééééééééé", 6);
        assert_eq!(shortened.chars().count(), 6);
    }

    #[test]
    fn a_workbook_shows_its_file_name_and_keeps_the_path_in_the_tooltip() {
        let label = Label::of_mention(
            &Mention::Workbook("reports/budget 2026.xlsx".to_string()),
            Some("other.xlsx"),
        );
        assert_eq!(label.kind, Kind::Workbook);
        assert_eq!(label.workbook, None);
        assert_eq!(label.name, "budget 2026.xlsx");
        assert_eq!(label.full, "reports/budget 2026.xlsx");
    }

    #[test]
    fn a_sheet_names_its_workbook() {
        let label = Label::of_mention(&Mention::Sheet("Summary".to_string()), Some("a.xlsx"));
        assert_eq!(label.kind, Kind::Sheet);
        assert_eq!(label.workbook.as_deref(), Some("a.xlsx"));
        assert_eq!(label.name, "Summary");
        assert_eq!(label.full, "a.xlsx / Summary");
    }

    #[test]
    fn a_range_shows_sheet_and_cells_after_the_workbook() {
        let label = Label::of_reference(&reference("My Sheet", "A1:C40"), Some("a.xlsx"));
        assert_eq!(label.kind, Kind::Range);
        assert_eq!(label.name, "'My Sheet'!A1:C40");
        assert_eq!(label.full, "a.xlsx / 'My Sheet'!A1:C40");
    }

    #[test]
    fn a_sheet_of_another_workbook_shows_that_workbook_not_the_sending_one() {
        let label = Label::of_mention(
            &Mention::BookSheet {
                workbook: "b.xlsx".to_string(),
                name: "Base".to_string(),
            },
            Some("a.xlsx"),
        );
        assert_eq!(label.full, "b.xlsx / Base");
        let mut external = reference("Base", "A1");
        external.workbook = Some("b.xlsx".to_string());
        let label = Label::of_reference(&external, Some("a.xlsx"));
        assert_eq!(label.full, "b.xlsx / Base!A1");
    }

    #[test]
    fn without_a_workbook_only_the_name_shows() {
        let label = Label::of_reference(&reference("Sales", "B4"), None);
        assert_eq!(label.workbook, None);
        assert_eq!(label.full, "Sales!B4");
    }

    #[test]
    fn names_from_files_lose_control_characters_and_quotes() {
        let label = Label::of_mention(
            &Mention::Sheet("ev\nil\"".to_string()),
            Some("x\u{7}\".xlsx"),
        );
        assert_eq!(label.name, "evil");
        assert_eq!(label.workbook.as_deref(), Some("x.xlsx"));
    }

    #[test]
    fn a_long_workbook_name_is_shortened_but_the_tooltip_keeps_it_whole() {
        let long = format!("{}.xlsx", "n".repeat(60));
        let label = Label::of_mention(&Mention::Selection, Some(&long));
        assert!(label.workbook.as_deref().unwrap().chars().count() <= MAX_WORKBOOK_CHARS);
        assert!(label.full.starts_with(&long));
    }
}
