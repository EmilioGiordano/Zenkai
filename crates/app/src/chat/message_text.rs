use std::panic::{self, AssertUnwindSafe};

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

enum Phase {
    Streaming,
    Checking,
    Rich {
        held: Option<String>,
    },
    Refused {
        text: SharedString,
        refusal: Refusal,
    },
}

// One assistant message: plain text while it streams, formatted once it is complete and checked.
// The view is given the checked text once and never more: its append parses an unchecked tail.
pub(super) struct MessageText {
    panel: WeakEntity<ChatPanel>,
    text: SharedString,
    phase: Phase,
    state: Entity<TextViewState>,
    // The text view parses with the extensions of the element that draws it, so text waits
    // until a frame has drawn it; the checked tree is then the one that is rendered.
    extensions: Extensions,
    // Bumped when the view is replaced, so a frame callback of the old view attaches nothing.
    generation: u64,
    check: Option<Task<()>>,
}

// A panic in the parser or the reference scan on a hostile text counts as a text that cannot
// be shown, instead of ending a task unseen.
fn guarded<T>(work: impl FnOnce() -> Result<T, Refusal>) -> Result<T, Refusal> {
    panic::catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|_| {
        tracing::error!("a chat message made its check panic; it is shown as plain text");
        Err(Refusal::TooLarge)
    })
}

fn verdict(text: String) -> Result<String, Refusal> {
    match markdown_depth::preview_refusal(&text) {
        Some(refusal) => Err(refusal),
        None => Ok(text),
    }
}

// The check for the UI thread, on the message with its references linked; Ok(None) leaves it
// for the background when that text is longer than the view parses synchronously.
fn check_here(raw: &str) -> Result<Option<String>, Refusal> {
    guarded(|| {
        let text = reference_scan::link_references(raw);
        if text.len() > markdown_depth::SYNC_BYTES {
            return Ok(None);
        }
        verdict(text.into_owned()).map(Some)
    })
}

fn check(raw: &str) -> Result<String, Refusal> {
    guarded(|| verdict(reference_scan::link_references(raw).into_owned()))
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

fn plain(text: impl Into<SharedString>) -> Div {
    div().line_height(relative(1.6)).child(text.into())
}

impl MessageText {
    pub(super) fn new(
        panel: WeakEntity<ChatPanel>,
        first: &str,
        cx: &mut Context<Self>,
    ) -> MessageText {
        let mut message = MessageText {
            panel,
            text: SharedString::from(first.to_owned()),
            phase: Phase::Streaming,
            state: cx.new(|cx| TextViewState::markdown("", cx)),
            extensions: Extensions::NotDrawn,
            generation: 0,
            check: None,
        };
        message.refuse_if_oversized();
        message
    }

    pub(super) fn append(&mut self, added: &str, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Refused { .. }) {
            return;
        }
        if !matches!(self.phase, Phase::Streaming) {
            self.reopen(cx);
        }
        self.text = SharedString::from(format!("{}{added}", self.text));
        self.refuse_if_oversized();
        cx.notify();
    }

    // The agent went on after the message was closed; the view cannot be given more text, so it
    // gets a new one.
    fn reopen(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.check = None;
        self.state = cx.new(|cx| TextViewState::markdown("", cx));
        self.extensions = Extensions::NotDrawn;
        self.phase = Phase::Streaming;
    }

    fn refuse_if_oversized(&mut self) {
        if self.text.len() > markdown_depth::MAX_PREVIEW_BYTES {
            self.refuse(Refusal::TooLarge);
        }
    }

    fn refuse(&mut self, refusal: Refusal) {
        tracing::debug!(?refusal, "chat message shown as plain text");
        self.phase = Phase::Refused {
            text: start_of(&self.text),
            refusal,
        };
        self.text = SharedString::default();
        self.check = None;
    }

    // The turn is over: the message is checked on the UI thread up to the size the view parses
    // there, and in the background past it.
    pub(super) fn complete(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::Streaming) {
            return;
        }
        match check_here(&self.text) {
            Ok(Some(text)) => self.accept(text, cx),
            Ok(None) => self.check_in_background(cx),
            Err(refusal) => self.refuse(refusal),
        }
        cx.notify();
    }

    fn check_in_background(&mut self, cx: &mut Context<Self>) {
        self.phase = Phase::Checking;
        let raw = self.text.to_string();
        self.check = Some(cx.spawn(async move |this, cx| {
            let checked = cx
                .background_executor()
                .spawn(async move { check(&raw) })
                .await;
            let settled = this.update(cx, |message, cx| {
                if matches!(message.phase, Phase::Checking) {
                    match checked {
                        Ok(text) => message.accept(text, cx),
                        Err(refusal) => message.refuse(refusal),
                    }
                    cx.notify();
                }
            });
            if let Err(error) = settled {
                tracing::debug!(%error, "chat message dropped before its check finished");
            }
        }));
    }

    fn accept(&mut self, text: String, cx: &mut Context<Self>) {
        self.phase = Phase::Rich { held: Some(text) };
        self.hand_over_checked_text(cx);
    }

    fn attach_extensions(&mut self, generation: u64, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        self.extensions = Extensions::Attached;
        self.hand_over_checked_text(cx);
        cx.notify();
    }

    fn hand_over_checked_text(&mut self, cx: &mut Context<Self>) {
        if self.extensions != Extensions::Attached {
            return;
        }
        if let Phase::Rich { held } = &mut self.phase
            && let Some(text) = held.take()
        {
            self.state.update(cx, |state, cx| state.set_text(&text, cx));
        }
    }
}

impl Render for MessageText {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.phase {
            Phase::Streaming | Phase::Checking => plain(self.text.clone()).into_any_element(),
            Phase::Rich { held } => {
                if self.extensions == Extensions::NotDrawn {
                    self.extensions = Extensions::Drawing;
                    let message = cx.entity().downgrade();
                    let generation = self.generation;
                    window.on_next_frame(move |_, cx| {
                        if let Err(error) = message
                            .update(cx, |message, cx| message.attach_extensions(generation, cx))
                        {
                            tracing::debug!(%error, "chat message dropped before it was drawn");
                        }
                    });
                }
                let formatted = TextView::new(&self.state)
                    .markdown_extensions(extensions(self.panel.clone()))
                    .selectable(true)
                    .line_height(relative(1.6));
                if held.is_some() {
                    v_flex()
                        .child(plain(self.text.clone()))
                        .child(formatted)
                        .into_any_element()
                } else {
                    formatted.into_any_element()
                }
            }
            Phase::Refused { text, refusal } => v_flex()
                .gap_1()
                .child(plain(text.clone()))
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
            ("parser panic", "1. $$\n-".to_owned()),
        ]
    }

    #[test]
    fn an_ordinary_answer_is_handed_over_unchanged() {
        let answer =
            "# Total\n\nThe **sum** is `42`, see the [docs](https://example.com).\n\n- a\n- b\n";
        assert_eq!(check(answer).as_deref(), Ok(answer));
        assert_eq!(check_here(answer), Ok(Some(answer.to_owned())));
    }

    #[test]
    fn a_hostile_message_is_refused_so_it_is_shown_as_plain_text() {
        for (name, text) in hostile_messages() {
            assert!(check(&text).is_err(), "{name}");
        }
    }

    #[test]
    fn a_short_hostile_message_is_refused_on_the_ui_thread() {
        assert_eq!(check_here(&">".repeat(100)), Err(Refusal::TooDeep));
        assert_eq!(check_here("1. $$\n-"), Err(Refusal::TooLarge));
    }

    #[test]
    fn a_message_past_the_preview_size_is_refused_without_a_parse() {
        let long = "word ".repeat(markdown_depth::MAX_PREVIEW_BYTES / 4);
        assert_eq!(check(&long), Err(Refusal::TooLarge));
    }

    #[test]
    fn the_references_of_a_message_are_checked_as_links() {
        let linked = check("see Sales!B4 now").unwrap();
        assert_eq!(linked, format!("see [Sales!B4]({LINK_TARGET}) now"));
    }

    #[test]
    fn html_and_math_in_a_message_pass_when_within_the_budgets() {
        assert!(check("<b>bold</b> and $x^2$ and\n\n<div>block</div>\n").is_ok());
    }

    #[test]
    fn a_message_that_links_past_the_synchronous_size_is_left_for_the_background() {
        let raw = "Sales!B4 ".repeat(markdown_depth::SYNC_BYTES / 9);
        assert!(raw.len() <= markdown_depth::SYNC_BYTES);
        assert_eq!(check_here(&raw), Ok(None));
        assert!(check_here("Sales!B4 and the rest").unwrap().is_some());
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
