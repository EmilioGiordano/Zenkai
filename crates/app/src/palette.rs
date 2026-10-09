use gpui_kit::component::command::{CommandGroup, CommandItem};
use gpui_kit::*;

use crate::actions::*;

fn item(label: &'static str, action: impl Action) -> CommandItem {
    CommandItem::new().label(label).action(Box::new(action))
}

pub fn groups(recent: &[std::path::PathBuf]) -> Vec<CommandGroup> {
    let open_recent: [Box<dyn Action>; crate::recent::MAX_RECENT] = [
        Box::new(OpenRecent1),
        Box::new(OpenRecent2),
        Box::new(OpenRecent3),
        Box::new(OpenRecent4),
        Box::new(OpenRecent5),
    ];
    let recent_items = recent.iter().zip(open_recent).map(|(path, action)| {
        CommandItem::new()
            .label(format!("Open recent: {}", crate::recent::label(path)))
            .action(action)
    });
    vec![
        CommandGroup::new().label("Recent").items(recent_items),
        CommandGroup::new().label("File").items([
            item("New workbook", NewWorkbook),
            item("Open…", Open),
            item("Close workbook", CloseDocument),
            item("Reopen closed workbook", ReopenClosedDocument),
            item(
                "Search files, recent files and sheets (replaces Excel's Flash Fill, Ctrl+E)",
                SearchFiles,
            ),
            item("New space", NewSpace),
            item("Rename space", RenameSpace),
            item("Delete space", DeleteSpace),
            item("Move workbook to the previous space", MoveToPreviousSpace),
            item("Move workbook to the next space", MoveToNextSpace),
            item("Next workbook", NextDocument),
            item("Previous workbook", PreviousDocument),
            item("Save", Save),
            item("Save As…", SaveAs),
        ]),
        CommandGroup::new().label("Edit").items([
            item("Undo", Undo),
            item("Redo", Redo),
            item("Copy", Copy),
            item("Cut", Cut),
            item("Paste", Paste),
            item("Paste values", PasteValues),
            item("Find", Find),
            item("Replace…", Replace),
            item("Fill down", FillDown),
            item("Fill right", FillRight),
            item("Clear formats", ClearFormats),
            item("Clear all (contents and formats)", ClearAll),
            item("Sort A to Z (by the active cell's column)", SortAscending),
            item("Sort Z to A (by the active cell's column)", SortDescending),
            item("AutoSum", AutoSum),
            item("Insert today's date", InsertDate),
            item("Insert the current time", InsertTime),
            item("Go to…", GoTo),
            item(
                "Select the current region (data block)",
                SelectCurrentRegion,
            ),
        ]),
        CommandGroup::new().label("Format").items([
            item("Bold", ToggleBold),
            item("Italic", ToggleItalic),
            item("Underline", ToggleUnderline),
            item("Strikethrough", ToggleStrikethrough),
            item("Increase font size", GrowFont),
            item("Decrease font size", ShrinkFont),
            item("No fill", NoFill),
            item("All borders", BordersAll),
            item("Outside borders", BordersOutside),
            item("Bottom border", BorderBottom),
            item("No border", BordersNone),
            item("Automatic font colour", AutomaticFontColor),
            item("Align left", AlignLeft),
            item("Center", AlignCenter),
            item("Align right", AlignRight),
            item("Wrap text", ToggleWrapText),
            item("General number format", FormatGeneral),
            item("Number format", FormatNumber),
            item("Currency format", FormatCurrency),
            item("Percent format", FormatPercent),
            item("Date format", FormatDate),
            item("Format cells (number format)…", FormatCells),
            item("Increase decimal", IncreaseDecimal),
            item("Decrease decimal", DecreaseDecimal),
        ]),
        CommandGroup::new().label("Insert").items([
            item("Chart of the selection", InsertChart),
            item("Insert rows above", InsertRows),
            item("Hide rows", HideRows),
            item("Unhide rows in the selection", UnhideRows),
            item("Hide columns", HideColumns),
            item("Unhide columns in the selection", UnhideColumns),
            item("Insert columns to the left", InsertColumns),
            item("Delete selected rows", DeleteRows),
            item("Delete selected columns", DeleteColumns),
            item("New sheet", NewSheet),
            item("Rename sheet", RenameSheet),
            item("Delete sheet", DeleteSheet),
            item("Move sheet left", MoveSheetLeft),
            item("Move sheet right", MoveSheetRight),
            item("Duplicate sheet", DuplicateSheet),
        ]),
        CommandGroup::new().label("View").items([
            item("Zoom in", ZoomIn),
            item("Zoom out", ZoomOut),
            item("Reset zoom", ZoomReset),
            item("Show formulas or values", ToggleFormulas),
            item("Larger interface (menus, bars, dialogs)", InterfaceLarger),
            item("Smaller interface", InterfaceSmaller),
            item("Reset interface size", InterfaceReset),
            item(
                "Reduce motion on or off (spinners, dialog animations)",
                ToggleReduceMotion,
            ),
            item("Freeze or unfreeze panes at the active cell", FreezePanes),
            item("Freeze top row", FreezeTopRow),
            item("Freeze first column", FreezeFirstColumn),
            item("Next sheet", NextSheet),
            item("Previous sheet", PreviousSheet),
            item("Theme: light, dark, high contrast", ToggleTheme),
            item("Diagnostics in the status bar", ToggleDiagnostics),
            item("Show or hide the sidebar", ToggleSidebar),
            item("Move focus between the grid and the sidebar", FocusSidebar),
        ]),
        CommandGroup::new().label("Agents").items([
            item("Settings…", OpenSettings),
            item("Detect installed agents", DetectAgents),
            item("Add the Claude agent", AddClaudeAgent),
            item("Add the Gemini CLI agent", AddGeminiAgent),
            item("Add the Codex agent", AddCodexAgent),
            item("Agents: read only", PermissionReadOnly),
            item("Agents: ask before writing", PermissionAskBeforeWrite),
            item("Agents: write without asking", PermissionAutomatic),
            item("Allow or block external agents", ToggleExternalAgents),
            item(
                "Copy the command that adds Zenkai to Claude Code",
                CopyClaudeCommand,
            ),
            item(
                "Make the next configured agent the default",
                CycleDefaultAgent,
            ),
            item(
                "Apply the settings.json change waiting for you",
                ApplyHeldSettings,
            ),
            item("Keep the current agent settings", KeepCurrentSettings),
            item("Allow the change an agent asked for", AllowAgentChange),
            item("Deny the change an agent asked for", DenyAgentChange),
            item(
                "Let agents edit this file (leave Protected View)",
                LetAgentsEdit,
            ),
        ]),
    ]
}
