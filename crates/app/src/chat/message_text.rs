use std::time::Duration;

use gpui_kit::component::text::{MarkdownExtensions, TextView, TextViewState};
use gpui_kit::*;

use super::ChatPanel;
use super::badge::ReferencePlugin;
use super::reference_scan;
use crate::markdown_depth::{self, Refusal};
use crate::markdown_pane::claim_html_and_math;

// Between two checks of a message that is still growing past the synchronous size.
const CHECK_DELAY: Duration = Duration::from_millis(150);

enum Shown {
    Markdown(Entity<TextViewState>),
    Plain,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Extensions {
    NotDrawn,
    Drawing,
    Attached,
}

// One assistant message. Text reaches the text view only after the depth check passed on all
// of it, so hostile Markdown is shown as plain text instead of being parsed and laid out.
pub(super) struct MessageText {
    panel: WeakEntity<ChatPanel>,
    raw: String,
    complete: bool,
    shown: Shown,
    // The text view parses with the extensions of the element that draws it, so text waits
    // until a frame has drawn it; the checked tree is then the one that is rendered.
    extensions: Extensions,
    version: u64,
    settled: u64,
    running: bool,
    check: Option<Task<()>>,
}

// What the text view is given for a message: once the turn is over, its references are links.
fn checked_text(raw: &str, complete: bool) -> Result<String, Refusal> {
    if raw.len() > markdown_depth::MAX_PREVIEW_BYTES {
        return Err(Refusal::TooLarge);
    }
    let text = if complete {
        reference_scan::link_references(raw).into_owned()
    } else {
        raw.to_owned()
    };
    match markdown_depth::preview_refusal(&text) {
        Some(refusal) => Err(refusal),
        None => Ok(text),
    }
}

fn extensions(panel: WeakEntity<ChatPanel>) -> MarkdownExtensions {
    claim_html_and_math(
        MarkdownExtensions::default()
            .parser_revision(1)
            .plugin(ReferencePlugin::new(panel)),
    )
}

impl MessageText {
    pub(super) fn new(
        panel: WeakEntity<ChatPanel>,
        first: &str,
        cx: &mut Context<Self>,
    ) -> MessageText {
        MessageText {
            panel,
            raw: first.to_owned(),
            complete: false,
            shown: Shown::Markdown(cx.new(|cx| TextViewState::markdown("", cx))),
            extensions: Extensions::NotDrawn,
            version: 1,
            settled: 0,
            running: false,
            check: None,
        }
    }

    pub(super) fn append(&mut self, added: &str, cx: &mut Context<Self>) {
        self.raw.push_str(added);
        self.version += 1;
        self.catch_up(cx);
        cx.notify();
    }

    pub(super) fn complete(&mut self, cx: &mut Context<Self>) {
        if self.complete {
            return;
        }
        self.complete = true;
        self.version += 1;
        self.catch_up(cx);
        cx.notify();
    }

    fn attach_extensions(&mut self, cx: &mut Context<Self>) {
        self.extensions = Extensions::Attached;
        self.catch_up(cx);
    }

    // Hands the text view what has been checked. Up to the size the text view parses on the UI
    // thread the check is made here; past it, in the background, one check at a time, each
    // over everything received so far.
    fn catch_up(&mut self, cx: &mut Context<Self>) {
        if self.extensions != Extensions::Attached
            || self.running
            || self.settled == self.version
            || matches!(self.shown, Shown::Plain)
        {
            return;
        }
        if self.raw.len() <= markdown_depth::SYNC_BYTES {
            let checked = checked_text(&self.raw, self.complete);
            self.settle(self.version, checked, cx);
            return;
        }
        self.running = true;
        self.check = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CHECK_DELAY).await;
                let Ok((version, raw, complete)) = this.update(cx, |message, _| {
                    (message.version, message.raw.clone(), message.complete)
                }) else {
                    return;
                };
                let checked = cx
                    .background_executor()
                    .spawn(async move { checked_text(&raw, complete) })
                    .await;
                match this.update(cx, |message, cx| message.settle(version, checked, cx)) {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(error) => {
                        tracing::debug!(%error, "chat message dropped before its check finished");
                        return;
                    }
                }
            }
        }));
    }

    // Applies a verdict and says whether the message has grown since, so another check is due.
    fn settle(
        &mut self,
        version: u64,
        checked: Result<String, Refusal>,
        cx: &mut Context<Self>,
    ) -> bool {
        match checked {
            Err(refusal) => {
                tracing::debug!(?refusal, "chat message shown as plain text");
                self.shown = Shown::Plain;
                self.running = false;
            }
            Ok(text) => {
                if let Shown::Markdown(state) = &self.shown {
                    state.update(cx, |state, cx| state.set_text(&text, cx));
                }
                self.settled = version;
                self.running = self.settled != self.version;
            }
        }
        cx.notify();
        self.running
    }
}

impl Render for MessageText {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.extensions == Extensions::NotDrawn {
            self.extensions = Extensions::Drawing;
            let message = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                if let Err(error) = message.update(cx, |message, cx| message.attach_extensions(cx))
                {
                    tracing::debug!(%error, "chat message dropped before it was drawn");
                }
            });
        }
        match &self.shown {
            Shown::Markdown(state) => TextView::new(state)
                .markdown_extensions(extensions(self.panel.clone()))
                .selectable(true)
                .stream_fade(true)
                .line_height(relative(1.6))
                .into_any_element(),
            Shown::Plain => div()
                .line_height(relative(1.6))
                .child(self.raw.clone())
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::reference_scan::LINK_TARGET;
    use super::{Refusal, checked_text, markdown_depth};

    fn hostile_messages() -> Vec<(&'static str, String)> {
        vec![
            ("quotes", ">".repeat(5_000)),
            (
                "lists",
                (0..200)
                    .map(|level| format!("{}- a\n", "  ".repeat(level)))
                    .collect(),
            ),
            ("emphasis", format!("{}x", "*a ".repeat(2_000))),
            ("html", format!("x{}y", "<b>".repeat(5_000))),
            ("brackets", "[a".repeat(3_000)),
        ]
    }

    #[test]
    fn an_ordinary_answer_is_handed_over_unchanged() {
        let answer =
            "# Total\n\nThe **sum** is `42`, see the [docs](https://example.com).\n\n- a\n- b\n";
        assert_eq!(checked_text(answer, false).as_deref(), Ok(answer));
    }

    #[test]
    fn a_hostile_message_is_refused_so_it_is_shown_as_plain_text() {
        for (name, text) in hostile_messages() {
            assert!(checked_text(&text, false).is_err(), "{name}");
            assert!(checked_text(&text, true).is_err(), "{name} once complete");
        }
    }

    #[test]
    fn a_message_past_the_preview_size_is_refused_without_a_parse() {
        let long = "word ".repeat(markdown_depth::MAX_PREVIEW_BYTES / 4);
        assert_eq!(checked_text(&long, false), Err(Refusal::TooLarge));
    }

    #[test]
    fn a_message_turns_hostile_at_the_chunk_that_makes_it_so() {
        let chunks = ["Here is the result:\n\n", "> a\n", &">".repeat(100)];
        let mut received = String::new();
        let verdicts: Vec<bool> = chunks
            .iter()
            .map(|chunk| {
                received.push_str(chunk);
                checked_text(&received, false).is_ok()
            })
            .collect();
        assert_eq!(verdicts, [true, true, false]);
    }

    #[test]
    fn a_completed_message_is_checked_with_its_references_linked() {
        let linked = checked_text("see Sales!B4 now", true).unwrap();
        assert_eq!(linked, format!("see [Sales!B4]({LINK_TARGET}) now"));
        assert_eq!(
            checked_text("see Sales!B4 now", false).unwrap(),
            "see Sales!B4 now"
        );
    }

    #[test]
    fn html_and_math_in_a_message_pass_when_within_the_budgets() {
        assert!(checked_text("<b>bold</b> and $x^2$ and\n\n<div>block</div>\n", false).is_ok());
    }
}
