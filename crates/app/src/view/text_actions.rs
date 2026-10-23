use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::{Severity, Workspace};
use crate::markdown_links::{LinkTarget, classify_link, host_of, shown_target};
use crate::markdown_pane::folder_of;

impl Workspace {
    // A link in a document is untrusted: the web asks first, files open only if Zenkai
    // opens them, and anything else is refused with a notice.
    pub(super) fn open_document_link(
        &mut self,
        id: WorkbookId,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folder = self
            .documents
            .text(id)
            .and_then(|text| folder_of(text.path.as_deref()));
        match classify_link(url, folder.as_deref()) {
            LinkTarget::Anchor => {}
            LinkTarget::Web(url) => self.confirm_web_link(url, window, cx),
            LinkTarget::File(path) => self.open_path(path, window, cx),
            LinkTarget::Refused => self.notify(
                Severity::Warning,
                t!("markdown.link.refused", target = shown_target(url)),
                cx,
            ),
        }
    }

    pub(super) fn confirm_web_link(
        &mut self,
        url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = self.ask(
            PromptLevel::Warning,
            t!("markdown.link.web_title"),
            Some(&t!(
                "markdown.link.web_detail",
                host = host_of(&url),
                link = shown_target(&url)
            )),
            &[t!("markdown.link.open_browser"), t!("button.cancel")],
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Some(0) {
                let update = this.update(cx, |_, cx| cx.open_url(&url));
                if let Err(error) = update {
                    tracing::debug!(%error, "workspace closed before a link opened");
                }
            }
        })
        .detach();
    }

    // Find and Replace of a text are the editor's own: the source is shown and takes focus first.
    pub(super) fn find_in_text(
        &mut self,
        replace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.shown_pane().map(|entry| entry.pane.clone()) else {
            return false;
        };
        self.palette = None;
        self.revert_theme_preview(cx);
        pane.update(cx, |pane, cx| pane.open_find(replace, window, cx));
        true
    }
}
