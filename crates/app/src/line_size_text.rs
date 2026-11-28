use zenkai_i18n::t;
use zenkai_types::LineAxis;

use crate::line_size::{Lines, Prefill, Refusal, Separator, Slot, format_number, limit};

pub fn title(axis: LineAxis) -> &'static str {
    match axis {
        LineAxis::Row => t!("linesize.row.title"),
        LineAxis::Column => t!("linesize.column.title"),
    }
}

pub fn label(axis: LineAxis) -> &'static str {
    match axis {
        LineAxis::Row => t!("linesize.row.label"),
        LineAxis::Column => t!("linesize.column.label"),
    }
}

pub fn unit(axis: LineAxis) -> &'static str {
    match axis {
        LineAxis::Row => t!("linesize.row.unit"),
        LineAxis::Column => t!("linesize.column.unit"),
    }
}

pub fn field_label(axis: LineAxis) -> &'static str {
    match axis {
        LineAxis::Row => t!("linesize.row.field"),
        LineAxis::Column => t!("linesize.column.field"),
    }
}

pub fn stale_notice(axis: LineAxis) -> &'static str {
    match axis {
        LineAxis::Row => t!("notice.sheet_changed_reopen_row_height"),
        LineAxis::Column => t!("notice.sheet_changed_reopen_column_width"),
    }
}

pub fn scope(lines: Lines, prefill: Prefill) -> String {
    let count = lines.count();
    let target = lines.target();
    let scope = match (lines.axis, lines.is_all()) {
        (LineAxis::Row, true) => t!("linesize.row.scope_all").to_string(),
        (LineAxis::Row, false) => t!("linesize.row.scope", count = count, target = target),
        (LineAxis::Column, true) => t!("linesize.column.scope_all").to_string(),
        (LineAxis::Column, false) => t!("linesize.column.scope", count = count, target = target),
    };
    if prefill != Prefill::Mixed {
        return scope;
    }
    let differs = match lines.axis {
        LineAxis::Row => t!("linesize.row.differs"),
        LineAxis::Column => t!("linesize.column.differs"),
    };
    format!("{scope}. {differs}")
}

// What the slot under the field says. It is two lines tall in every state, so what goes
// here must fit two lines (see the budget test).
pub fn slot(lines: Lines, slot: Slot, separator: Separator) -> String {
    let (count, target) = (lines.count(), lines.target());
    let max = format_number(limit(lines.axis), separator);
    match (slot, lines.axis) {
        (Slot::Hint, _) => t!("linesize.hint", max = max),
        (Slot::ZeroNote, LineAxis::Row) => t!("linesize.row.zero", count = count, target = target),
        (Slot::ZeroNote, LineAxis::Column) => {
            t!("linesize.column.zero", count = count, target = target)
        }
        (Slot::HiddenNote, LineAxis::Row) => t!("linesize.row.hidden", count = count),
        (Slot::HiddenNote, LineAxis::Column) => t!("linesize.column.hidden", count = count),
        (Slot::Refusal(Refusal::OutOfRange), LineAxis::Row) => t!("linesize.row.range", max = max),
        (Slot::Refusal(Refusal::OutOfRange), LineAxis::Column) => {
            t!("linesize.column.range", max = max)
        }
        (Slot::Refusal(Refusal::EveryLineHidden) | Slot::EveryLineHiddenNote, LineAxis::Row) => {
            t!("linesize.row.refusal").to_string()
        }
        (Slot::Refusal(Refusal::EveryLineHidden) | Slot::EveryLineHiddenNote, LineAxis::Column) => {
            t!("linesize.column.refusal").to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use zenkai_i18n::embedded_catalog;
    use zenkai_types::{Language, LineAxis, MAX_COLS, MAX_ROWS};

    use super::{scope, slot};
    use crate::line_size::{Lines, Prefill, Refusal, Separator, Slot};

    const LINE_BUDGET: usize = 50;
    const SLOT_LINES: usize = 2;

    fn rows(first: u32, last: u32) -> Lines {
        Lines {
            axis: LineAxis::Row,
            first,
            last,
        }
    }

    fn columns(first: u32, last: u32) -> Lines {
        Lines {
            axis: LineAxis::Column,
            first,
            last,
        }
    }

    #[test]
    fn the_scope_names_the_lines_and_says_when_their_sizes_differ() {
        let uniform = Prefill::Uniform(20.0);
        assert_eq!(scope(rows(6, 6), uniform), "Applies to row 7");
        assert_eq!(scope(rows(3, 5), uniform), "Applies to rows 4:6");
        assert_eq!(scope(rows(0, MAX_ROWS - 1), uniform), "Applies to all rows");
        assert_eq!(
            scope(rows(3, 8), Prefill::Mixed),
            "Applies to rows 4:9. Heights differ."
        );
        assert_eq!(scope(columns(2, 2), uniform), "Applies to column C");
        assert_eq!(scope(columns(1, 3), uniform), "Applies to columns B:D");
        assert_eq!(
            scope(columns(0, u32::from(MAX_COLS) - 1), uniform),
            "Applies to all columns"
        );
        assert_eq!(
            scope(columns(1, 3), Prefill::Mixed),
            "Applies to columns B:D. Widths differ."
        );
    }

    #[test]
    fn the_slot_says_what_the_field_will_do() {
        let en = Separator::Dot;
        let row = rows(3, 5);
        assert_eq!(slot(row, Slot::Hint, en), "0 to 409.5. Enter 0 to hide.");
        assert_eq!(
            slot(columns(1, 3), Slot::Hint, en),
            "0 to 255. Enter 0 to hide."
        );
        assert_eq!(
            slot(row, Slot::ZeroNote, en),
            "Hides rows 4:6. To see them again, use Unhide rows."
        );
        assert_eq!(
            slot(rows(6, 6), Slot::ZeroNote, en),
            "Hides row 7. To see it again, use Unhide rows."
        );
        assert_eq!(
            slot(columns(2, 4), Slot::HiddenNote, en),
            "Hidden. Enter a width above 0 to show them."
        );
        assert_eq!(
            slot(columns(2, 2), Slot::HiddenNote, en),
            "Hidden. Enter a width above 0 to show it."
        );
        assert_eq!(
            slot(row, Slot::Refusal(Refusal::OutOfRange), en),
            "Row height must be a number from 0 to 409.5 points. Enter a value in that range."
        );
        assert_eq!(
            slot(columns(1, 3), Slot::Refusal(Refusal::OutOfRange), en),
            "Column width must be a number from 0 to 255 characters. Enter a value in that range."
        );
        let every = rows(0, MAX_ROWS - 1);
        let refusal = "Hiding every row is not supported. Enter a height above 0.";
        assert_eq!(slot(every, Slot::EveryLineHiddenNote, en), refusal);
        assert_eq!(
            slot(every, Slot::Refusal(Refusal::EveryLineHidden), en),
            refusal
        );
    }

    #[test]
    fn the_hint_uses_the_decimal_separator_of_the_interface() {
        assert_eq!(
            slot(rows(0, 0), Slot::Hint, Separator::Comma),
            "0 to 409,5. Enter 0 to hide."
        );
    }

    // The longest target a zero note names, and the largest limit a message prints.
    const LONGEST_TARGET: &str = "1048575:1048576";
    const SLOT_KEYS: [&str; 17] = [
        "linesize.hint",
        "linesize.row.zero.one",
        "linesize.row.zero.other",
        "linesize.column.zero.one",
        "linesize.column.zero.other",
        "linesize.row.hidden.one",
        "linesize.row.hidden.other",
        "linesize.column.hidden.one",
        "linesize.column.hidden.other",
        "linesize.row.range",
        "linesize.column.range",
        "linesize.row.refusal",
        "linesize.column.refusal",
        "linesize.row.scope_all",
        "linesize.column.scope_all",
        "linesize.row.differs",
        "linesize.column.differs",
    ];

    #[test]
    fn every_slot_text_fits_its_two_lines_in_english_and_spanish() {
        for language in [Language::English, Language::Spanish] {
            let catalog = embedded_catalog(language).unwrap();
            for key in SLOT_KEYS {
                let template = catalog.get(key).unwrap();
                let text = template
                    .replace("{target}", LONGEST_TARGET)
                    .replace("{max}", "409,5");
                let characters = text.chars().count();
                assert!(
                    characters <= LINE_BUDGET * SLOT_LINES,
                    "{key} in {language:?} is {characters} characters: {text}"
                );
            }
        }
    }
}
