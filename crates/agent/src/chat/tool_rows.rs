use std::time::Duration;

use serde_json::Value;
use zenkai_i18n::t;

use crate::chat::thread::{Entry, ToolCard, ToolKind, ToolStatus};
use crate::settings::ToolCallDisplay;
use crate::tools::{is_protected_view_refusal, shown_text};

const RESULT_LINES: usize = 6;
const TARGET_KEYS: [&str; 6] = ["path", "file_path", "command", "pattern", "query", "url"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KnownTool {
    ListWorkbooks,
    ListSheets,
    GetSelection,
    ReadRange,
    Find,
    WriteCells,
    SetFormula,
    FormatRange,
    GenerateData,
    CreateWorkbook,
    OpenWorkbook,
}

const KNOWN: [(&str, KnownTool); 11] = [
    ("list_workbooks", KnownTool::ListWorkbooks),
    ("list_sheets", KnownTool::ListSheets),
    ("get_selection", KnownTool::GetSelection),
    ("read_range", KnownTool::ReadRange),
    ("find", KnownTool::Find),
    ("write_cells", KnownTool::WriteCells),
    ("set_formula", KnownTool::SetFormula),
    ("format_range", KnownTool::FormatRange),
    ("generate_data", KnownTool::GenerateData),
    ("create_workbook", KnownTool::CreateWorkbook),
    ("open_workbook", KnownTool::OpenWorkbook),
];

impl KnownTool {
    // Agents name the same tool "read_range", "mcp__zenkai__read_range" or "Read range"; only a
    // whole name counts, so "Finding files" is not the find tool.
    fn of(title: &str) -> Option<KnownTool> {
        let normalized = title.trim().to_lowercase().replace([' ', '-'], "_");
        let mut parts = normalized.split("__").flat_map(|part| part.split([':', '.', '/']));
        KNOWN
            .iter()
            .find(|(name, _)| parts.any(|part| part == *name))
            .map(|(_, tool)| *tool)
    }

    fn label(self) -> &'static str {
        match self {
            KnownTool::ListWorkbooks => t!("chat.tool.label.list_workbooks"),
            KnownTool::ListSheets => t!("chat.tool.label.list_sheets"),
            KnownTool::GetSelection => t!("chat.tool.label.get_selection"),
            KnownTool::ReadRange => t!("chat.tool.label.read_range"),
            KnownTool::Find => t!("chat.tool.label.find"),
            KnownTool::WriteCells => t!("chat.tool.label.write_cells"),
            KnownTool::SetFormula => t!("chat.tool.label.set_formula"),
            KnownTool::FormatRange => t!("chat.tool.label.format_range"),
            KnownTool::GenerateData => t!("chat.tool.label.generate_data"),
            KnownTool::CreateWorkbook => t!("chat.tool.label.create_workbook"),
            KnownTool::OpenWorkbook => t!("chat.tool.label.open_workbook"),
        }
    }

    fn is_read_only(self) -> bool {
        matches!(
            self,
            KnownTool::ListWorkbooks
                | KnownTool::ListSheets
                | KnownTool::GetSelection
                | KnownTool::ReadRange
                | KnownTool::Find
        )
    }

    fn target(self, input: &Value) -> Option<String> {
        let text = |key: &str| input.get(key).and_then(Value::as_str);
        let on_sheet = |key: &str| match (text("sheet"), text(key)) {
            (Some(sheet), Some(place)) => Some(format!("{sheet}!{place}")),
            (None, Some(place)) => Some(place.to_string()),
            (Some(sheet), None) => Some(sheet.to_string()),
            (None, None) => None,
        };
        match self {
            KnownTool::ReadRange | KnownTool::FormatRange => on_sheet("range"),
            KnownTool::WriteCells | KnownTool::GenerateData => on_sheet("start"),
            KnownTool::SetFormula => on_sheet("cell"),
            KnownTool::Find => text("text").map(|found| format!("\"{found}\"")),
            KnownTool::CreateWorkbook | KnownTool::OpenWorkbook => text("path").map(str::to_string),
            KnownTool::ListWorkbooks | KnownTool::ListSheets | KnownTool::GetSelection => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowText {
    pub label: String,
    pub target: String,
}

fn parsed(input: &str) -> Option<Value> {
    serde_json::from_str(input).ok()
}

fn generic_target(input: &Value) -> Option<String> {
    TARGET_KEYS
        .iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
        .map(str::to_string)
}

pub fn row_text(card: &ToolCard) -> RowText {
    let known = KnownTool::of(&card.title);
    let input = parsed(&card.input);
    let target = input
        .as_ref()
        .and_then(|input| match known {
            Some(tool) => tool.target(input),
            None => generic_target(input),
        })
        .map(|target| shown_text(&target))
        .unwrap_or_default();
    RowText {
        label: known.map_or_else(|| card.title.clone(), |tool| tool.label().to_string()),
        target,
    }
}

pub fn is_read_only(card: &ToolCard) -> bool {
    match KnownTool::of(&card.title) {
        Some(tool) => tool.is_read_only(),
        None => matches!(card.kind, ToolKind::Read | ToolKind::Search),
    }
}

// Calls the user has to see whatever the display setting says.
pub fn needs_attention(card: &ToolCard) -> bool {
    card.status == ToolStatus::WaitingForPermission
        || (card.status == ToolStatus::Failed && is_protected_view_refusal(&card.detail))
}

fn groupable(card: &ToolCard) -> bool {
    card.status == ToolStatus::Completed && is_read_only(card)
}

pub fn group_text(cards: &[&ToolCard]) -> RowText {
    let count = cards.len();
    let all_ranges = cards
        .iter()
        .all(|card| KnownTool::of(&card.title) == Some(KnownTool::ReadRange));
    let label = if all_ranges {
        t!("chat.tool.group.ranges", count = count)
    } else {
        t!("chat.tool.group.calls", count = count)
    };
    let targets: Vec<String> = cards
        .iter()
        .map(|card| row_text(card).target)
        .filter(|target| !target.is_empty())
        .collect();
    RowText {
        label,
        target: targets.join(", "),
    }
}

pub fn used_label(count: usize) -> String {
    t!("chat.tool.used", count = count)
}

pub fn failed_label(label: &str) -> String {
    t!("chat.tool.failed_label", label = label)
}

pub fn took_label(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if seconds < 60.0 {
        t!("chat.tool.took", seconds = format!("{seconds:.1}"))
    } else {
        let whole = duration.as_secs();
        t!(
            "chat.tool.took_long",
            minutes = whole / 60,
            seconds = whole % 60
        )
    }
}

pub fn pretty_input(input: &str) -> String {
    parsed(input)
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| input.to_string())
}

pub struct ResultPreview {
    pub shown: String,
    pub more_lines: usize,
}

pub fn result_preview(detail: &str) -> ResultPreview {
    let mut lines = detail.lines();
    let shown: Vec<&str> = lines.by_ref().take(RESULT_LINES).collect();
    ResultPreview {
        shown: shown.join("\n"),
        more_lines: lines.count(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Entry(usize),
    Call(usize),
    Group(Vec<usize>),
    Used { count: usize },
}

fn tool_at(entries: &[Entry], index: usize) -> Option<&ToolCard> {
    match entries.get(index) {
        Some(Entry::Tool(card)) => Some(card),
        _ => None,
    }
}

fn hidden_in_turn(entries: &[Entry], from: usize) -> usize {
    entries[from..]
        .iter()
        .take_while(|entry| !matches!(entry, Entry::User(_)))
        .filter(|entry| matches!(entry, Entry::Tool(card) if !needs_attention(card)))
        .count()
}

pub fn plan(entries: &[Entry], display: ToolCallDisplay) -> Vec<Item> {
    let mut items = Vec::new();
    let mut summarised = false;
    let mut index = 0;
    while index < entries.len() {
        let Some(card) = tool_at(entries, index) else {
            if matches!(entries[index], Entry::User(_)) {
                summarised = false;
            }
            items.push(Item::Entry(index));
            index += 1;
            continue;
        };
        if display == ToolCallDisplay::Hidden && !needs_attention(card) {
            if !summarised {
                summarised = true;
                items.push(Item::Used {
                    count: hidden_in_turn(entries, index),
                });
            }
            index += 1;
            continue;
        }
        let run: Vec<usize> = (index..entries.len())
            .take_while(|next| tool_at(entries, *next).is_some_and(groupable))
            .collect();
        if display == ToolCallDisplay::Collapsed && run.len() >= 2 {
            index += run.len();
            items.push(Item::Group(run));
        } else {
            items.push(Item::Call(index));
            index += 1;
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::thread::ToolCallId;

    fn card(id: &str, title: &str, status: ToolStatus, input: &str) -> ToolCard {
        ToolCard {
            id: ToolCallId::new(id),
            title: title.to_string(),
            kind: ToolKind::Other,
            status,
            detail: String::new(),
            input: input.to_string(),
            started: None,
            took: None,
        }
    }

    fn read(id: &str) -> Entry {
        Entry::Tool(card(
            id,
            "mcp__zenkai__read_range",
            ToolStatus::Completed,
            r#"{"sheet":"Sales","range":"A1:F9"}"#,
        ))
    }

    fn write(id: &str, status: ToolStatus) -> Entry {
        Entry::Tool(card(id, "write_cells", status, r#"{"sheet":"S","start":"A1"}"#))
    }

    #[test]
    fn a_zenkai_tool_gets_a_human_label_and_its_target() {
        let text = row_text(&card(
            "1",
            "mcp__zenkai__read_range",
            ToolStatus::Completed,
            r#"{"workbook":1,"sheet":"Sales","range":"A1:F4210"}"#,
        ));
        assert_eq!(text.label, "Read range");
        assert_eq!(text.target, "Sales!A1:F4210");
    }

    #[test]
    fn only_a_whole_tool_name_counts() {
        let finding = card("1", "Finding files", ToolStatus::Completed, "");
        assert_eq!(row_text(&finding).label, "Finding files");
        assert!(!is_read_only(&finding));
        let found = card("2", "Find", ToolStatus::Completed, r#"{"text":"total"}"#);
        assert_eq!(row_text(&found).label, "Find");
        assert_eq!(row_text(&found).target, "\"total\"");
        assert!(is_read_only(&found));
    }

    #[test]
    fn an_unknown_tool_keeps_its_title_and_takes_a_path_like_target() {
        let text = row_text(&card(
            "1",
            "Read notes.txt",
            ToolStatus::Completed,
            r#"{"file_path":"C:\\a\\notes.txt"}"#,
        ));
        assert_eq!(text.label, "Read notes.txt");
        assert_eq!(text.target, "C:\\a\\notes.txt");
    }

    #[test]
    fn input_that_is_not_json_gives_no_target_and_stays_raw() {
        let broken = card("1", "read_range", ToolStatus::Completed, r#"{"sheet":"Sa…"#);
        assert_eq!(row_text(&broken).target, "");
        assert_eq!(pretty_input(&broken.input), broken.input);
        assert_eq!(pretty_input(r#"{"a":1}"#), "{\n  \"a\": 1\n}");
    }

    #[test]
    fn control_characters_in_a_target_are_escaped() {
        let text = row_text(&card(
            "1",
            "read_range",
            ToolStatus::Completed,
            "{\"sheet\":\"A\\nB\",\"range\":\"A1\"}",
        ));
        assert!(!text.target.contains('\n'));
    }

    #[test]
    fn consecutive_completed_reads_group_into_one_row() {
        let entries = vec![
            Entry::User("go".into()),
            read("1"),
            read("2"),
            write("3", ToolStatus::Completed),
        ];
        assert_eq!(
            plan(&entries, ToolCallDisplay::Collapsed),
            vec![
                Item::Entry(0),
                Item::Group(vec![1, 2]),
                Item::Call(3)
            ]
        );
        let cards: Vec<&ToolCard> = [1, 2].iter().filter_map(|i| tool_at(&entries, *i)).collect();
        let text = group_text(&cards);
        assert_eq!(text.label, "Read 2 ranges");
        assert_eq!(text.target, "Sales!A1:F9, Sales!A1:F9");
    }

    #[test]
    fn a_lone_read_a_running_read_and_an_interrupted_run_do_not_group() {
        let running = Entry::Tool(card("r", "read_range", ToolStatus::InProgress, ""));
        let entries = vec![read("1"), running, read("2"), Entry::Notice {
            kind: crate::chat::thread::NoticeKind::Info,
            text: String::new(),
        }, read("3"), read("4")];
        assert_eq!(
            plan(&entries, ToolCallDisplay::Collapsed),
            vec![
                Item::Call(0),
                Item::Call(1),
                Item::Call(2),
                Item::Entry(3),
                Item::Group(vec![4, 5])
            ]
        );
    }

    #[test]
    fn expanded_never_groups() {
        let entries = vec![read("1"), read("2")];
        assert_eq!(
            plan(&entries, ToolCallDisplay::Expanded),
            vec![Item::Call(0), Item::Call(1)]
        );
    }

    #[test]
    fn hidden_keeps_one_summary_per_turn_and_the_calls_waiting_for_you() {
        let entries = vec![
            Entry::User("a".into()),
            read("1"),
            write("2", ToolStatus::WaitingForPermission),
            read("3"),
            Entry::User("b".into()),
            read("4"),
        ];
        assert_eq!(
            plan(&entries, ToolCallDisplay::Hidden),
            vec![
                Item::Entry(0),
                Item::Used { count: 2 },
                Item::Call(2),
                Item::Entry(4),
                Item::Used { count: 1 }
            ]
        );
        assert_eq!(used_label(1), "Used 1 tool");
        assert_eq!(used_label(4), "Used 4 tools");
    }

    #[test]
    fn a_protected_view_refusal_is_never_hidden() {
        let mut refused = card("1", "write_cells", ToolStatus::Failed, "");
        refused.detail =
            "the file came from the internet (Protected View), so agents may only read it".to_string();
        assert!(needs_attention(&refused));
        assert!(!needs_attention(&card("2", "write_cells", ToolStatus::Failed, "")));
        assert!(needs_attention(&card(
            "3",
            "x",
            ToolStatus::WaitingForPermission,
            ""
        )));
    }

    #[test]
    fn long_results_are_cut_with_a_count_of_what_is_left() {
        let detail = (1..=10).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let preview = result_preview(&detail);
        assert_eq!(preview.shown.lines().count(), RESULT_LINES);
        assert_eq!(preview.more_lines, 4);
        assert_eq!(result_preview("one line").more_lines, 0);
    }

    #[test]
    fn durations_read_in_seconds_then_minutes() {
        assert_eq!(took_label(Duration::from_millis(120)), "0.1 s");
        assert_eq!(took_label(Duration::from_secs(75)), "1 min 15 s");
    }
}
