use std::borrow::Cow;
use std::panic::{self, AssertUnwindSafe};
use std::time::Duration;

use gpui_kit::base::v_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::text::{MarkdownExtensions, TextView, TextViewState};
use gpui_kit::*;
use zenkai_i18n::t;

use super::ChatPanel;
use super::badge::ReferencePlugin;
use super::reference_scan;
use crate::markdown_depth::{self, Refusal};
use crate::markdown_pane::{Extensions, claim_html_and_math};

// Between two checks of a message that is still growing past the synchronous size.
const CHECK_DELAY: Duration = Duration::from_millis(150);

enum Shown {
    Markdown(Entity<TextViewState>),
    Plain {
        text: SharedString,
        refusal: Refusal,
    },
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

// A panic in the parser or the reference scan on a hostile text must not end a task unseen:
// the text counts as one that cannot be shown.
fn guarded<T>(work: impl FnOnce() -> Result<T, Refusal>) -> Result<T, Refusal> {
    panic::catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| {
        tracing::error!("a chat message made its check panic; it is shown as plain text");
        Err(Refusal::TooLarge)
    })
}

// What the text view is given for a message: once the turn is over, its references are links.
fn linked(raw: &str, complete: bool) -> Cow<'_, str> {
    if complete {
        reference_scan::link_references(raw)
    } else {
        Cow::Borrowed(raw)
    }
}

fn verdict(text: String) -> Result<String, Refusal> {
    match markdown_depth::preview_refusal(&text) {
        Some(refusal) => Err(refusal),
        None => Ok(text),
    }
}

// The check for the UI thread: Ok(None) when the text to check, once linked, is longer than
// what the text view parses synchronously, so it is left for the background.
fn check_here(raw: &str, complete: bool) -> Result<Option<String>, Refusal> {
    guarded(|| {
        let text = linked(raw, complete);
        if text.len() > markdown_depth::SYNC_BYTES {
            return Ok(None);
        }
        verdict(text.into_owned()).map(Some)
    })
}

fn check(raw: &str, complete: bool) -> Result<String, Refusal> {
    guarded(|| {
        if raw.len() > markdown_depth::MAX_PREVIEW_BYTES {
            return Err(Refusal::TooLarge);
        }
        verdict(linked(raw, complete).into_owned())
    })
}

// The start of a refused message, cut on a character boundary, for what is shown instead.
fn start_of(raw: &str) -> SharedString {
    let end = (0..=raw.len().min(markdown_depth::MAX_PREVIEW_BYTES))
        .rev()
        .find(|index| raw.is_char_boundary(*index))
        .unwrap_or(0);
    SharedString::from(raw[..end].to_owned())
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

    fn is_plain(&self) -> bool {
        matches!(self.shown, Shown::Plain { .. })
    }

    pub(super) fn append(&mut self, added: &str, cx: &mut Context<Self>) {
        if self.is_plain() {
            return;
        }
        self.raw.push_str(added);
        self.version += 1;
        self.hand_over_checked_text(cx);
        cx.notify();
    }

    pub(super) fn complete(&mut self, cx: &mut Context<Self>) {
        if self.complete || self.is_plain() {
            return;
        }
        self.complete = true;
        self.version += 1;
        self.hand_over_checked_text(cx);
        cx.notify();
    }

    fn attach_extensions(&mut self, cx: &mut Context<Self>) {
        self.extensions = Extensions::Attached;
        self.hand_over_checked_text(cx);
    }

    // Gives the text view what has been checked. What the text view parses on the UI thread is
    // checked here and never more than that; anything longer is checked in the background, one
    // check at a time, each over everything received so far.
    fn hand_over_checked_text(&mut self, cx: &mut Context<Self>) {
        if self.extensions != Extensions::Attached
            || self.running
            || self.settled == self.version
            || self.is_plain()
        {
            return;
        }
        if self.raw.len() > markdown_depth::MAX_PREVIEW_BYTES {
            self.settle(self.version, Err(Refusal::TooLarge), cx);
            return;
        }
        match check_here(&self.raw, self.complete) {
            Ok(Some(text)) => self.settle(self.version, Ok(text), cx),
            Ok(None) => {
                self.check_in_background(cx);
                false
            }
            Err(refusal) => self.settle(self.version, Err(refusal), cx),
        };
    }

    fn check_in_background(&mut self, cx: &mut Context<Self>) {
        self.running = true;
        self.check = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CHECK_DELAY).await;
                let Ok(snapshot) = this.update(cx, |message, cx| message.snapshot(cx)) else {
                    return;
                };
                let Some((version, raw, complete)) = snapshot else {
                    return;
                };
                let checked = cx
                    .background_executor()
                    .spawn(async move { check(&raw, complete) })
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

    // What the next background check works on; a message that has outgrown what is checked at
    // all is refused here, so its text is never copied.
    fn snapshot(&mut self, cx: &mut Context<Self>) -> Option<(u64, String, bool)> {
        if self.raw.len() > markdown_depth::MAX_PREVIEW_BYTES {
            self.settle(self.version, Err(Refusal::TooLarge), cx);
            return None;
        }
        Some((self.version, self.raw.clone(), self.complete))
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
                self.shown = Shown::Plain {
                    text: start_of(&self.raw),
                    refusal,
                };
                self.raw = String::new();
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
            Shown::Plain { text, refusal } => v_flex()
                .gap_1()
                .child(div().line_height(relative(1.6)).child(text.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(match refusal {
                            Refusal::TooDeep => t!("chat.plain.too_deep"),
                            Refusal::TooLarge => t!("chat.plain.too_large"),
                        }),
                )
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::reference_scan::LINK_TARGET;
    use super::{Refusal, check, check_here, guarded, markdown_depth, start_of};

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
        assert_eq!(check(answer, false).as_deref(), Ok(answer));
        assert_eq!(check_here(answer, false), Ok(Some(answer.to_owned())));
    }

    #[test]
    fn a_hostile_message_is_refused_so_it_is_shown_as_plain_text() {
        for (name, text) in hostile_messages() {
            assert!(check(&text, false).is_err(), "{name}");
            assert!(check(&text, true).is_err(), "{name} once complete");
        }
    }

    #[test]
    fn a_short_hostile_message_is_refused_on_the_ui_thread() {
        assert_eq!(check_here(&">".repeat(100), false), Err(Refusal::TooDeep));
    }

    #[test]
    fn a_message_past_the_preview_size_is_refused_without_a_parse() {
        let long = "word ".repeat(markdown_depth::MAX_PREVIEW_BYTES / 4);
        assert_eq!(check(&long, false), Err(Refusal::TooLarge));
    }

    #[test]
    fn a_message_turns_hostile_at_the_chunk_that_makes_it_so() {
        let chunks = ["Here is the result:\n\n", "> a\n", &">".repeat(100)];
        let mut received = String::new();
        let verdicts: Vec<bool> = chunks
            .iter()
            .map(|chunk| {
                received.push_str(chunk);
                check(&received, false).is_ok()
            })
            .collect();
        assert_eq!(verdicts, [true, true, false]);
    }

    #[test]
    fn a_completed_message_is_checked_with_its_references_linked() {
        let linked = check("see Sales!B4 now", true).unwrap();
        assert_eq!(linked, format!("see [Sales!B4]({LINK_TARGET}) now"));
        assert_eq!(
            check("see Sales!B4 now", false).unwrap(),
            "see Sales!B4 now"
        );
    }

    #[test]
    fn html_and_math_in_a_message_pass_when_within_the_budgets() {
        assert!(check("<b>bold</b> and $x^2$ and\n\n<div>block</div>\n", false).is_ok());
    }

    #[test]
    fn a_message_that_links_past_the_synchronous_size_is_left_for_the_background() {
        let raw = "Sales!B4 ".repeat(markdown_depth::SYNC_BYTES / 9);
        assert!(raw.len() <= markdown_depth::SYNC_BYTES);
        assert_eq!(check_here(&raw, false), Ok(Some(raw.clone())));
        assert_eq!(check_here(&raw, true), Ok(None));
    }

    #[test]
    fn a_panic_while_checking_is_a_refusal() {
        let panicked: Result<(), Refusal> = guarded(|| panic!("hostile"));
        assert_eq!(panicked, Err(Refusal::TooLarge));
    }

    #[test]
    fn a_refused_message_keeps_at_most_the_preview_size_cut_on_a_character_boundary() {
        let wide = "\u{e9}".repeat(markdown_depth::MAX_PREVIEW_BYTES);
        let shown = start_of(&wide);
        assert!(shown.len() <= markdown_depth::MAX_PREVIEW_BYTES);
        assert!(shown.len() >= markdown_depth::MAX_PREVIEW_BYTES - 1);
        assert_eq!(start_of("short"), "short");
    }
}
