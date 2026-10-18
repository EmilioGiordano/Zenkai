use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use zenkai_agent::capabilities::{Capability, Grant, grant};
use zenkai_agent::tools::{AgentAccess, ReadOnlyReason};
use zenkai_i18n::t;

use super::menus::{heading, note, rule};
use super::{ChatPanel, Link};

fn capability_label(capability: Capability) -> &'static str {
    match capability {
        Capability::ReadWorkbooks => t!("chat.access.cap.read"),
        Capability::OpenWorkbooks => t!("chat.access.cap.open"),
        Capability::EditCells => t!("chat.access.cap.edit"),
        Capability::CreateWorkbooks => t!("chat.access.cap.create"),
        Capability::ChangeStructure => t!("chat.access.cap.structure"),
        Capability::FileSystem => t!("chat.access.cap.files"),
        Capability::Macros => t!("chat.access.cap.macros"),
    }
}

fn grant_label(grant: Grant) -> &'static str {
    match grant {
        Grant::Always => t!("chat.access.grant.always"),
        Grant::AsksFirst => t!("chat.access.grant.asks"),
        Grant::Never => t!("chat.access.grant.never"),
    }
}

fn read_only_note(access: AgentAccess) -> Option<&'static str> {
    match access {
        AgentAccess::Editable | AgentAccess::ReadOnly(ReadOnlyReason::Settings) => None,
        AgentAccess::ReadOnly(ReadOnlyReason::ProtectedView) => {
            Some(t!("chat.access.note.protected_view"))
        }
        AgentAccess::ReadOnly(ReadOnlyReason::ValuesOnlyFile) => {
            Some(t!("chat.access.note.values_only"))
        }
    }
}

impl ChatPanel {
    pub(super) fn access_summary(&self, cx: &App) -> Option<Div> {
        let permission = self.access.setting();
        let file = self
            .workspace
            .read_with(cx, |workspace, _| {
                workspace.agent_access_for_chat(permission)
            })
            .ok()?;
        let theme = cx.theme();
        let mut rows = v_flex().gap_1p5().px_2p5().pb_1p5();
        for capability in Capability::ALL {
            rows = rows.child(
                h_flex()
                    .gap_2p5()
                    .items_baseline()
                    .text_sm()
                    .child(div().flex_1().child(capability_label(capability)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(grant_label(grant(capability, permission, file))),
                    ),
            );
        }
        let summary = v_flex()
            .child(rule(cx))
            .child(heading(t!("chat.access.can_do"), cx))
            .child(rows)
            .child(note(t!("chat.access.note.own_tools"), cx));
        Some(match read_only_note(file) {
            Some(text) => summary.child(note(text, cx)),
            None => summary,
        })
    }

    pub(super) fn server_summary(&self, cx: &App) -> Div {
        let state = match self.link {
            Link::Ready(_) => t!(
                "chat.access.server.offered",
                count = Capability::tool_count()
            ),
            _ => t!("chat.access.server.idle").to_string(),
        };
        v_flex()
            .child(rule(cx))
            .child(heading(t!("chat.access.servers"), cx))
            .child(
                h_flex()
                    .px_2p5()
                    .pb_1p5()
                    .gap_2p5()
                    .text_sm()
                    .child(div().flex_1().child(t!("chat.access.server.zenkai")))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(state),
                    ),
            )
    }
}
