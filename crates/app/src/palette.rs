use gpui_kit::component::command::{CommandGroup, CommandItem};
use gpui_kit::*;
use zenkai_i18n::t;

use crate::actions::*;

pub struct Entry {
    pub label: &'static str,
    pub action: Box<dyn Action>,
}

pub struct Group {
    pub label: &'static str,
    pub entries: Vec<Entry>,
}

fn item(label: &'static str, action: impl Action) -> Entry {
    Entry {
        label,
        action: Box::new(action),
    }
}

fn group(label: &'static str, entries: impl IntoIterator<Item = Entry>) -> Group {
    Group {
        label,
        entries: entries.into_iter().collect(),
    }
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
            .label(t!("palette.open_recent", name = crate::recent::label(path)))
            .action(action)
    });
    std::iter::once(
        CommandGroup::new()
            .label(t!("palette.group.recent"))
            .items(recent_items),
    )
    .chain(table().into_iter().map(|group| {
        CommandGroup::new().label(group.label).items(
            group
                .entries
                .into_iter()
                .map(|entry| CommandItem::new().label(entry.label).action(entry.action)),
        )
    }))
    .collect()
}

pub fn table() -> Vec<Group> {
    vec![
        group(
            t!("palette.group.file"),
            [
                item(t!("palette.new_workbook"), NewWorkbook),
                item(t!("palette.open"), Open),
                item(t!("palette.close_workbook"), CloseDocument),
                item(t!("palette.reopen_workbook"), ReopenClosedDocument),
                item(t!("palette.search_files"), SearchFiles),
                item(t!("palette.new_space"), NewSpace),
                item(t!("palette.rename_space"), RenameSpace),
                item(t!("palette.delete_space"), DeleteSpace),
                item(t!("palette.move_previous_space"), MoveToPreviousSpace),
                item(t!("palette.move_next_space"), MoveToNextSpace),
                item(t!("palette.next_workbook"), NextDocument),
                item(t!("palette.previous_workbook"), PreviousDocument),
                item(t!("palette.save"), Save),
                item(t!("palette.save_as"), SaveAs),
            ],
        ),
        group(
            t!("palette.group.edit"),
            [
                item(t!("palette.undo"), Undo),
                item(t!("palette.redo"), Redo),
                item(t!("palette.copy"), Copy),
                item(t!("palette.cut"), Cut),
                item(t!("palette.paste"), Paste),
                item(t!("palette.paste_values"), PasteValues),
                item(t!("palette.find"), Find),
                item(t!("palette.replace"), Replace),
                item(t!("palette.fill_down"), FillDown),
                item(t!("palette.fill_right"), FillRight),
                item(t!("palette.clear_formats"), ClearFormats),
                item(t!("palette.clear_all"), ClearAll),
                item(t!("palette.sort_ascending"), SortAscending),
                item(t!("palette.sort_descending"), SortDescending),
                item(t!("palette.autosum"), AutoSum),
                item(t!("palette.insert_date"), InsertDate),
                item(t!("palette.insert_time"), InsertTime),
                item(t!("palette.go_to"), GoTo),
                item(t!("palette.select_region"), SelectCurrentRegion),
            ],
        ),
        group(
            t!("palette.group.format"),
            [
                item(t!("palette.bold"), ToggleBold),
                item(t!("palette.italic"), ToggleItalic),
                item(t!("palette.underline"), ToggleUnderline),
                item(t!("palette.strikethrough"), ToggleStrikethrough),
                item(t!("palette.grow_font"), GrowFont),
                item(t!("palette.shrink_font"), ShrinkFont),
                item(t!("palette.no_fill"), NoFill),
                item(t!("palette.borders_all"), BordersAll),
                item(t!("palette.borders_outside"), BordersOutside),
                item(t!("palette.border_bottom"), BorderBottom),
                item(t!("palette.borders_none"), BordersNone),
                item(t!("palette.automatic_font_color"), AutomaticFontColor),
                item(t!("palette.align_left"), AlignLeft),
                item(t!("palette.align_center"), AlignCenter),
                item(t!("palette.align_right"), AlignRight),
                item(t!("palette.wrap_text"), ToggleWrapText),
                item(t!("palette.format_general"), FormatGeneral),
                item(t!("palette.format_number"), FormatNumber),
                item(t!("palette.format_currency"), FormatCurrency),
                item(t!("palette.format_percent"), FormatPercent),
                item(t!("palette.format_date"), FormatDate),
                item(t!("palette.format_cells"), FormatCells),
                item(t!("palette.increase_decimal"), IncreaseDecimal),
                item(t!("palette.decrease_decimal"), DecreaseDecimal),
            ],
        ),
        group(
            t!("palette.group.insert"),
            [
                item(t!("palette.generate_data"), GenerateData),
                item(t!("palette.insert_chart"), InsertChart),
                item(t!("palette.insert_rows"), InsertRows),
                item(t!("palette.hide_rows"), HideRows),
                item(t!("palette.unhide_rows"), UnhideRows),
                item(t!("palette.hide_columns"), HideColumns),
                item(t!("palette.unhide_columns"), UnhideColumns),
                item(t!("palette.insert_columns"), InsertColumns),
                item(t!("palette.delete_rows"), DeleteRows),
                item(t!("palette.delete_columns"), DeleteColumns),
                item(t!("palette.new_sheet"), NewSheet),
                item(t!("palette.rename_sheet"), RenameSheet),
                item(t!("palette.delete_sheet"), DeleteSheet),
                item(t!("palette.move_sheet_left"), MoveSheetLeft),
                item(t!("palette.move_sheet_right"), MoveSheetRight),
                item(t!("palette.duplicate_sheet"), DuplicateSheet),
            ],
        ),
        group(
            t!("palette.group.view"),
            [
                item(t!("palette.zoom_in"), ZoomIn),
                item(t!("palette.zoom_out"), ZoomOut),
                item(t!("palette.zoom_reset"), ZoomReset),
                item(t!("palette.toggle_formulas"), ToggleFormulas),
                item(t!("palette.interface_larger"), InterfaceLarger),
                item(t!("palette.interface_smaller"), InterfaceSmaller),
                item(t!("palette.interface_reset"), InterfaceReset),
                item(t!("palette.toggle_reduce_motion"), ToggleReduceMotion),
                item(t!("palette.freeze_panes"), FreezePanes),
                item(t!("palette.freeze_top_row"), FreezeTopRow),
                item(t!("palette.freeze_first_column"), FreezeFirstColumn),
                item(t!("palette.next_sheet"), NextSheet),
                item(t!("palette.previous_sheet"), PreviousSheet),
                item(t!("palette.select_theme"), SelectTheme),
                item(t!("palette.toggle_theme"), ToggleTheme),
                item(t!("palette.toggle_diagnostics"), ToggleDiagnostics),
                item(t!("palette.toggle_sidebar"), ToggleSidebar),
                item(t!("palette.cycle_space_color"), CycleSpaceColor),
                item(t!("palette.customize_space"), CustomizeSpace),
                item(t!("palette.reset_space_appearance"), ResetSpaceAppearance),
                item(t!("palette.delete_file"), DeleteFile),
                item(t!("palette.focus_sidebar"), FocusSidebar),
            ],
        ),
        group(
            t!("palette.group.agents"),
            [
                item(t!("palette.settings"), OpenSettings),
                item(t!("palette.show_agent_change"), ShowAgentChange),
                item(t!("palette.next_agent_change"), NextAgentChange),
                item(t!("palette.previous_agent_change"), PreviousAgentChange),
                item(t!("palette.keep_agent_change"), KeepAgentChange),
                item(t!("palette.reject_agent_change"), RejectAgentChange),
                item(t!("palette.keep_all_agent_changes"), KeepAllAgentChanges),
                item(
                    t!("palette.reject_all_agent_changes"),
                    RejectAllAgentChanges,
                ),
                item(t!("palette.detect_agents"), DetectAgents),
                item(t!("palette.add_claude"), AddClaudeAgent),
                item(t!("palette.add_gemini"), AddGeminiAgent),
                item(t!("palette.add_codex"), AddCodexAgent),
                item(t!("palette.permission_read_only"), PermissionReadOnly),
                item(t!("palette.permission_ask"), PermissionAskBeforeWrite),
                item(t!("palette.permission_automatic"), PermissionAutomatic),
                item(t!("palette.toggle_external_agents"), ToggleExternalAgents),
                item(t!("palette.copy_claude_command"), CopyClaudeCommand),
                item(t!("palette.cycle_default_agent"), CycleDefaultAgent),
                item(t!("palette.apply_held_settings"), ApplyHeldSettings),
                item(t!("palette.keep_current_settings"), KeepCurrentSettings),
                item(t!("palette.allow_agent_change"), AllowAgentChange),
                item(t!("palette.deny_agent_change"), DenyAgentChange),
                item(t!("palette.let_agents_edit"), LetAgentsEdit),
            ],
        ),
    ]
}
