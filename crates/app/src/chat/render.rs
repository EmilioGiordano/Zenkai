use gpui_kit::base::v_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::presets::Preset;
use zenkai_agent::settings::Settings;
use zenkai_i18n::t;

use super::launch::chosen_agent;
use super::{ChatEvent, ChatPanel, View};
use crate::actions::*;
use crate::agent_settings::AgentConfig;

const PANEL_WIDTH_REMS: f32 = 29.0;

fn provider_name(settings: &Settings) -> String {
    match chosen_agent(settings) {
        Some((id, server)) => {
            Preset::for_agent(&id).map_or_else(|| server.name, |preset| preset.provider.to_string())
        }
        None => t!("chat.provider.unknown").to_string(),
    }
}

impl Render for ChatPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut context = KeyContext::default();
        context.add("AgentChat");
        if self.busy() {
            context.add("Working");
        }
        if self.slash_open(cx) {
            context.add("Slash");
        }
        if self.menu.is_some() {
            context.add("Menu");
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
        let sessions = if self.view == View::Sessions {
            Some(self.render_sessions(cx))
        } else {
            None
        };
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
        let chat_view = self.view == View::Chat;
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
            .on_action(cx.listener(|this, _: &ShowChatSessions, window, cx| {
                this.toggle_sessions(window, cx)
            }))
            .on_action(cx.listener(|this, _: &PickChatModel, _, cx| this.toggle_model_menu(cx)))
            .on_action(cx.listener(|this, _: &CycleChatMode, _, cx| this.cycle_mode(cx)))
            .on_action(cx.listener(|this, _: &SlashNext, _, cx| this.slash_step(true, cx)))
            .on_action(cx.listener(|this, _: &SlashPrevious, _, cx| this.slash_step(false, cx)))
            .on_action(
                cx.listener(|this, _: &SlashAccept, window, cx| this.slash_accept(window, cx)),
            )
            .on_action(cx.listener(|this, _: &SlashClose, _, cx| this.slash_close(cx)))
            .child(self.render_header(cx))
            .children(sessions)
            .when(chat_view, |root| root.child(body))
            .when(chat_view, |root| {
                root.child(self.render_composer(window, cx))
            })
            .child(
                div()
                    .flex_shrink_0()
                    .px_4()
                    .pb_3()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("chat.provider.notice", provider = provider)),
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
