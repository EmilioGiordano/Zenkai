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
        bind("alt-f4", CloseWindow, CONTEXT),
        bind("ctrl-shift-q", Quit, CONTEXT),
        bind("ctrl-z", Undo, CONTEXT),
        bind("ctrl-y", Redo, CONTEXT),
        bind("ctrl-c", Copy, CONTEXT),
        bind("ctrl-x", Cut, CONTEXT),
        bind("ctrl-v", Paste, CONTEXT),
        bind("ctrl-shift-v", PasteValues, CONTEXT),
        bind("ctrl-shift-v", CycleMarkdownView, Some("MarkdownDocument")),
        bind("f5", ReloadMarkdown, Some("MarkdownDocument")),
        bind("alt-k", KeepMarkdownChanges, Some("MarkdownDocument")),
        bind("pageup", PdfPreviousPage, Some("PdfDocument")),
        bind("pagedown", PdfNextPage, Some("PdfDocument")),
        bind("home", PdfFirstPage, Some("PdfDocument")),
        bind("end", PdfLastPage, Some("PdfDocument")),
        bind("ctrl-a", SelectPdfPage, Some("PdfDocument")),
        bind("alt-1", PdfFitPage, Some("PdfDocument")),
        bind("alt-2", PdfFitWidth, Some("PdfDocument")),
        bind("alt-3", PdfActualSize, Some("PdfDocument")),
        bind("ctrl-0", ResetDocumentZoom, Some("MarkdownDocument")),
        bind("ctrl-0", ResetDocumentZoom, Some("PdfDocument")),
        bind("alt-o", TogglePdfOutline, Some("PdfDocument")),
        bind("ctrl-alt-up", WidenPdfPages, Some("PdfDocument")),
        bind("ctrl-alt-down", NarrowPdfPages, Some("PdfDocument")),
        bind("escape", ClosePdfFind, Some("PdfFindBar")),
        bind("down", PdfOutlineNext, Some("PdfOutline")),
        bind("up", PdfOutlinePrevious, Some("PdfOutline")),
        bind("enter", PdfOutlineOpen, Some("PdfOutline")),
        bind("escape", PdfOutlineClose, Some("PdfOutline")),
        bind("ctrl-b", ToggleBold, CONTEXT),
        bind("ctrl-i", ToggleItalic, CONTEXT),
        bind("ctrl-u", ToggleUnderline, CONTEXT),
        bind("ctrl-5", ToggleStrikethrough, CONTEXT),
        bind("ctrl-1", FormatCells, CONTEXT),
        bind("enter", ApplyNumberFormat, Some("FormatDialog")),
        bind("escape", CloseFormatDialog, Some("FormatDialog")),
        bind(
            "enter",
            ConfirmCloseDialog,
            Some("CloseDialog && !CloseButton"),
        ),
        bind("y", ConfirmCloseDialog, Some("CloseDialog")),
        bind("n", DiscardCloseDialog, Some("CloseDialog")),
        bind("escape", CancelCloseDialog, Some("CloseDialog")),
        bind("tab", FocusNextControl, Some("CloseDialog")),
        bind("shift-tab", FocusPreviousControl, Some("CloseDialog")),
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
        bind("ctrl-·", FormatDate, CONTEXT),
        bind("ctrl-=", ZoomIn, CONTEXT),
        bind("ctrl-+", ZoomIn, CONTEXT),
        bind("ctrl--", ZoomOut, CONTEXT),
        bind("ctrl--", DeleteSelection, Some("Grid")),
        bind("ctrl-+", InsertSelection, Some("Grid")),
        bind("ctrl-shift-=", InsertSelection, Some("Grid")),
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
        bind("ctrl-alt-=", InterfaceLarger, Some("SettingsWindow")),
        bind("ctrl-alt--", InterfaceSmaller, Some("SettingsWindow")),
        bind("ctrl-alt-0", InterfaceReset, Some("SettingsWindow")),
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
        bind("ctrl-alt-shift-j", FocusNotice, CONTEXT),
        bind("escape", DismissNotice, CONTEXT),
        bind("ctrl-c", CopyNotice, Some("NoticeCard")),
        bind("ctrl-shift-j", ShowMessages, CONTEXT),
        bind("escape", CloseMessages, Some("MessagesPanel")),
        bind("down", NextMessage, Some("MessagesPanel")),
        bind("up", PreviousMessage, Some("MessagesPanel")),
        bind("ctrl-c", CopyNotice, Some("MessagesPanel")),
        bind("alt-a", ApplyHeldSettings, CONTEXT),
        bind("alt-y", AllowAgentChange, CONTEXT),
        bind("alt-w", ShowAgentChange, CONTEXT),
        bind("alt-n", DenyAgentChange, CONTEXT),
        bind("alt-]", NextAgentChange, CONTEXT),
        bind("alt-[", PreviousAgentChange, CONTEXT),
        bind("alt-k", KeepAgentChange, CONTEXT),
        bind("alt-j", RejectAgentChange, CONTEXT),
        bind("alt-shift-k", KeepAllAgentChanges, CONTEXT),
        bind("alt-shift-j", RejectAllAgentChanges, CONTEXT),
        bind("ctrl-alt-g", GenerateData, CONTEXT),
        bind("enter", ConfirmGenerate, Some("GenerateDialog > Input")),
        bind("escape", CancelGenerate, Some("GenerateDialog")),
        bind("alt-h", SaveGenerateHeaders, Some("GenerateDialog")),
        bind("alt-n", AddGenerateColumn, Some("GenerateDialog")),
        bind("tab", FocusNextControl, Some("GenerateDialog")),
        bind("shift-tab", FocusPreviousControl, Some("GenerateDialog")),
        bind("ctrl-j", ToggleAgentChat, CONTEXT),
        bind("ctrl-shift-l", FocusAgentChat, CONTEXT),
        bind("ctrl-alt-w", CycleChatLayout, CONTEXT),
        bind("ctrl-alt-f", ToggleChatFullWindow, CONTEXT),
        bind("ctrl-alt-right", WidenSidebar, CONTEXT),
        bind("ctrl-alt-left", NarrowSidebar, CONTEXT),
        bind("ctrl-alt-shift-left", WidenChat, CONTEXT),
        bind("ctrl-alt-shift-right", NarrowChat, CONTEXT),
        bind("left", MoveEdgeLeft, Some("PanelEdge")),
        bind("right", MoveEdgeRight, Some("PanelEdge")),
        bind("enter", RestorePanelWidth, Some("PanelEdge")),
        bind("escape", LeavePanelEdge, Some("PanelEdge")),
        bind("ctrl-l", AddSelectionToChat, CONTEXT),
        bind("ctrl-shift-n", NewAgentConversation, CONTEXT),
        bind("alt-shift-y", AllowChatPermission, CONTEXT),
        bind("alt-shift-n", DenyChatPermission, CONTEXT),
        bind("ctrl-enter", SendChatMessage, Some("AgentChat")),
        bind("escape", StopAgentTurn, Some("AgentChat && Working")),
        bind("escape", LeaveAgentChat, Some("AgentChat && !Working")),
        bind("alt-g", CycleChatAgent, Some("AgentChat")),
        bind("alt-p", CycleChatPermission, Some("AgentChat")),
        bind("alt-c", CopyLoginCommand, Some("AgentChat")),
        bind("alt-shift-r", RetryAgentTurn, Some("AgentChat")),
        bind("alt-shift-a", RestartAgent, Some("AgentChat")),
        bind("ctrl-shift-h", ShowChatSessions, CONTEXT),
        bind("alt-m", PickChatModel, Some("AgentChat")),
        bind("ctrl-.", PickChatPermission, Some("AgentChat")),
        bind("alt-t", CycleToolCallDisplay, Some("AgentChat")),
        bind("enter", ToggleToolCall, Some("ToolRow")),
        bind("space", ToggleToolCall, Some("ToolRow")),
        bind("ctrl-c", CopyToolResult, Some("ToolRow")),
        bind("down", FocusNextToolCall, Some("ToolRow")),
        bind("up", FocusPreviousToolCall, Some("ToolRow")),
        bind("tab", FocusNextToolCall, Some("ToolRow")),
        bind("shift-tab", FocusPreviousToolCall, Some("ToolRow")),
        bind("alt-r", FocusToolCalls, Some("AgentChat")),
        bind("down", SlashNext, Some("Slash > Input")),
        bind("up", SlashPrevious, Some("Slash > Input")),
        bind("tab", SlashAccept, Some("Slash > Input")),
        bind("escape", SlashClose, Some("Slash > Input")),
        bind("escape", CloseChatMenu, Some("AgentChat && Menu")),
        bind("down", ChatMenuNext, Some("Menu > Input")),
        bind("up", ChatMenuPrevious, Some("Menu > Input")),
        bind("down", SessionsNext, Some("ChatSessions > Input")),
        bind("up", SessionsPrevious, Some("ChatSessions > Input")),
        bind("enter", SessionsAccept, Some("ChatSessions > Input")),
        bind("escape", SessionsEscape, Some("ChatSessions > Input")),
        bind("alt-l", ConfirmAgentLaunch, Some("AgentLaunchGate")),
        bind("escape", DeclineAgentLaunch, Some("AgentLaunchGate")),
        bind("ctrl-k", RecordShortcutKeys, Some("SettingsWindow")),
        bind("ctrl-shift-r", ResetAllShortcuts, Some("SettingsWindow")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Action, KeybindingKeystroke, Keystroke, Modifiers};

    use super::*;

    #[test]
    fn every_default_shortcut_parses() {
        let written = include_str!("defaults.rs")
            .lines()
            .filter(|line| line.starts_with("        bind("))
            .count();
        assert_eq!(defaults().len(), written);
    }

    #[test]
    fn ctrl_zero_resets_the_zoom_in_a_markdown_or_pdf_tab_and_hides_columns_elsewhere() {
        let action_in = |context: &str| {
            defaults()
                .into_iter()
                .find(|binding| {
                    binding.chord.as_str() == "ctrl-0" && binding.context == Some(context)
                })
                .map(|binding| binding.action.name())
        };
        assert_eq!(
            action_in("MarkdownDocument"),
            Some(<ResetDocumentZoom as Action>::name_for_type())
        );
        assert_eq!(
            action_in("PdfDocument"),
            Some(<ResetDocumentZoom as Action>::name_for_type())
        );
        assert_eq!(
            action_in("Workspace"),
            Some(<HideColumns as Action>::name_for_type())
        );
    }

    #[test]
    fn the_zoom_keys_are_bound_for_both_plus_spellings() {
        let action_of = |chord: &str| {
            defaults()
                .into_iter()
                .find(|binding| {
                    binding.chord.as_str() == chord && binding.context == Some("Workspace")
                })
                .map(|binding| binding.action.name())
        };
        let zoom_in = Some(<ZoomIn as Action>::name_for_type());
        assert_eq!(action_of("ctrl-="), zoom_in);
        assert_eq!(action_of("ctrl-+"), zoom_in);
        assert_eq!(
            action_of("ctrl--"),
            Some(<ZoomOut as Action>::name_for_type())
        );
    }

    #[test]
    fn the_grid_keeps_ctrl_minus_and_ctrl_plus_for_deleting_and_inserting() {
        let action_of = |chord: &str, context: &str| {
            defaults()
                .into_iter()
                .find(|binding| binding.chord.as_str() == chord && binding.context == Some(context))
                .map(|binding| binding.action.name())
        };
        let delete = Some(<DeleteSelection as Action>::name_for_type());
        let insert = Some(<InsertSelection as Action>::name_for_type());
        assert_eq!(action_of("ctrl--", "Grid"), delete);
        assert_eq!(action_of("ctrl-+", "Grid"), insert);
        assert_eq!(action_of("ctrl-shift-=", "Grid"), insert);
        assert_eq!(
            action_of("ctrl--", "Workspace"),
            Some(<ZoomOut as Action>::name_for_type())
        );
    }

    #[test]
    fn the_close_dialog_accepts_y_to_save_and_n_to_discard() {
        let dialog_action = |chord: &str| {
            defaults()
                .into_iter()
                .find(|binding| {
                    binding.chord.as_str() == chord
                        && binding
                            .context
                            .is_some_and(|context| context.starts_with("CloseDialog"))
                })
                .map(|binding| binding.action.name())
        };
        assert_eq!(
            dialog_action("y"),
            Some(<ConfirmCloseDialog as Action>::name_for_type())
        );
        assert_eq!(
            dialog_action("n"),
            Some(<DiscardCloseDialog as Action>::name_for_type())
        );
    }

    // Windows reports Shift+digit as the shifted character with no shift flag, so the
    // chord must name that character. The symbols are what each layout types.
    #[test]
    fn every_number_format_has_a_chord_windows_types_on_the_us_and_spanish_layouts() {
        let typed = |key: &str| Keystroke {
            modifiers: Modifiers::control(),
            key: key.to_string(),
            key_char: Some(key.to_string()),
        };
        let matches = |key: &str, action: &str| {
            defaults().into_iter().any(|binding| {
                binding.action.name() == action
                    && Keystroke::parse(binding.chord.as_str()).is_ok_and(|chord| {
                        typed(key).should_match(&KeybindingKeystroke::from_keystroke(chord))
                    })
            })
        };
        let cases = [
            ("~", <FormatGeneral as Action>::name_for_type()),
            ("!", <FormatNumber as Action>::name_for_type()),
            ("$", <FormatCurrency as Action>::name_for_type()),
            ("%", <FormatPercent as Action>::name_for_type()),
            ("#", <FormatDate as Action>::name_for_type()),
            ("·", <FormatDate as Action>::name_for_type()),
        ];
        for (key, action) in cases {
            assert!(matches(key, action), "Ctrl+{key} does not reach {action}");
        }
    }
}
