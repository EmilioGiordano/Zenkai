use gpui_kit::assets::IconName;
use gpui_kit::base::{Selectable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Textarea;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::chat::state::{ConfigKind, Select};
use zenkai_i18n::t;

use super::slash::{Entry, Target};
use super::{ChatPanel, MONO, Menu};
use crate::actions::*;
use crate::agent_settings::AgentConfig;
use crate::keymap;

fn icon_button(
    id: &'static str,
    icon: IconName,
    tooltip: impl Into<SharedString>,
    action: impl Action + Clone,
) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .icon(icon)
        .tooltip(tooltip)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

fn picker(
    id: &'static str,
    label: impl Into<SharedString>,
    tooltip: &'static str,
    action: impl Action + Clone,
) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .label(label)
        .icon(IconName::ChevronDown)
        .tooltip(tooltip)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

impl ChatPanel {
    fn render_slash(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let entries = self.slash_entries(cx);
        if entries.is_empty() {
            return None;
        }
        let selected = self.slash_index.min(entries.len() - 1);
        let agent = self.agent_name(cx);
        let rows: Vec<AnyElement> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                self.slash_row(index, entry, index == selected, cx)
                    .into_any_element()
            })
            .collect();
        let theme = cx.theme();
        let mut list = v_flex().gap_0p5();
        let mut group: Option<bool> = None;
        for (entry, row) in entries.iter().zip(rows) {
            let own = matches!(entry.target, Target::Zenkai(_));
            if group != Some(own) {
                group = Some(own);
                list = list.child(
                    div()
                        .px_2p5()
                        .pt_1p5()
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.muted_foreground)
                        .child(if own {
                            "Zenkai".to_string()
                        } else {
                            agent.clone()
                        }),
                );
            }
            list = list.child(row);
        }
        Some(
            div()
                .id("chat-slash")
                .role(Role::ListBox)
                .aria_label(t!("chat.slash.label"))
                .occlude()
                .absolute()
                .left_0()
                .right_0()
                .bottom(relative(1.0))
                .mb_2()
                .p_1p5()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .shadow_lg()
                .child(list)
                .into_any_element(),
        )
    }

    fn slash_row(
        &self,
        index: usize,
        entry: &Entry,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .id(SharedString::from(format!("slash-{index}")))
            .role(Role::ListBoxOption)
            .h(px(32.0))
            .px_2p5()
            .gap_2p5()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(theme.accent).text_color(theme.accent_foreground)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.slash_index = index;
                this.slash_accept(window, cx);
            }))
            .child(
                div()
                    .min_w(px(110.0))
                    .font_family(MONO)
                    .text_xs()
                    .child(entry.name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(entry.description.clone()),
            )
    }

    fn render_model_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.menu != Some(Menu::Model) {
            return None;
        }
        let model = self.state.select(ConfigKind::Model).cloned();
        let effort = self.state.select(ConfigKind::Effort).cloned();
        let effort_row = effort.as_ref().map(|effort| self.effort_row(effort, cx));
        let theme = cx.theme();
        let heading = |text: &'static str| {
            div()
                .px_2p5()
                .pt_1p5()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(text)
        };
        let mut menu = v_flex().gap_0p5();
        if let Some(model) = &model {
            menu = menu.child(heading(t!("chat.model.heading")));
            for choice in &model.choices {
                let value = choice.value.clone();
                let current = model.current == choice.value;
                menu = menu.child(
                    Button::new(SharedString::from(format!("model-{}", choice.value)))
                        .ghost()
                        .w_full()
                        .selected(current)
                        .label(if current {
                            t!("chat.model.current", label = choice.label)
                        } else {
                            choice.label.clone()
                        })
                        .when_some(choice.description.clone(), |button, text| {
                            button.tooltip(text)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.choose(ConfigKind::Model, &value, cx);
                        })),
                );
            }
        }
        if let Some(effort_row) = effort_row {
            menu = menu
                .child(heading(t!("chat.effort.heading")))
                .child(effort_row);
        }
        Some(
            div()
                .id("chat-model-menu")
                .role(Role::Menu)
                .aria_label(t!("chat.model.heading"))
                .occlude()
                .absolute()
                .left_0()
                .right_0()
                .bottom(relative(1.0))
                .mb_2()
                .p_1p5()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .shadow_lg()
                .child(menu)
                .into_any_element(),
        )
    }

    fn effort_row(&self, effort: &Select, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap_1()
            .px_1()
            .pb_1()
            .children(effort.choices.iter().map(|choice| {
                let value = choice.value.clone();
                Button::new(SharedString::from(format!("effort-{}", choice.value)))
                    .compact()
                    .flex_1()
                    .when(effort.current == choice.value, |button| button.primary())
                    .when(effort.current != choice.value, |button| button.ghost())
                    .label(choice.label.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.choose(ConfigKind::Effort, &value, cx);
                    }))
            }))
            .into_any_element()
    }

    pub(super) fn render_composer(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let settings = cx.global::<AgentConfig>().state.current.clone();
        let focused = self.composer.focus_handle(cx).contains_focused(window, cx);
        let agent = self.agent_name(cx);
        let busy = self.busy();
        let model = self.state.select(ConfigKind::Model).cloned();
        let effort = self.state.select(ConfigKind::Effort).cloned();
        let mode = self.state.select(ConfigKind::Mode).cloned();
        let send = if busy {
            icon_button(
                "chat-stop",
                IconName::Square,
                t!("chat.stop"),
                StopAgentTurn,
            )
        } else {
            icon_button(
                "chat-send",
                IconName::ArrowUp,
                t!("chat.send"),
                SendChatMessage,
            )
            .primary()
        };
        let model_label = model
            .as_ref()
            .map(|select| select.current_label().to_string())
            .or_else(|| {
                effort
                    .as_ref()
                    .map(|select| select.current_label().to_string())
            });
        let slash = self.render_slash(cx);
        let menu = self.render_model_menu(cx);
        let theme = cx.theme();
        let border = if focused { theme.ring } else { theme.border };
        v_flex()
            .relative()
            .flex_shrink_0()
            .mx_3()
            .mb_3()
            .p_2p5()
            .gap_2p5()
            .rounded_xl()
            .border_1()
            .border_color(border)
            .bg(theme.background)
            .children(slash)
            .children(menu)
            .child(
                Textarea::new(&self.composer)
                    .appearance(false)
                    .bordered(false)
                    .aria_label(t!("chat.composer.label")),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .items_center()
                    .flex_wrap()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(picker(
                        "chat-agent",
                        agent,
                        t!("chat.switch_agent"),
                        CycleChatAgent,
                    ))
                    .when_some(model_label, |row, label| {
                        row.child(picker(
                            "chat-model",
                            label,
                            t!("chat.model_and_effort"),
                            PickChatModel,
                        ))
                    })
                    .when_some(mode, |row, select| {
                        row.child(picker(
                            "chat-mode",
                            select.current_label().to_string(),
                            t!("chat.change_mode"),
                            CycleChatMode,
                        ))
                    })
                    .child(picker(
                        "chat-permission",
                        settings.agents.permission.label(),
                        t!("chat.change_permission"),
                        CycleChatPermission,
                    ))
                    .child(div().flex_1())
                    .child(icon_button(
                        "chat-selection",
                        IconName::Grid2x2,
                        keymap::labeled(cx, t!("chat.add_selection"), &AddSelectionToChat),
                        AddSelectionToChat,
                    ))
                    .child(send),
            )
            .into_any_element()
    }
}
