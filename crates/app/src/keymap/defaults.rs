use gpui_kit::Action;

use super::chord::Chord;
use crate::actions::*;

const CONTEXT: Option<&str> = Some("Workspace");

pub struct Binding {
    pub chord: Chord,
    pub action: Box<dyn Action>,
    pub context: Option<&'static str>,
}

fn bind(keys: &'static str, action: impl Action, context: Option<&'static str>) -> Option<Binding> {
    match Chord::parse(keys) {
        Ok(chord) => Some(Binding {
            chord,
            action: Box::new(action),
            context,
        }),
        Err(error) => {
            tracing::error!(?error, keys, "a default shortcut does not parse");
            None
        }
    }
}

pub fn defaults() -> Vec<Binding> {
    [
        bind("ctrl-n", NewWorkbook, CONTEXT),
        bind("ctrl-o", Open, CONTEXT),
        bind("ctrl-s", Save, CONTEXT),
        bind("f12", SaveAs, CONTEXT),
        bind("ctrl-shift-s", SaveAs, CONTEXT),
        bind("alt-f4", Quit, CONTEXT),
        bind("ctrl-z", Undo, CONTEXT),
        bind("ctrl-y", Redo, CONTEXT),
        bind("ctrl-c", Copy, CONTEXT),
        bind("ctrl-x", Cut, CONTEXT),
        bind("ctrl-v", Paste, CONTEXT),
        bind("ctrl-shift-v", PasteValues, CONTEXT),
        bind("ctrl-b", ToggleBold, CONTEXT),
        bind("ctrl-i", ToggleItalic, CONTEXT),
        bind("ctrl-u", ToggleUnderline, CONTEXT),
        bind("ctrl-5", ToggleStrikethrough, CONTEXT),
        bind("ctrl-1", FormatCells, CONTEXT),
        bind("enter", ApplyNumberFormat, Some("FormatDialog")),
        bind("escape", CloseFormatDialog, Some("FormatDialog")),
        bind("ctrl-`", ToggleFormulas, CONTEXT),
        bind("ctrl-*", SelectCurrentRegion, CONTEXT),
        bind("ctrl-&", BordersOutside, CONTEXT),
        bind("ctrl-_", BordersNone, CONTEXT),
        bind("ctrl->", GrowFont, CONTEXT),
        bind("ctrl-<", ShrinkFont, CONTEXT),
        bind("ctrl-~", FormatGeneral, CONTEXT),
        bind("ctrl-!", FormatNumber, CONTEXT),
        bind("ctrl-$", FormatCurrency, CONTEXT),
        bind("ctrl-%", FormatPercent, CONTEXT),
        bind("ctrl-#", FormatDate, CONTEXT),
        bind("ctrl-=", ZoomIn, CONTEXT),
        bind("ctrl-+", ZoomIn, CONTEXT),
        bind("ctrl--", ZoomOut, CONTEXT),
        bind("ctrl-9", HideRows, CONTEXT),
        bind("ctrl-0", HideColumns, CONTEXT),
        bind("ctrl-(", UnhideRows, CONTEXT),
        bind("ctrl-)", UnhideColumns, CONTEXT),
        bind("ctrl-alt-=", InterfaceLarger, CONTEXT),
        bind("ctrl-alt--", InterfaceSmaller, CONTEXT),
        bind("ctrl-alt-0", InterfaceReset, CONTEXT),
        bind("ctrl-alt-m", ToggleReduceMotion, CONTEXT),
        bind("ctrl-tab", NextDocument, CONTEXT),
        bind("ctrl-shift-tab", PreviousDocument, CONTEXT),
        bind("ctrl-w", CloseDocument, CONTEXT),
        bind("ctrl-shift-t", ReopenClosedDocument, CONTEXT),
        bind("ctrl-alt-b", ToggleSidebar, CONTEXT),
        bind("ctrl-e", SearchFiles, CONTEXT),
        bind("f6", FocusSidebar, CONTEXT),
        bind("ctrl-alt-r", RenameSpace, CONTEXT),
        bind("ctrl-alt-k", CycleSpaceColor, CONTEXT),
        bind("ctrl-alt-shift-k", CustomizeSpace, CONTEXT),
        bind("ctrl-alt-shift-r", ResetSpaceAppearance, CONTEXT),
        bind("escape", CloseSpacePanel, Some("SpacePanel")),
        bind("tab", FocusNextControl, Some("SpacePanel")),
        bind("shift-tab", FocusPreviousControl, Some("SpacePanel")),
        bind(
            "ctrl-shift-delete",
            DeleteFile,
            Some("Sidebar && !SpaceRename"),
        ),
        bind("ctrl-alt-shift-d", DeleteSpace, CONTEXT),
        bind("ctrl-alt-n", NewSpace, CONTEXT),
        bind("ctrl-alt-pageup", MoveToPreviousSpace, CONTEXT),
        bind("ctrl-alt-pagedown", MoveToNextSpace, CONTEXT),
        bind("up", SidebarUp, Some("Sidebar && !SpaceRename")),
        bind("down", SidebarDown, Some("Sidebar && !SpaceRename")),
        bind("home", SidebarFirst, Some("Sidebar && !SpaceRename")),
        bind("end", SidebarLast, Some("Sidebar && !SpaceRename")),
        bind("left", SidebarCollapse, Some("Sidebar && !SpaceRename")),
        bind("right", SidebarExpand, Some("Sidebar && !SpaceRename")),
        bind("enter", SidebarOpen, Some("Sidebar && !SpaceRename")),
        bind("escape", LeaveSidebar, Some("Sidebar && !SpaceRename")),
        bind("f2", RenameSpace, Some("Sidebar && !SpaceRename")),
        bind("delete", SidebarDelete, Some("Sidebar && !SpaceRename")),
        bind(
            "alt-up",
            MoveToPreviousSpace,
            Some("Sidebar && !SpaceRename"),
        ),
        bind("alt-down", MoveToNextSpace, Some("Sidebar && !SpaceRename")),
        bind("escape", CloseSpaceRename, Some("SpaceRename")),
        bind("ctrl-pagedown", NextSheet, CONTEXT),
        bind("ctrl-pageup", PreviousSheet, CONTEXT),
        bind("shift-f11", NewSheet, CONTEXT),
        bind("ctrl-shift-d", ToggleDiagnostics, CONTEXT),
        bind("alt-f1", InsertChart, CONTEXT),
        bind("ctrl-f", Find, CONTEXT),
        bind("ctrl-h", Replace, CONTEXT),
        bind("ctrl-shift-p", TogglePalette, CONTEXT),
        bind("escape", CloseRename, Some("RenameBar")),
        bind("ctrl-g", GoTo, CONTEXT),
        bind("enter", ConfirmCsvImport, Some("CsvPreview")),
        bind("escape", CancelCsvImport, Some("CsvPreview")),
        bind("f5", GoTo, CONTEXT),
        bind("ctrl-d", FillDown, CONTEXT),
        bind("ctrl-r", FillRight, CONTEXT),
        bind("alt-=", AutoSum, CONTEXT),
        bind("ctrl-;", InsertDate, CONTEXT),
        bind("ctrl-shift-;", InsertTime, CONTEXT),
        bind("ctrl-:", InsertTime, CONTEXT),
        bind("escape", CloseGoTo, Some("NameBox")),
        bind("escape", CloseFind, Some("FindBar")),
        bind("escape", CancelFormulaBar, Some("FormulaBar")),
        bind("escape", ClosePalette, Some("Palette")),
        bind("escape", CancelThemePicker, Some("ThemePicker")),
        bind("ctrl-alt-t", SelectTheme, CONTEXT),
        bind("escape", CloseSearch, Some("SearchOverlay")),
        bind("ctrl-,", OpenSettings, CONTEXT),
        bind("ctrl-,", OpenSettings, Some("SettingsWindow")),
        bind("escape", CloseSettings, Some("SettingsWindow")),
        bind("ctrl-f", FocusSettingsSearch, Some("SettingsWindow")),
        bind("ctrl-shift-m", ToggleModifiedOnly, Some("SettingsWindow")),
        bind("ctrl-pagedown", NextSettingsSection, Some("SettingsWindow")),
        bind(
            "ctrl-pageup",
            PreviousSettingsSection,
            Some("SettingsWindow"),
        ),
        bind("f5", DetectAgents, Some("SettingsWindow")),
        bind("alt-c", AddClaudeAgent, Some("SettingsWindow")),
        bind("alt-g", AddGeminiAgent, Some("SettingsWindow")),
        bind("alt-x", AddCodexAgent, Some("SettingsWindow")),
        bind("alt-r", PermissionReadOnly, Some("SettingsWindow")),
        bind("alt-a", PermissionAskBeforeWrite, Some("SettingsWindow")),
        bind("alt-u", PermissionAutomatic, Some("SettingsWindow")),
        bind("alt-e", ToggleExternalAgents, Some("SettingsWindow")),
        bind("tab", FocusNextControl, Some("SettingsWindow")),
        bind("shift-tab", FocusPreviousControl, Some("SettingsWindow")),
        bind("alt-d", CycleDefaultAgent, Some("SettingsWindow")),
        bind("alt-s", SaveSecrets, Some("SettingsWindow")),
        bind("alt-m", CopyClaudeCommand, Some("SettingsWindow")),
        bind(
            "alt-a",
            ApplyHeldSettings,
            Some("SettingsWindow && HeldPending"),
        ),
        bind(
            "alt-k",
            KeepCurrentSettings,
            Some("SettingsWindow && HeldPending"),
        ),
        bind("enter", KeepCurrentSettings, Some("HeldSettings")),
        bind("escape", KeepCurrentSettings, Some("HeldSettings")),
        bind("alt-a", ApplyHeldSettings, Some("HeldSettings")),
        bind("enter", DenyAgentChange, Some("AgentApproval")),
        bind("alt-y", AllowAgentChange, Some("AgentApproval")),
        bind("escape", DenyAgentChange, Some("AgentApproval")),
        bind("ctrl-shift-e", LetAgentsEdit, CONTEXT),
        bind("alt-a", ApplyHeldSettings, CONTEXT),
        bind("alt-y", AllowAgentChange, CONTEXT),
        bind("alt-w", ShowAgentChange, CONTEXT),
        bind("alt-n", DenyAgentChange, CONTEXT),
        bind("ctrl-alt-g", GenerateData, CONTEXT),
        bind("enter", ConfirmGenerate, Some("GenerateDialog > Input")),
        bind("escape", CancelGenerate, Some("GenerateDialog")),
        bind("alt-h", SaveGenerateHeaders, Some("GenerateDialog")),
        bind("alt-n", AddGenerateColumn, Some("GenerateDialog")),
        bind("tab", FocusNextControl, Some("GenerateDialog")),
        bind("shift-tab", FocusPreviousControl, Some("GenerateDialog")),
        bind("ctrl-k", RecordShortcutKeys, Some("SettingsWindow")),
        bind("ctrl-shift-r", ResetAllShortcuts, Some("SettingsWindow")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_shortcut_parses() {
        let written = include_str!("defaults.rs")
            .lines()
            .filter(|line| line.starts_with("        bind("))
            .count();
        assert_eq!(defaults().len(), written);
    }
}
