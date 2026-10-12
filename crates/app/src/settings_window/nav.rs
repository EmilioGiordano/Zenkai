use gpui_kit::assets::IconName;
use gpui_kit::base::{Selectable, h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::rows::{Section, Values};
use super::{SettingsWindow, update};

const WIDTH: f32 = 232.0;

fn icon_of(section: Section) -> IconName {
    match section {
        Section::General => IconName::Settings,
        Section::Appearance => IconName::Palette,
        Section::Keyboard => IconName::Keyboard,
        Section::Ai => IconName::Bot,
        Section::Files => IconName::Folder,
        Section::Privacy => IconName::Shield,
        Section::About => IconName::Info,
    }
}

fn badge(count: usize, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .min_w(px(18.0))
        .h(px(18.0))
        .px_1p5()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .bg(theme.secondary)
        .text_color(theme.secondary_foreground)
        .child(count.to_string())
}

impl SettingsWindow {
    pub(super) fn nav(&self, values: Values, cx: &mut Context<Self>) -> AnyElement {
        let searching = !self.query.trim().is_empty();
        let theme = cx.theme();
        let total = values.modified_total();
        let sections =
            Section::ALL.map(|section| {
                let active = !searching && !self.modified_only && section == self.section;
                let count = values.modified_in(section);
                Button::new(("section", section as usize))
                    .ghost()
                    .w_full()
                    .selected(active)
                    .accessibility_label(if count > 0 {
                        t!(
                            "settings.nav.modified",
                            count = count,
                            section = section.label()
                        )
                    } else {
                        section.label().to_string()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_section(section, window, cx)
                    }))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2p5()
                            .items_center()
                            .child(Icon::new(icon_of(section)).size_4())
                            .child(
                                div()
                                    .flex_1()
                                    .text_left()
                                    .font_weight(if active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .child(section.label()),
                            )
                            .when(count > 0, |this| this.child(badge(count, cx))),
                    )
            });
        v_flex()
            .w(px(WIDTH))
            .flex_shrink_0()
            .h_full()
            .gap_0p5()
            .px_2p5()
            .py_3()
            .bg(theme.title_bar)
            .child(
                div().mb_2p5().child(
                    Input::new(&self.search)
                        .prefix(Icon::new(IconName::Search).size_4())
                        .suffix(
                            div()
                                .text_xs()
                                .font_family("monospace")
                                .text_color(theme.muted_foreground)
                                .child("Ctrl+F"),
                        )
                        .aria_label(t!("settings.search"))
                        .cleanable(true),
                ),
            )
            .children(sections)
            .child(div().flex_1())
            .child(
                h_flex()
                    .h(px(40.0))
                    .px_2p5()
                    .gap_2p5()
                    .items_center()
                    .rounded_lg()
                    .bg(theme.secondary)
                    .child(div().flex_1().child(t!("settings.modified_only")))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(total.to_string()),
                    )
                    .child(super::controls::switch(
                        "modified-only",
                        t!("settings.modified_only.shortcut"),
                        self.modified_only,
                        {
                            let window = cx.entity().downgrade();
                            move |on, _, cx| {
                                update(&window, cx, |this, cx| {
                                    this.modified_only = on;
                                    this.dropdown = None;
                                    cx.notify();
                                })
                            }
                        },
                    )),
            )
            .into_any_element()
    }
}
