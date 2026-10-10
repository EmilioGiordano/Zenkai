use zenkai_agent::settings::{ExternalAgents, PermissionMode, Settings};
use zenkai_i18n::t;

use crate::space_appearance::SpaceAppearance;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    General,
    Appearance,
    Keyboard,
    Ai,
    Files,
    Privacy,
    About,
}

impl Section {
    pub const ALL: [Section; 7] = [
        Section::General,
        Section::Appearance,
        Section::Keyboard,
        Section::Ai,
        Section::Files,
        Section::Privacy,
        Section::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::General => t!("settings.section.general"),
            Section::Appearance => t!("settings.section.appearance"),
            Section::Keyboard => t!("settings.section.keyboard"),
            Section::Ai => t!("settings.section.ai"),
            Section::Files => t!("settings.section.files"),
            Section::Privacy => t!("settings.section.privacy"),
            Section::About => t!("settings.section.about"),
        }
    }

    pub fn lead(self) -> &'static str {
        match self {
            Section::General => t!("settings.lead.general"),
            Section::Appearance => t!("settings.lead.appearance"),
            Section::Keyboard => t!("settings.lead.keyboard"),
            Section::Ai => t!("settings.lead.ai"),
            Section::Files => t!("settings.lead.files"),
            Section::Privacy => t!("settings.lead.privacy"),
            Section::About => t!("settings.lead.about"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowId {
    RestoreSession,
    Language,
    Autosave,
    ColorMode,
    DarkTheme,
    LightTheme,
    SpaceStyle,
    SpaceIntensity,
    SpaceTint,
    NewSpaceColor,
    Agents,
    Permission,
    ConfirmElevated,
    ExternalAgents,
    ConnectionCommand,
    WhatLeaves,
    RecoveryFolder,
    SettingsFile,
    FastXlsxReader,
    LogsFolder,
    Version,
    License,
}

pub struct RowInfo {
    pub section: Section,
    pub group: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub keywords: &'static [&'static str],
}

impl RowId {
    pub const ALL: [RowId; 22] = [
        RowId::RestoreSession,
        RowId::Language,
        RowId::Autosave,
        RowId::ColorMode,
        RowId::DarkTheme,
        RowId::LightTheme,
        RowId::SpaceStyle,
        RowId::SpaceIntensity,
        RowId::SpaceTint,
        RowId::NewSpaceColor,
        RowId::Agents,
        RowId::Permission,
        RowId::ConfirmElevated,
        RowId::ExternalAgents,
        RowId::ConnectionCommand,
        RowId::WhatLeaves,
        RowId::RecoveryFolder,
        RowId::SettingsFile,
        RowId::FastXlsxReader,
        RowId::LogsFolder,
        RowId::Version,
        RowId::License,
    ];

    pub fn info(self) -> RowInfo {
        let row = |section, group, title, description, keywords| RowInfo {
            section,
            group,
            title,
            description,
            keywords,
        };
        match self {
            RowId::RestoreSession => row(
                Section::General,
                t!("settings.group.startup"),
                t!("settings.row.restore_session"),
                t!("settings.row.restore_session.description"),
                &["start", "reopen", "recover"],
            ),
            RowId::Language => row(
                Section::General,
                t!("settings.group.language"),
                t!("settings.row.language"),
                t!("settings.row.language.description"),
                &["english", "spanish", "espanol", "locale"],
            ),
            RowId::Autosave => row(
                Section::General,
                t!("settings.group.saving"),
                t!("settings.row.autosave"),
                t!("settings.row.autosave.description"),
                &["recovery", "backup", "seconds", "interval"],
            ),
            RowId::ColorMode => row(
                Section::Appearance,
                t!("settings.group.theme"),
                t!("settings.row.color_mode"),
                t!("settings.row.color_mode.description"),
                &["light", "dark", "system", "mode"],
            ),
            RowId::DarkTheme => row(
                Section::Appearance,
                t!("settings.group.theme"),
                t!("settings.row.dark_theme"),
                t!("settings.row.theme.description"),
                &["contrast", "color"],
            ),
            RowId::LightTheme => row(
                Section::Appearance,
                t!("settings.group.theme"),
                t!("settings.row.light_theme"),
                t!("settings.row.theme.description"),
                &["color"],
            ),
            RowId::SpaceStyle => row(
                Section::Appearance,
                t!("settings.group.spaces"),
                t!("settings.row.space_style"),
                t!("settings.row.space_style.description"),
                &["dot", "header", "border", "tint", "sidebar"],
            ),
            RowId::SpaceIntensity => row(
                Section::Appearance,
                t!("settings.group.spaces"),
                t!("settings.row.space_intensity"),
                t!("settings.row.space_intensity.description"),
                &["space", "color", "strength"],
            ),
            RowId::SpaceTint => row(
                Section::Appearance,
                t!("settings.group.spaces"),
                t!("settings.row.space_tint"),
                t!("settings.row.space_tint.description"),
                &["space", "color", "apply"],
            ),
            RowId::NewSpaceColor => row(
                Section::Appearance,
                t!("settings.group.spaces"),
                t!("settings.row.new_space_color"),
                t!("settings.row.new_space_color.description"),
                &["space", "palette", "rotation"],
            ),
            RowId::Agents => row(
                Section::Ai,
                t!("settings.group.agents"),
                t!("settings.row.agents"),
                t!("settings.row.agents.description"),
                &[
                    "claude",
                    "gemini",
                    "codex",
                    "openai",
                    "anthropic",
                    "google",
                    "key",
                    "secret",
                ],
            ),
            RowId::Permission => row(
                Section::Ai,
                t!("settings.group.permissions"),
                t!("settings.row.permission"),
                t!("settings.row.permission.description"),
                &["read only", "ask", "automatic", "write"],
            ),
            RowId::ConfirmElevated => row(
                Section::Ai,
                t!("settings.group.permissions"),
                t!("settings.row.confirm_elevated"),
                t!("settings.row.confirm_elevated.description"),
                &["safety", "startup", "automatic"],
            ),
            RowId::ExternalAgents => row(
                Section::Ai,
                t!("settings.group.external_agents"),
                t!("settings.row.external_agents"),
                t!("settings.row.external_agents.description"),
                &["mcp", "claude code", "bridge"],
            ),
            RowId::ConnectionCommand => row(
                Section::Ai,
                t!("settings.group.external_agents"),
                t!("settings.row.connection_command"),
                t!("settings.row.connection_command.description"),
                &["mcp", "copy", "claude code"],
            ),
            RowId::WhatLeaves => row(
                Section::Ai,
                t!("settings.group.privacy"),
                t!("settings.row.what_leaves"),
                t!("settings.row.what_leaves.description"),
                &["privacy", "data", "upload", "cloud"],
            ),
            RowId::RecoveryFolder => row(
                Section::Files,
                t!("settings.group.recovery"),
                t!("settings.row.recovery_folder"),
                t!("settings.row.recovery_folder.description"),
                &["autosave", "backup", "crash"],
            ),
            RowId::SettingsFile => row(
                Section::Files,
                t!("settings.group.settings"),
                t!("settings.row.settings_file"),
                t!("settings.row.settings_file.description"),
                &["json", "config"],
            ),
            RowId::FastXlsxReader => row(
                Section::Files,
                t!("settings.group.advanced"),
                t!("settings.row.fast_xlsx_reader"),
                t!("settings.row.fast_xlsx_reader.description"),
                &["xlsx", "open", "speed", "reader", "advanced"],
            ),
            RowId::LogsFolder => row(
                Section::Privacy,
                t!("settings.group.logs"),
                t!("settings.row.logs_folder"),
                t!("settings.row.logs_folder.description"),
                &["diagnostics", "telemetry", "folder"],
            ),
            RowId::Version => row(
                Section::About,
                t!("settings.group.zenkai"),
                t!("settings.row.version"),
                "",
                &["release", "build"],
            ),
            RowId::License => row(
                Section::About,
                t!("settings.group.zenkai"),
                t!("settings.row.license"),
                t!("settings.row.license.description"),
                &["apache", "open source", "licence"],
            ),
        }
    }

    pub fn is_space_row(self) -> bool {
        matches!(
            self,
            RowId::SpaceStyle | RowId::SpaceIntensity | RowId::SpaceTint | RowId::NewSpaceColor
        )
    }

    pub fn is_modified(self, settings: &Settings, spaces: &SpaceAppearance) -> bool {
        let default = Settings::default();
        let default_spaces = SpaceAppearance::default();
        match self {
            RowId::RestoreSession => {
                settings.general.restore_session != default.general.restore_session
            }
            RowId::Language => settings.language != default.language,
            RowId::Autosave => {
                settings.general.autosave_seconds != default.general.autosave_seconds
            }
            RowId::ColorMode => settings.appearance.mode != default.appearance.mode,
            RowId::DarkTheme => settings.appearance.dark_theme != default.appearance.dark_theme,
            RowId::LightTheme => settings.appearance.light_theme != default.appearance.light_theme,
            RowId::SpaceStyle => spaces.look.style != default_spaces.look.style,
            RowId::SpaceIntensity => spaces.look.intensity != default_spaces.look.intensity,
            RowId::SpaceTint => spaces.look.apply_to != default_spaces.look.apply_to,
            RowId::NewSpaceColor => spaces.new_space_color != default_spaces.new_space_color,
            RowId::Permission => settings.agents.permission != default.agents.permission,
            RowId::ConfirmElevated => {
                settings.agents.confirm_elevated_at_start
                    != default.agents.confirm_elevated_at_start
            }
            RowId::ExternalAgents => {
                settings.agents.external_agents != default.agents.external_agents
            }
            RowId::FastXlsxReader => settings.advanced.xlsx_reader != default.advanced.xlsx_reader,
            RowId::Agents
            | RowId::ConnectionCommand
            | RowId::WhatLeaves
            | RowId::RecoveryFolder
            | RowId::SettingsFile
            | RowId::LogsFolder
            | RowId::Version
            | RowId::License => false,
        }
    }

    pub fn reset_settings(self, settings: &mut Settings) {
        let default = Settings::default();
        match self {
            RowId::RestoreSession => {
                settings.general.restore_session = default.general.restore_session
            }
            RowId::Language => settings.language = default.language,
            RowId::Autosave => settings.general.autosave_seconds = default.general.autosave_seconds,
            RowId::ColorMode => settings.appearance.mode = default.appearance.mode,
            RowId::DarkTheme => settings.appearance.dark_theme = default.appearance.dark_theme,
            RowId::LightTheme => settings.appearance.light_theme = default.appearance.light_theme,
            RowId::Permission => settings.agents.permission = default.agents.permission,
            RowId::ConfirmElevated => {
                settings.agents.confirm_elevated_at_start = default.agents.confirm_elevated_at_start
            }
            RowId::ExternalAgents => {
                settings.agents.external_agents = default.agents.external_agents
            }
            RowId::FastXlsxReader => settings.advanced.xlsx_reader = default.advanced.xlsx_reader,
            _ => {}
        }
    }

    pub fn reset_spaces(self, spaces: &mut SpaceAppearance) {
        let default = SpaceAppearance::default();
        match self {
            RowId::SpaceStyle => spaces.look.style = default.look.style,
            RowId::SpaceIntensity => spaces.look.intensity = default.look.intensity,
            RowId::SpaceTint => spaces.look.apply_to = default.look.apply_to,
            RowId::NewSpaceColor => spaces.new_space_color = default.new_space_color,
            _ => {}
        }
    }

    fn matches(self, query: &str) -> bool {
        let info = self.info();
        let query = query.to_lowercase();
        [info.title, info.description, info.group]
            .iter()
            .chain(info.keywords)
            .any(|text| text.to_lowercase().contains(&query))
    }
}

#[derive(Clone, Copy)]
pub struct Values<'a> {
    pub settings: &'a Settings,
    pub spaces: &'a SpaceAppearance,
}

impl Values<'_> {
    pub fn modified(&self, row: RowId) -> bool {
        row.is_modified(self.settings, self.spaces)
    }

    pub fn modified_in(&self, section: Section) -> usize {
        RowId::ALL
            .iter()
            .filter(|row| row.info().section == section && self.modified(**row))
            .count()
    }

    pub fn modified_total(&self) -> usize {
        RowId::ALL.iter().filter(|row| self.modified(**row)).count()
    }
}

// What the content area lists: a search looks through every section, "Modified only" keeps
// what differs from the defaults, otherwise the selected section.
pub fn visible_rows(
    section: Section,
    query: &str,
    modified_only: bool,
    values: Values,
) -> Vec<RowId> {
    let query = query.trim();
    RowId::ALL
        .into_iter()
        .filter(|row| {
            let in_scope = !query.is_empty() || modified_only || row.info().section == section;
            in_scope
                && (query.is_empty() || row.matches(query))
                && (!modified_only || values.modified(*row))
        })
        .collect()
}

pub fn permission_blurb(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::ReadOnly => t!("settings.permission.read_only.blurb"),
        PermissionMode::AskBeforeWrite => t!("settings.permission.ask_before_write.blurb"),
        PermissionMode::Automatic => t!("settings.permission.automatic.blurb"),
    }
}

pub fn external_agents_on(settings: &Settings) -> bool {
    settings.agents.external_agents == ExternalAgents::Allowed
}

#[cfg(test)]
mod tests {
    use zenkai_agent::preferences::{ColorMode, DarkTheme};
    use zenkai_types::Language;

    use super::*;
    use crate::space_appearance::{Intensity, NewSpaceColor, SpaceStyle};

    fn values<'a>(settings: &'a Settings, spaces: &'a SpaceAppearance) -> Values<'a> {
        Values { settings, spaces }
    }

    fn rows(section: Section, query: &str, only: bool, v: Values) -> Vec<RowId> {
        visible_rows(section, query, only, v)
    }

    #[test]
    fn nothing_is_modified_at_the_defaults() {
        let (settings, spaces) = (Settings::default(), SpaceAppearance::default());
        let v = values(&settings, &spaces);
        assert_eq!(v.modified_total(), 0);
        assert!(rows(Section::Appearance, "", true, v).is_empty());
    }

    #[test]
    fn each_changed_value_marks_its_own_row_and_counts_in_its_section() {
        let mut settings = Settings::default();
        settings.general.autosave_seconds = 30;
        settings.language = Some(Language::Spanish);
        settings.appearance.mode = ColorMode::Dark;
        settings.agents.confirm_elevated_at_start = false;
        let mut spaces = SpaceAppearance::default();
        spaces.look.style = SpaceStyle::Dot;
        let v = values(&settings, &spaces);
        for row in [
            RowId::Autosave,
            RowId::Language,
            RowId::ColorMode,
            RowId::ConfirmElevated,
            RowId::SpaceStyle,
        ] {
            assert!(v.modified(row), "{row:?}");
        }
        assert!(!v.modified(RowId::RestoreSession));
        assert_eq!(v.modified_in(Section::General), 2);
        assert_eq!(v.modified_in(Section::Appearance), 2);
        assert_eq!(v.modified_in(Section::Ai), 1);
        assert_eq!(v.modified_total(), 5);
    }

    #[test]
    fn reset_puts_a_row_back_to_its_default_and_touches_nothing_else() {
        let mut settings = Settings::default();
        settings.general.restore_session = false;
        settings.general.autosave_seconds = 120;
        settings.appearance.dark_theme = DarkTheme::HighContrast;
        let mut spaces = SpaceAppearance::default();
        spaces.look.intensity = Intensity::from(40);
        spaces.new_space_color = NewSpaceColor::None;

        RowId::Autosave.reset_settings(&mut settings);
        RowId::SpaceIntensity.reset_spaces(&mut spaces);
        let v = values(&settings, &spaces);
        assert!(!v.modified(RowId::Autosave));
        assert!(!v.modified(RowId::SpaceIntensity));
        assert!(v.modified(RowId::RestoreSession));
        assert!(v.modified(RowId::DarkTheme));
        assert!(v.modified(RowId::NewSpaceColor));

        for row in RowId::ALL {
            row.reset_settings(&mut settings);
            row.reset_spaces(&mut spaces);
        }
        assert_eq!(values(&settings, &spaces).modified_total(), 0);
        assert_eq!(settings, Settings::default());
        assert_eq!(spaces, SpaceAppearance::default());
    }

    #[test]
    fn without_a_search_the_selected_section_lists_its_rows_in_order() {
        let (settings, spaces) = (Settings::default(), SpaceAppearance::default());
        let v = values(&settings, &spaces);
        assert_eq!(
            rows(Section::General, "", false, v),
            [RowId::RestoreSession, RowId::Language, RowId::Autosave]
        );
        assert!(rows(Section::Keyboard, "", false, v).is_empty());
    }

    #[test]
    fn a_search_looks_through_every_section_ignoring_case_and_the_selection() {
        let (settings, spaces) = (Settings::default(), SpaceAppearance::default());
        let v = values(&settings, &spaces);
        assert_eq!(
            rows(Section::General, "AUTOSAVE", false, v),
            [RowId::Autosave, RowId::RecoveryFolder]
        );
        let dark = rows(Section::Ai, "dark", false, v);
        assert!(dark.contains(&RowId::ColorMode) && dark.contains(&RowId::DarkTheme));
        assert!(!dark.contains(&RowId::Permission));
        assert!(rows(Section::General, "zzz-nothing", false, v).is_empty());
        assert_eq!(
            rows(Section::General, "  autosave  ", false, v),
            rows(Section::General, "autosave", false, v)
        );
    }

    #[test]
    fn modified_only_spans_sections_and_combines_with_a_search() {
        let mut settings = Settings::default();
        settings.general.restore_session = false;
        settings.appearance.mode = ColorMode::Light;
        let spaces = SpaceAppearance::default();
        let v = values(&settings, &spaces);
        assert_eq!(
            rows(Section::Ai, "", true, v),
            [RowId::RestoreSession, RowId::ColorMode]
        );
        assert_eq!(
            rows(Section::Ai, "session", true, v),
            [RowId::RestoreSession]
        );
        assert!(rows(Section::Ai, "autosave", true, v).is_empty());
    }

    #[test]
    fn every_row_belongs_to_a_section_and_rows_of_a_group_are_contiguous() {
        let mut seen: Vec<(Section, &str)> = Vec::new();
        for row in RowId::ALL {
            let info = row.info();
            let key = (info.section, info.group);
            if seen.last() != Some(&key) {
                assert!(!seen.contains(&key), "{key:?} is split");
                seen.push(key);
            }
        }
    }
}
