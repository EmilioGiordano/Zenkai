use gpui_kit::*;

use super::Workspace;

impl Workspace {
    // Every blocking question goes through here so the gate stays shut for exactly as long as
    // the dialog is open, whatever the platform does with the owner window.
    pub(super) fn ask<T>(
        &mut self,
        level: PromptLevel,
        message: &str,
        detail: Option<&str>,
        answers: &[T],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<usize>>
    where
        T: Clone + Into<PromptButton>,
    {
        let answer = window.prompt(level, message, detail, answers, cx);
        let guard = self.prompts.open();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let choice = match answer.await {
                Ok(choice) => Some(choice),
                Err(error) => {
                    tracing::warn!(%error, "a prompt closed without an answer");
                    None
                }
            };
            drop(guard);
            if let Err(error) = this.update(cx, |_, cx| cx.notify()) {
                tracing::debug!(%error, "workspace closed during a prompt");
            }
            choice
        })
    }
}
