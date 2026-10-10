use gpui_kit::assets::IconName;
use gpui_kit::base::{Selectable, h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::badge;
use super::slash::{Entry, Target};
use super::{ChatPanel, MONO, Menu};
use crate::actions::*;
use crate::keymap;

const PICKER_LABEL_WIDTH: f32 = 150.0;
const MENU_GAP: f32 = 8.0;
const FOCUSED_EDGE: f32 = 0.16;

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
    tooltip: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .child(
            h_flex()
                .gap_1p5()
                .items_center()
                .child(
                    div()
                        .max_w(px(PICKER_LABEL_WIDTH))
                        .truncate()
                        .child(label.into()),
                )
                .child(Icon::new(IconName::ChevronDown).size_3()),
        )
        .tooltip(tooltip)
        .on_click(on_click)
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

    // Drawn in the overlay layer; the backdrop takes the dismissing click so the button
    // cannot reopen the menu it just closed.
    fn picker_slot(
        &self,
        button: Button,
        menu: Option<AnyElement>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let slot = div().relative().child(button);
        let Some(menu) = menu else {
            return slot;
        };
        let viewport = window.viewport_size();
        let backdrop = div()
            .occlude()
            .w(viewport.width)
            .h(viewport.height)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close_menu(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| this.close_menu(cx)),
            );
        slot.child(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .position(point(px(0.0), px(0.0)))
                    .child(backdrop),
            )
            .priority(1),
        )
        .child(
            deferred(
                anchored()
                    .anchor(Anchor::BottomLeft)
                    .offset(point(px(0.0), px(-MENU_GAP)))
                    .snap_to_window_with_margin(px(MENU_GAP))
                    .child(menu),
            )
            .priority(2),
        )
    }

    fn open_picker(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_composer(window, cx);
        self.toggle_menu(menu, cx);
    }

    pub(super) fn render_composer(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.composer.focus_handle(cx).contains_focused(window, cx);
        let agent = self.agent_name(cx);
        let busy = self.busy();
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
        let slash = self.render_slash(cx);
        let model_menu = self.render_model_menu(cx);
        let access_menu = self.render_access_menu(cx);
        let agent_menu = self.render_agent_menu(cx);
        let model_picker = picker(
            "chat-model",
            self.model_label(),
            keymap::labeled(cx, t!("chat.model_and_effort"), &PickChatModel),
            cx.listener(|this, _, window, cx| this.open_picker(Menu::Model, window, cx)),
        )
        .selected(self.menu == Some(Menu::Model));
        let access_picker = picker(
            "chat-access",
            self.access.label(),
            keymap::labeled(cx, t!("chat.change_access"), &PickChatPermission),
            cx.listener(|this, _, window, cx| this.open_picker(Menu::Access, window, cx)),
        )
        .selected(self.menu == Some(Menu::Access));
        let agent_picker = picker(
            "chat-agent",
            agent,
            keymap::labeled(cx, t!("chat.switch_agent"), &CycleChatAgent),
            cx.listener(|this, _, window, cx| this.open_picker(Menu::Agent, window, cx)),
        )
        .selected(self.menu == Some(Menu::Agent));
        let theme = cx.theme();
        let border = if focused {
            theme.border.blend(theme.foreground.opacity(FOCUSED_EDGE))
        } else {
            theme.border
        };
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
            .child(
                Textarea::new(&self.composer)
                    .appearance(false)
                    .bordered(false)
                    .aria_label(t!("chat.composer.label")),
            )
            .when(!self.references.is_empty(), |composer| {
                let panel = cx.weak_entity();
                composer.child(
                    h_flex().flex_wrap().gap_1p5().children(
                        self.references
                            .iter()
                            .enumerate()
                            .map(|(index, reference)| {
                                badge::chip(index, reference, panel.clone(), cx)
                            }),
                    ),
                )
            })
            .child(
                h_flex()
                    .gap_0p5()
                    .items_center()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.picker_slot(agent_picker, agent_menu, window, cx))
                    .child(self.picker_slot(model_picker, model_menu, window, cx))
                    .child(self.picker_slot(access_picker, access_menu, window, cx))
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
