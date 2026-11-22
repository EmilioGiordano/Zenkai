use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use gpui_kit::component::input::RopeExt;
use gpui_kit::component::text::{
    InlineElement, InlineRenderContext, MarkdownExtensions, MarkdownNode, MarkdownParseContext,
    MarkdownPlugin, markdown_ast,
};
use gpui_kit::*;

use super::MarkdownPane;
use crate::markdown_depth::{self, Refusal};
use crate::markdown_images::{image_path, image_urls, vet_image};
use crate::text_document::ViewMode;

const PREVIEW_DELAY: Duration = Duration::from_millis(150);
const FOLLOW_DELAY: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Extensions {
    NotDrawn,
    Drawing,
    Attached,
}

// The verdict on each image the document names, found off the UI thread: the preview shows an
// image only once its path has been checked, and nothing is read from disk while drawing.
pub(super) type Images = Arc<Mutex<HashMap<PathBuf, Option<PathBuf>>>>;

pub(super) fn verdicts(images: &Images) -> MutexGuard<'_, HashMap<PathBuf, Option<PathBuf>>> {
    images.lock().unwrap_or_else(PoisonError::into_inner)
}

// Raw HTML in a document is shown as the text it is, never interpreted.
struct HtmlAsText {
    block: bool,
}

impl MarkdownPlugin for HtmlAsText {
    fn is_block(&self) -> bool {
        self.block
    }

    fn name(&self) -> &str {
        if self.block {
            "zenkai-html-block"
        } else {
            "zenkai-html-inline"
        }
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        _: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::Html(html) = node else {
            return None;
        };
        Some(MarkdownNode::new(self.name().to_owned(), ()).text(html.value.clone()))
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        _: &InlineRenderContext,
        _: &mut Window,
        _: &mut App,
    ) -> Option<InlineElement> {
        Some(InlineElement::new(node.as_text().to_string()))
    }
}

// A math span is shown as the text it was written as. A span nobody claims is parsed again as
// prose by the text view, which would build a tree the depth check never measured.
struct MathAsText;

impl MarkdownPlugin for MathAsText {
    fn name(&self) -> &str {
        "zenkai-math"
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        context: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::InlineMath(math) = node else {
            return None;
        };
        let written = context
            .node_source(node)
            .map_or_else(|| math.value.clone(), str::to_owned);
        Some(MarkdownNode::new(self.name().to_owned(), ()).text(written))
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        _: &InlineRenderContext,
        _: &mut Window,
        _: &mut App,
    ) -> Option<InlineElement> {
        Some(InlineElement::new(node.as_text().to_string()))
    }
}

// Every view that shows text the depth check passed claims HTML and math with these, so the
// text view never parses them again as something the check did not measure.
pub(crate) fn claim_html_and_math(extensions: MarkdownExtensions) -> MarkdownExtensions {
    extensions
        .plugin(HtmlAsText { block: false })
        .plugin(HtmlAsText { block: true })
        .plugin(MathAsText)
}

pub(super) fn extensions() -> MarkdownExtensions {
    claim_html_and_math(MarkdownExtensions::default().parser_revision(2))
}

pub(super) fn blocked_image() -> ImageSource {
    ImageSource::Custom(Arc::new(|_, _| None))
}

impl MarkdownPane {
    pub(super) fn vet_images(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(folder) = self.state.folder.clone() else {
            return;
        };
        for url in image_urls(text) {
            let Some(path) = image_path(url, Some(&folder)) else {
                continue;
            };
            let mut known = verdicts(&self.images);
            if known.contains_key(&path) {
                continue;
            }
            known.insert(path.clone(), None);
            drop(known);
            let (images, folder) = (self.images.clone(), folder.clone());
            cx.spawn(async move |this, cx| {
                let target = path.clone();
                let real = cx
                    .background_executor()
                    .spawn(async move { vet_image(&target, &folder) })
                    .await;
                verdicts(&images).insert(path, real);
                if let Err(error) = this.update(cx, |_, cx| cx.notify()) {
                    tracing::debug!(%error, "markdown pane closed before an image was checked");
                }
            })
            .detach();
        }
    }

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

    // Text whose parsed tree nests too deeply is not handed to the text view; the pane says so
    // instead. A long text is measured in the background and the preview waits for the answer.
    pub(super) fn show_in_preview(&mut self, text: &str, cx: &mut Context<Self>) {
        self.vet_images(text, cx);
        self.preview_stale = false;
        if text.len() > markdown_depth::MAX_PREVIEW_BYTES
            || text.len() <= markdown_depth::SYNC_BYTES
        {
            self.depth_task = None;
            self.apply_depth(text, markdown_depth::preview_refusal(text), cx);
            return;
        }
        let text = text.to_owned();
        self.depth_task = Some(cx.spawn(async move |this, cx| {
            let (text, refusal) = cx
                .background_executor()
                .spawn(async move {
                    let refusal = markdown_depth::preview_refusal(&text);
                    (text, refusal)
                })
                .await;
            if let Err(error) = this.update(cx, |this, cx| this.apply_depth(&text, refusal, cx)) {
                tracing::debug!(%error, "markdown pane closed before its depth was checked");
            }
        }));
    }

    fn apply_depth(&mut self, text: &str, refusal: Option<Refusal>, cx: &mut Context<Self>) {
        self.refusal = refusal;
        let shown = if refusal.is_none() { text } else { "" };
        if self.extensions == Extensions::Attached {
            self.preview
                .update(cx, |preview, cx| preview.set_text(shown, cx));
        } else {
            self.held_text = Some(shown.to_owned());
        }
        cx.notify();
    }

    // The text view parses with the extensions it has when it is given text, and takes them from
    // the element that draws it. So text waits until a frame has drawn the preview, and the
    // checked tree is the one that is rendered, with HTML and math claimed.
    pub(super) fn await_extensions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.extensions = Extensions::Drawing;
        let pane = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            if let Err(error) = pane.update(cx, |pane, cx| pane.attach_extensions(cx)) {
                tracing::debug!(%error, "markdown pane closed before its preview was drawn");
            }
        });
    }

    fn attach_extensions(&mut self, cx: &mut Context<Self>) {
        self.extensions = Extensions::Attached;
        if let Some(text) = self.held_text.take() {
            self.preview
                .update(cx, |preview, cx| preview.set_text(&text, cx));
        }
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

    // Scrolls the preview to what the caret's line renders; a blank line looks upwards for
    // the nearest line that renders something.
    pub(super) fn follow_caret(&mut self, cx: &mut Context<Self>) {
        let (offset, rows) = {
            let editor = self.editor.read(cx);
            (editor.cursor(), editor.text().clone())
        };
        if offset == self.followed {
            return;
        }
        self.followed = offset;
        let rendered = self.preview.read(cx).rendered_text();
        let mut row = rows.offset_to_point(offset).row;
        let range = loop {
            let start = rows.line_start_offset(row);
            let end = rows.line_end_offset(row).min(rows.len());
            if let Some(range) = rendered.range_for_source(start..end) {
                break Some(range);
            }
            if row == 0 {
                break None;
            }
            row -= 1;
        };
        let Some(range) = range else {
            return;
        };
        let revealed = self
            .preview
            .update(cx, |preview, cx| preview.reveal_range(range, cx));
        if let Err(error) = revealed {
            tracing::debug!(?error, "the preview could not follow the caret");
        }
    }
}
