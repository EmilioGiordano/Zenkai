use gpui_kit::component::command::{Command, CommandState};
use gpui_kit::*;
use zenkai_agent::preferences::ThemeChoice;
use zenkai_i18n::t;

use super::{Workspace, command_overlay};
use crate::actions::CancelThemePicker;
use crate::theme::{self, ThemePreview};
use crate::{agent_settings, theme_list};

pub(super) struct ThemePicker {
    state: Entity<CommandState>,
    preview: ThemePreview,
}

impl Workspace {
    pub(super) fn open_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.search = None;
        let appearance = agent_settings::settings(cx).appearance;
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |state, cx| state.focus(window, cx));
        self.theme_picker = Some(ThemePicker {
            state,
            preview: ThemePreview::begin(appearance, theme::shown(cx)),
        });
        cx.notify();
    }

    fn preview_theme(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(picker) = self.theme_picker.as_mut() else {
            return;
        };
        let Some(choice) = ThemeChoice::ALL.get(row) else {
            return;
        };
        if let Some(shown) = picker.preview.highlight(*choice) {
            theme::show(shown, cx);
        }
    }

    fn keep_theme(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(picker), Some(choice)) = (self.theme_picker.take(), ThemeChoice::ALL.get(row))
        else {
            return;
        };
        let appearance = picker.preview.keep(*choice);
        theme::show(*choice, cx);
        agent_settings::change(cx, move |settings| settings.appearance = appearance);
        self.leave_theme_picker(window, cx);
    }

    pub(super) fn cancel_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.revert_theme_preview(cx) {
            self.leave_theme_picker(window, cx);
        }
    }

    // Any other overlay opening over the picker ends the preview, so a theme that was only
    // being tried never stays on screen.
    pub(super) fn revert_theme_preview(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(mut picker) = self.theme_picker.take() else {
            return false;
        };
        if let Some(original) = picker.preview.cancel() {
            theme::show(original, cx);
        }
        true
    }

    fn leave_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn render_theme_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.theme_picker.as_ref()?;
        let entity = cx.entity().downgrade();
        let selected = entity.clone();
        let command = Command::new(&picker.state)
            .items(theme_list::items(
                &ThemeChoice::ALL,
                picker.preview.original_shown(),
            ))
            .placeholder(t!("palette.select_theme"))
            .on_select(move |path, _, cx| {
                if let Err(error) = selected.update(cx, |this, cx| this.preview_theme(path.row, cx))
                {
                    tracing::debug!(%error, "workspace dropped");
                }
            })
            .on_confirm(move |path, window, cx| {
                if let Err(error) =
                    entity.update(cx, |this, cx| this.keep_theme(path.row, window, cx))
                {
                    tracing::debug!(%error, "workspace dropped");
                }
            })
            .on_cancel(|window, cx| window.dispatch_action(Box::new(CancelThemePicker), cx));
        Some(
            command_overlay(command, "ThemePicker", || Box::new(CancelThemePicker))
                .into_any_element(),
        )
    }
}
