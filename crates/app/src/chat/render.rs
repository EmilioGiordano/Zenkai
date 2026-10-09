use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Textarea;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::presets::Preset;
use zenkai_agent::settings::Settings;

use super::launch::chosen_agent;
use super::{ChatEvent, ChatPanel};
use crate::actions::*;
use crate::agent_settings::AgentConfig;

const PANEL_WIDTH_REMS: f32 = 22.5;

fn icon_button(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    action: impl Action + Clone,
) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .icon(icon)
        .tooltip(tooltip)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

fn text_button(
    id: &'static str,
    label: impl Into<SharedString>,
    tooltip: &'static str,
    action: impl Action + Clone,
) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .label(label)
        .tooltip(tooltip)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

fn provider_name(settings: &Settings) -> String {
    match chosen_agent(settings) {
        Some((id, server)) => {
            Preset::for_agent(&id).map_or_else(|| server.name, |preset| preset.provider.to_string())
        }
        None => "the agent's provider".to_string(),
    }
}

impl ChatPanel {
    fn render_header(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let workbook = self.active_workbook(cx).map(|(_, name)| name);
        h_flex()
            .h(px(44.0))
            .flex_shrink_0()
            .px_3p5()
            .gap_2p5()
            .items_center()
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.sidebar_foreground)
                    .child("Agent"),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .child(workbook.unwrap_or_default()),
            )
            .child(div().flex_1())
            .child(icon_button(
                "chat-new",
                IconName::Plus,
                "New conversation (Ctrl+Shift+N)",
                NewAgentConversation,
            ))
            .child(icon_button(
                "chat-close",
                IconName::PanelRightClose,
                "Close the agent panel (Ctrl+J)",
                ToggleAgentChat,
            ))
    }

    fn render_composer(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let settings = cx.global::<AgentConfig>().state.current.clone();
        let focused = self.composer.focus_handle(cx).contains_focused(window, cx);
        let agent = chosen_agent(&settings).map_or_else(|| "No agent".to_string(), |a| a.1.name);
        let busy = self.busy();
        let send = if busy {
            icon_button("chat-stop", IconName::Square, "Stop (Esc)", StopAgentTurn)
        } else {
            icon_button(
                "chat-send",
                IconName::ArrowUp,
                "Send (Enter)",
                SendChatMessage,
            )
            .primary()
        };
        v_flex()
            .flex_shrink_0()
            .mx_3()
            .mb_3()
            .p_2p5()
            .gap_2p5()
            .rounded_xl()
            .border_1()
            .border_color(if focused { theme.ring } else { theme.border })
            .bg(theme.background)
            .child(
                Textarea::new(&self.composer)
                    .appearance(false)
                    .bordered(false)
                    .aria_label("Message to the agent"),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(text_button(
                        "chat-agent",
                        agent,
                        "Switch agent (Alt+G)",
                        CycleChatAgent,
                    ))
                    .child(text_button(
                        "chat-permission",
                        settings.agents.permission.label(),
                        "Change what agents may do to the workbook (Alt+P)",
                        CycleChatPermission,
                    ))
                    .child(div().flex_1())
                    .child(icon_button(
                        "chat-selection",
                        IconName::Grid2x2,
                        "Add the selected cells to the message (Ctrl+L)",
                        AddSelectionToChat,
                    ))
                    .child(send),
            )
    }
}

impl Render for ChatPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut context = KeyContext::default();
        context.add("AgentChat");
        if self.busy() {
            context.add("Working");
        }
        let provider = provider_name(&cx.global::<AgentConfig>().state.current);
        let (sidebar, foreground, border, muted) = {
            let theme = cx.theme();
            (
                theme.sidebar,
                theme.sidebar_foreground,
                theme.border,
                theme.muted_foreground,
            )
        };
        let entries = self.render_entries(cx);
        let body = v_flex()
            .id("chat-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .gap_4()
            .px_3p5()
            .pt_2()
            .pb_3p5()
            .when(entries.is_empty() && self.gate.is_none(), |body| {
                body.child(self.render_empty(cx))
            })
            .children(entries)
            .children(self.render_gate(cx))
            .children(self.render_problem(cx))
            .children(self.render_status(cx));
        let root = v_flex()
            .id("agent-chat")
            .key_context(context)
            .flex_shrink_0()
            .w(rems(PANEL_WIDTH_REMS))
            .h_full()
            .bg(sidebar)
            .text_color(foreground)
            .border_l_1()
            .border_color(border)
            .on_action(cx.listener(|this, _: &SendChatMessage, window, cx| this.send(window, cx)))
            .on_action(cx.listener(|this, _: &StopAgentTurn, _, cx| this.stop(cx)))
            .on_action(cx.listener(|_, _: &LeaveAgentChat, _, cx| cx.emit(ChatEvent::Leave)))
            .on_action(
                cx.listener(|this, _: &CycleChatAgent, window, cx| this.cycle_agent(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &CycleChatPermission, _, cx| this.cycle_permission(cx)),
            )
            .on_action(cx.listener(|this, _: &CopyLoginCommand, _, cx| this.copy_login_command(cx)))
            .on_action(cx.listener(|this, _: &ConfirmAgentLaunch, window, cx| {
                this.confirm_launch(window, cx)
            }))
            .on_action(cx.listener(|this, _: &DeclineAgentLaunch, window, cx| {
                this.decline_launch(window, cx)
            }))
            .child(self.render_header(cx))
            .child(body)
            .child(self.render_composer(window, cx))
            .child(
                div()
                    .flex_shrink_0()
                    .px_4()
                    .pb_3()
                    .text_xs()
                    .text_color(muted)
                    .child(format!(
                        "What the agent reads is sent to {provider}. Zenkai gives it no access to \
                         your files or terminal; its own tools ask you here first."
                    )),
            );
        swallow_cell_edits(root)
    }
}

// Typing in the composer must never reach the grid: these Workspace shortcuts would
// otherwise change the selected cells while the focus is here.
fn swallow_cell_edits(root: Stateful<Div>) -> Stateful<Div> {
    fn swallow<A: Action>(root: Stateful<Div>) -> Stateful<Div> {
        root.on_action(|_: &A, _, _| {})
    }
    let root = swallow::<ToggleBold>(root);
    let root = swallow::<ToggleItalic>(root);
    let root = swallow::<ToggleUnderline>(root);
    let root = swallow::<ToggleStrikethrough>(root);
    let root = swallow::<FillDown>(root);
    let root = swallow::<FillRight>(root);
    let root = swallow::<AutoSum>(root);
    let root = swallow::<InsertDate>(root);
    let root = swallow::<InsertTime>(root);
    let root = swallow::<FormatCells>(root);
    swallow::<SelectCurrentRegion>(root)
}
