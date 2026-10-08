use gpui_kit::component::command::{CommandGroup, CommandItem};
use gpui_kit::*;

use crate::actions::*;

fn item(label: &'static str, action: impl Action) -> CommandItem {
    CommandItem::new().label(label).action(Box::new(action))
}

pub fn groups() -> Vec<CommandGroup> {
    vec![
        CommandGroup::new().label("File").items([
            item("New workbook", NewWorkbook),
            item("Open…", Open),
            item("Save", Save),
            item("Save As…", SaveAs),
        ]),
        CommandGroup::new().label("Edit").items([
            item("Undo", Undo),
            item("Redo", Redo),
            item("Copy", Copy),
            item("Cut", Cut),
            item("Paste", Paste),
            item("Find", Find),
        ]),
        CommandGroup::new().label("Format").items([
            item("Bold", ToggleBold),
            item("Italic", ToggleItalic),
            item("Underline", ToggleUnderline),
            item("Align left", AlignLeft),
            item("Center", AlignCenter),
            item("Align right", AlignRight),
            item("General number format", FormatGeneral),
            item("Number format", FormatNumber),
            item("Currency format", FormatCurrency),
            item("Percent format", FormatPercent),
            item("Date format", FormatDate),
        ]),
        CommandGroup::new().label("Insert").items([
            item("Chart of the selection", InsertChart),
            item("New sheet", NewSheet),
            item("Rename sheet", RenameSheet),
            item("Delete sheet", DeleteSheet),
            item("Move sheet left", MoveSheetLeft),
            item("Move sheet right", MoveSheetRight),
        ]),
        CommandGroup::new().label("View").items([
            item("Zoom in", ZoomIn),
            item("Zoom out", ZoomOut),
            item("Reset zoom", ZoomReset),
            item("Next sheet", NextSheet),
            item("Previous sheet", PreviousSheet),
            item("Light or dark theme", ToggleTheme),
            item("Diagnostics in the status bar", ToggleDiagnostics),
        ]),
    ]
}
