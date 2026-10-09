use std::path::{Path, PathBuf};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::*;

const RECENT_ACTIONS: [fn() -> Box<dyn Action>; 5] = [
    || Box::new(OpenRecent1),
    || Box::new(OpenRecent2),
    || Box::new(OpenRecent3),
    || Box::new(OpenRecent4),
    || Box::new(OpenRecent5),
];

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn recent_row(index: usize, path: &Path, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let action = RECENT_ACTIONS[index];
    h_flex()
        .id(("start-recent", index))
        .gap_3()
        .px_3()
        .py_1()
        .items_baseline()
        .rounded_sm()
        .cursor_pointer()
        .hover(|row| row.bg(theme.list_hover))
        .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
        .child(div().child(file_name(path)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(path.display().to_string()),
        )
}

// Shown when no workbook is open, as in Excel after closing the last one.
pub fn render(recent: &[PathBuf], focus: &FocusHandle, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let entries = recent
        .iter()
        .take(RECENT_ACTIONS.len())
        .enumerate()
        .map(|(index, path)| recent_row(index, path, cx));
    div()
        .id("start-view")
        .track_focus(focus)
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            v_flex()
                .w(px(520.0))
                .gap_4()
                .child(div().text_xl().child("No workbook open"))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("start-new")
                                .primary()
                                .label("New workbook (Ctrl+N)")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(NewWorkbook), cx)
                                }),
                        )
                        .child(
                            Button::new("start-open").label("Open… (Ctrl+O)").on_click(
                                |_, window, cx| window.dispatch_action(Box::new(Open), cx),
                            ),
                        ),
                )
                .when(!recent.is_empty(), |column| {
                    column
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Recent"),
                        )
                        .child(v_flex().children(entries))
                }),
        )
}
