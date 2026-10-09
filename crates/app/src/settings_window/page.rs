use gpui_kit::base::v_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::*;
use zenkai_agent::preferences::{ColorMode, MAX_AUTOSAVE_SECONDS, MIN_AUTOSAVE_SECONDS};
use zenkai_agent::settings::{PermissionMode, Settings};
use zenkai_i18n::t;

use super::choices::{self, Choices};
use super::controls::{self, RowParts, card, explanation, group_title, mode_cards, segmented};
use super::rows::{RowId, Section, Values, visible_rows};
use super::{OpenDropdown, SettingsWindow, agents, info, keyboard, update};
use crate::space_appearance::SpaceAppearance;
use crate::space_controls::style_cards;
use crate::{agent_settings, space_settings, theme_list};

impl SettingsWindow {
    pub(super) fn content(
        &mut self,
        values: Values,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.query.trim().to_string();
        let searching = !query.is_empty();
        let spanning = searching || self.modified_only;
        let rows = visible_rows(self.section, &query, self.modified_only, values);
        let listing_shortcuts = spanning || self.section == Section::Keyboard;
        let shortcut_matches = if listing_shortcuts {
            keyboard::matching(
                &self.shortcuts,
                &query,
                self.keyboard.pressed.as_deref(),
                self.modified_only,
            )
        } else {
            Vec::new()
        };
        let results = rows.len() + shortcut_matches.len();
        let shortcut_blocks = self.shortcut_blocks(&shortcut_matches, cx);
        let shortcut_tools =
            (self.section == Section::Keyboard && !spanning).then(|| self.shortcut_tools(cx));
        let (title, lead): (SharedString, SharedString) = if self.modified_only {
            (
                t!("settings.modified.title").into(),
                t!("settings.modified.lead").into(),
            )
        } else if searching {
            (
                t!("settings.search.title").into(),
                t!("settings.search.matches", count = results).into(),
            )
        } else {
            (self.section.label().into(), self.section.lead().into())
        };

        let mut blocks: Vec<AnyElement> = Vec::new();
        let mut start = 0;
        while start < rows.len() {
            let info = rows[start].info();
            let end = rows[start..]
                .iter()
                .position(|row| {
                    let next = row.info();
                    (next.section, next.group) != (info.section, info.group)
                })
                .map_or(rows.len(), |offset| start + offset);
            let heading = if spanning {
                format!("{}: {}", info.section.label().to_uppercase(), info.group)
            } else {
                info.group.to_string()
            };
            let cards: Vec<AnyElement> = rows[start..end]
                .iter()
                .enumerate()
                .map(|(position, row)| self.render_row(*row, position == 0, values, window, cx))
                .collect();
            blocks.push(
                v_flex()
                    .gap_2()
                    .child(group_title(heading, cx))
                    .child(card(cards, cx))
                    .into_any_element(),
            );
            start = end;
        }
        blocks.extend(shortcut_tools);
        blocks.extend(shortcut_blocks);
        if results == 0 {
            blocks.push(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.modified_only {
                        t!("settings.modified.empty")
                    } else {
                        t!("settings.search.empty")
                    })
                    .into_any_element(),
            );
        }

        let theme = cx.theme();
        div()
            .id("settings-content")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                v_flex()
                    .gap_5()
                    .px_10()
                    .pt_7()
                    .pb_10()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(div().text_color(theme.muted_foreground).child(lead)),
                    )
                    .children(
                        self.shown_held
                            .as_ref()
                            .map(|held| info::held_banner(held, cx)),
                    )
                    .children(info::notices(cx))
                    .children(blocks),
            )
            .into_any_element()
    }

    fn render_row(
        &mut self,
        row: RowId,
        first: bool,
        values: Values,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let info = row.info();
        let id = row as usize;
        let settings = values.settings;
        let spaces = values.spaces;
        let modified = values.modified(row);
        let mut parts = RowParts {
            modified,
            first,
            control: None,
            below: None,
            overlay: None,
            code: None,
        };
        let weak = cx.entity().downgrade();
        match row {
            RowId::RestoreSession => {
                parts.control = Some(controls::switch(
                    ("row", id),
                    info.title,
                    settings.general.restore_session,
                    |on, _, cx| {
                        agent_settings::change(cx, move |settings| {
                            settings.general.restore_session = on
                        })
                    },
                ));
            }
            RowId::Language | RowId::DarkTheme | RowId::LightTheme | RowId::NewSpaceColor => {
                let open = self.dropdown.as_ref().is_some_and(|open| open.row == row);
                parts.control = Some(controls::select(
                    ("row", id),
                    info.title,
                    choices::value_label(row, settings, spaces),
                    open,
                    move |window, cx| {
                        update(&weak, cx, |this, cx| this.toggle_dropdown(row, window, cx))
                    },
                ));
                parts.overlay = self.dropdown_overlay(row, settings, spaces, cx);
            }
            RowId::Autosave => {
                let seconds = settings.general.autosave_seconds;
                parts.control = Some(controls::stepper(
                    "autosave",
                    t!("settings.autosave_interval"),
                    format!("{seconds} s"),
                    seconds > MIN_AUTOSAVE_SECONDS,
                    seconds < MAX_AUTOSAVE_SECONDS,
                    |up, _, cx| {
                        agent_settings::change(cx, move |settings| {
                            settings.general.autosave_seconds =
                                settings.general.autosave_stepped(up)
                        })
                    },
                    cx,
                ));
            }
            RowId::ColorMode => {
                parts.below = Some(
                    mode_cards(
                        settings.appearance.mode,
                        |mode: ColorMode, _, cx| {
                            agent_settings::change(cx, move |settings| {
                                settings.appearance.mode = mode
                            })
                        },
                        cx,
                    )
                    .into_any_element(),
                );
            }
            RowId::SpaceStyle => {
                parts.below = Some(
                    style_cards(
                        "default-space-style",
                        spaces.look,
                        |style, _, cx| {
                            space_settings::change(cx, move |spaces| spaces.look.style = style)
                        },
                        cx,
                    )
                    .into_any_element(),
                );
            }
            RowId::SpaceIntensity => {
                let percent = spaces.look.intensity.percent();
                parts.control = Some(controls::stepper(
                    "space-intensity",
                    t!("settings.space_intensity"),
                    format!("{percent}%"),
                    percent > 0,
                    percent < crate::space_appearance::MAX_INTENSITY,
                    |up, _, cx| {
                        space_settings::change(cx, move |spaces| {
                            spaces.look.intensity = spaces.look.intensity.stepped(up)
                        })
                    },
                    cx,
                ));
            }
            RowId::SpaceTint => {
                parts.control = Some(controls::switch(
                    ("row", id),
                    info.title,
                    spaces.look.apply_to.workbooks(),
                    |on, _, cx| {
                        space_settings::change(cx, move |spaces| {
                            spaces.look.apply_to = crate::space_appearance::ApplyTo::of(
                                spaces.look.apply_to.header(),
                                on,
                            )
                        })
                    },
                ));
            }
            RowId::Agents => {
                parts.control = Some(self.add_custom_agent_button(cx));
                parts.below = Some(self.agents_list(settings, window, cx));
            }
            RowId::Permission => {
                let current = settings.agents.permission;
                parts.below = Some(
                    v_flex()
                        .gap_2p5()
                        .child(segmented(
                            "permission",
                            PermissionMode::ALL
                                .into_iter()
                                .map(|mode| (mode, mode.label(), mode == current))
                                .collect(),
                            |mode, _, cx| agents::set_permission(mode, cx),
                            cx,
                        ))
                        .child(explanation(super::rows::permission_blurb(current), cx))
                        .into_any_element(),
                );
            }
            RowId::ConfirmElevated => {
                parts.control = Some(controls::switch(
                    ("row", id),
                    info.title,
                    settings.agents.confirm_elevated_at_start,
                    |on, _, cx| {
                        agent_settings::change(cx, move |settings| {
                            settings.agents.confirm_elevated_at_start = on
                        })
                    },
                ));
            }
            RowId::ExternalAgents => {
                parts.control = Some(controls::switch(
                    ("row", id),
                    info.title,
                    super::rows::external_agents_on(settings),
                    |on, _, cx| agents::set_external_agents(on, cx),
                ));
                parts.below = Some(info::bridge_status(cx));
            }
            RowId::ConnectionCommand => {
                parts.code = self.claude_command.clone();
                parts.control = Some(info::button(
                    ("row", id),
                    t!("settings.copy_command"),
                    t!("palette.copy_claude_command"),
                    |_, cx| agents::copy_claude_command(cx),
                ));
            }
            RowId::WhatLeaves => {}
            RowId::RecoveryFolder => {
                let folder = crate::recovery::directory();
                parts.code = folder.as_ref().map(|path| path.display().to_string());
                if let Some(folder) = folder {
                    parts.control = Some(info::button(
                        ("row", id),
                        t!("settings.show_folder"),
                        t!("settings.show_recovery_folder"),
                        move |_, cx| cx.reveal_path(&folder),
                    ));
                }
            }
            RowId::SettingsFile => {
                let file = info::settings_file(cx);
                parts.code = file.as_ref().map(|path| path.display().to_string());
                if let Some(file) = file {
                    parts.control = Some(info::button(
                        ("row", id),
                        t!("settings.open"),
                        t!("settings.open_settings_file"),
                        move |_, cx| cx.open_with_system(&file),
                    ));
                }
            }
            RowId::LogsFolder => {
                let file = crate::logging::log_path();
                parts.code = file.as_ref().map(|path| path.display().to_string());
                if let Some(file) = file {
                    parts.control = Some(info::button(
                        ("row", id),
                        t!("settings.show_folder"),
                        t!("settings.show_log_file"),
                        move |_, cx| cx.reveal_path(&file),
                    ));
                }
            }
            RowId::Version => {
                parts.control = Some(info::value(env!("CARGO_PKG_VERSION"), cx));
            }
            RowId::License => {
                parts.control = Some(info::value(env!("CARGO_PKG_LICENSE"), cx));
            }
        }
        controls::row(
            id,
            &info,
            parts,
            move |_, cx| {
                if row.is_space_row() {
                    space_settings::change(cx, move |spaces| row.reset_spaces(spaces));
                } else {
                    agent_settings::change(cx, move |settings| row.reset_settings(settings));
                }
            },
            cx,
        )
    }

    fn toggle_dropdown(&mut self, row: RowId, window: &mut Window, cx: &mut Context<Self>) {
        if self.dropdown.as_ref().is_some_and(|open| open.row == row) {
            self.dropdown = None;
            window.focus(&self.focus, cx);
        } else {
            let list = cx.new(|cx| CommandState::new(window, cx));
            list.update(cx, |list, cx| list.focus(window, cx));
            self.dropdown = Some(OpenDropdown { row, list });
        }
        cx.notify();
    }

    fn dropdown_overlay(
        &self,
        row: RowId,
        settings: &Settings,
        spaces: &SpaceAppearance,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.dropdown.as_ref().filter(|open| open.row == row)?;
        let choices = choices::choices(row, settings, spaces)?;
        let window_for_pick = cx.entity().downgrade();
        let command = Command::new(&open.list);
        let command = match choices {
            Choices::Themes { themes, current } => command
                .items(theme_list::items(&themes, current))
                .placeholder(t!("palette.select_theme")),
            Choices::Plain(options) => command.searchable(false).items(
                options
                    .into_iter()
                    .map(|(label, current)| CommandItem::new().label(label).checked(current)),
            ),
        }
        .max_h(px(260.0))
        .bordered(true)
        .on_confirm(move |path, window, cx| {
            choices::pick(row, path.row, cx);
            update(&window_for_pick, cx, |this, cx| {
                this.dropdown = None;
                window.focus(&this.focus, cx);
                cx.notify();
            });
        });
        Some(command.into_any_element())
    }
}
