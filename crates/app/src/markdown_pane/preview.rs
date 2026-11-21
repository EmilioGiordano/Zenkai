use std::rc::Rc;
use std::time::Duration;

use gpui_kit::*;

use super::MarkdownPane;
use crate::markdown_depth::{self, Refusal};
use crate::markdown_render::{Block, Mode, blocks_of};
use crate::text_document::ViewMode;

const PREVIEW_DELAY: Duration = Duration::from_millis(150);
const FOLLOW_DELAY: Duration = Duration::from_millis(120);

// The lines of a text, or why it is not drawn. This is the work of a preview: the parse, the
// checks and the walk that makes the lines.
fn lines_of(text: &str) -> Result<Vec<Block>, Refusal> {
    let tree = markdown_depth::preview_tree(text)?;
    blocks_of(&tree, Mode::Document).ok_or(Refusal::TooLarge)
}

impl MarkdownPane {
    pub(super) fn schedule_preview(&mut self, cx: &mut Context<Self>) {
        if self.mode == ViewMode::Source {
            return;
        }
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DELAY).await;
            if let Err(error) = this.update(cx, |this, cx| this.refresh_preview(cx)) {
                tracing::debug!(%error, "markdown pane closed before its preview refreshed");
            }
        }));
    }

    pub(super) fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value();
        self.show_in_preview(&text, cx);
    }

    // A long text is checked and turned into lines in the background and the preview waits for
    // the answer; a short one is done at once.
    pub(super) fn show_in_preview(&mut self, text: &str, cx: &mut Context<Self>) {
        self.preview_stale = false;
        if text.len() > markdown_depth::MAX_PREVIEW_BYTES
            || text.len() <= markdown_depth::SYNC_BYTES
        {
            self.depth_task = None;
            self.apply_preview(lines_of(text), cx);
            return;
        }
        let text = text.to_owned();
        self.depth_task = Some(cx.spawn(async move |this, cx| {
            let lines = cx
                .background_executor()
                .spawn(async move { lines_of(&text) })
                .await;
            if let Err(error) = this.update(cx, |this, cx| this.apply_preview(lines, cx)) {
                tracing::debug!(%error, "markdown pane closed before its preview was ready");
            }
        }));
    }

    // The list keeps its place in the text while the lines are replaced, so a preview that
    // follows an edit does not jump back to the top.
    fn apply_preview(&mut self, lines: Result<Vec<Block>, Refusal>, cx: &mut Context<Self>) {
        let (refusal, lines) = match lines {
            Ok(lines) => (None, lines),
            Err(refusal) => (Some(refusal), Vec::new()),
        };
        let old = self.lines.len();
        self.refusal = refusal;
        self.lines = Rc::new(
            lines
                .into_iter()
                .map(|block| {
                    let text = SharedString::from(block.text.clone());
                    (block, text)
                })
                .collect(),
        );
        self.list.splice(0..old, self.lines.len());
        cx.notify();
    }

    pub(super) fn schedule_follow(&mut self, cx: &mut Context<Self>) {
        if self.mode != ViewMode::Split {
            return;
        }
        self.follow_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FOLLOW_DELAY).await;
            if let Err(error) = this.update(cx, |this, cx| this.follow_caret(cx)) {
                tracing::debug!(%error, "markdown pane closed before following the caret");
            }
        }));
    }

    // Scrolls the preview to the line the caret is in: the last one that begins at or before it.
    pub(super) fn follow_caret(&mut self, cx: &mut Context<Self>) {
        let offset = self.editor.read(cx).cursor();
        if offset == self.followed {
            return;
        }
        self.followed = offset;
        if let Some(index) = self
            .lines
            .iter()
            .rposition(|(block, _)| block.start <= offset)
        {
            self.list.scroll_to_reveal_item(index);
        }
    }
}
