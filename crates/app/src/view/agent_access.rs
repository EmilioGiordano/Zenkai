use std::time::Duration;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::*;
use zenkai_agent::protected_view::FileOrigin;
use zenkai_i18n::t;

use super::Workspace;
use crate::actions::LetAgentsEdit;
use crate::assets::{LOCK_MARK, LOCK_OPEN_MARK};
use crate::keymap;

const TOAST_TIME: Duration = Duration::from_secs(3);
const TOAST_TOP: f32 = 96.0;

pub(super) struct Toast {
    generation: u64,
    editable: bool,
    file: String,
}

fn flipped(origin: FileOrigin) -> FileOrigin {
    match origin {
        FileOrigin::Local => FileOrigin::Internet,
        FileOrigin::Internet => FileOrigin::Local,
    }
}

pub(super) fn agents_can_edit(origin: FileOrigin) -> bool {
    origin == FileOrigin::Local
}

impl Workspace {
    // Both ways, per document and for this session only; the Zone.Identifier stream of the
    // file is never touched.
    pub(crate) fn toggle_agent_editing(&mut self, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active_mut() else {
            return;
        };
        document.origin = flipped(document.origin);
        self.announce_agent_access(cx);
    }

    pub(crate) fn allow_agent_editing(&mut self, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active_mut() else {
            return;
        };
        if !agents_can_edit(document.origin) {
            document.origin = FileOrigin::Local;
            self.announce_agent_access(cx);
        }
    }

    fn announce_agent_access(&mut self, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active() else {
            return;
        };
        self.toast_generation += 1;
        let generation = self.toast_generation;
        self.toast = Some(Toast {
            generation,
            editable: agents_can_edit(document.origin),
            file: document.name(),
        });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_TIME).await;
            let hidden = this.update(cx, |this, cx| {
                if this
                    .toast
                    .as_ref()
                    .is_some_and(|toast| toast.generation == generation)
                {
                    this.toast = None;
                    cx.notify();
                }
            });
            if hidden.is_err() {
                tracing::debug!("workspace closed before the notice was hidden");
            }
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_toast(&self, cx: &App) -> Option<impl IntoElement> {
        let toast = self.toast.as_ref()?;
        let theme = cx.theme();
        let keys = keymap::shortcut(cx, &LetAgentsEdit)
            .unwrap_or_else(|| t!("palette.let_agents_edit").to_string());
        let (mark, title, hint) = if toast.editable {
            (
                LOCK_OPEN_MARK,
                t!("toast.agents_can_edit", file = toast.file),
                t!("toast.agents_can_edit.hint", keys = keys),
            )
        } else {
            (
                LOCK_MARK,
                t!("toast.agents_read_only", file = toast.file),
                t!("toast.agents_read_only.hint", keys = keys),
            )
        };
        Some(
            div()
                .absolute()
                .top(px(TOAST_TOP))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    h_flex()
                        .id("agent-access-toast")
                        .role(Role::Status)
                        .aria_label(title.clone())
                        .gap_3()
                        .items_center()
                        .px_4()
                        .py_2p5()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .text_color(theme.popover_foreground)
                        .shadow_md()
                        .child(Icon::empty().path(mark).size_5())
                        .child(
                            v_flex().child(div().child(title)).child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(hint),
                            ),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use zenkai_agent::protected_view::FileOrigin;

    use super::{agents_can_edit, flipped};

    #[test]
    fn toggling_twice_returns_to_the_same_protection() {
        for origin in [FileOrigin::Local, FileOrigin::Internet] {
            assert_eq!(flipped(flipped(origin)), origin);
        }
    }

    #[test]
    fn a_local_file_can_be_protected_and_a_download_unlocked() {
        assert!(agents_can_edit(FileOrigin::Local));
        assert!(!agents_can_edit(flipped(FileOrigin::Local)));
        assert!(agents_can_edit(flipped(FileOrigin::Internet)));
    }
}
