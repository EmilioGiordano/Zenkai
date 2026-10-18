use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::text::{
    InlineElement, InlineRenderContext, MarkdownNode, MarkdownPlugin, markdown_ast,
};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;
use zenkai_i18n::t;

use super::ChatPanel;
use super::mention_label::Label;
use super::mention_parse::{self, Chip};
use super::reference::Reference;
use super::reference_scan::{self, LINK_TARGET};
use crate::interface_scale::rem_px;

const BADGE_HEIGHT: f32 = 20.0;
const ICON_SIZE: f32 = 14.0;

fn frame(id: impl Into<ElementId>, label: &Label, clickable: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let accent = theme.success;
    let full = label.full.clone();
    let mut parts = h_flex().gap_1().items_center().whitespace_nowrap();
    if let Some(workbook) = &label.workbook {
        parts = parts
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(workbook.clone()),
            )
            .child(div().text_color(theme.muted_foreground).child("/"));
    }
    let frame = h_flex()
        .id(id)
        .flex_shrink_0()
        .gap_1()
        .items_center()
        .aria_label(label.full.clone())
        .h(rem_px(BADGE_HEIGHT))
        .text_color(accent)
        .font_weight(FontWeight::MEDIUM)
        .text_size(rem_px(13.0))
        .whitespace_nowrap()
        .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
        .child(
            svg()
                .path(label.kind.icon())
                .size(rem_px(ICON_SIZE))
                .flex_shrink_0()
                .text_color(accent),
        )
        .child(parts.child(label.name.clone()));
    if clickable {
        frame.hover(|style| style.underline())
    } else {
        frame
    }
}

fn open_on_click(
    panel: WeakEntity<ChatPanel>,
    reference: Reference,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) {
    move |_, window, cx| {
        super::closed(panel.update(cx, |panel, cx| panel.open_reference(&reference, window, cx)));
    }
}

pub(super) fn badge(
    reference: &Reference,
    book: Option<&str>,
    panel: WeakEntity<ChatPanel>,
    cx: &App,
) -> impl IntoElement {
    let label = Label::of_reference(reference, book);
    let id = SharedString::from(format!("reference-{}", reference.text()));
    frame(id, &label, true, cx)
        .role(Role::Button)
        .cursor_pointer()
        .on_click(open_on_click(panel, reference.clone()))
}

pub(super) fn chip(
    index: usize,
    reference: &Reference,
    book: Option<&str>,
    panel: WeakEntity<ChatPanel>,
    cx: &App,
) -> impl IntoElement {
    let label = Label::of_reference(reference, book);
    let target = reference.clone();
    let removal = panel.clone();
    frame(("chat-chip", index), &label, true, cx)
        .role(Role::Button)
        .h(rem_px(BADGE_HEIGHT + 2.0))
        .cursor_pointer()
        .on_click(open_on_click(panel, target))
        .child(
            Button::new(("chat-chip-remove", index))
                .ghost()
                .compact()
                .icon(IconName::X)
                .tooltip(t!("chat.reference.remove"))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    super::closed(
                        removal.update(cx, |panel, cx| panel.remove_reference(index, cx)),
                    );
                }),
        )
}

fn plain_text(children: &[markdown_ast::Node]) -> String {
    children
        .iter()
        .map(|child| match child {
            markdown_ast::Node::Text(text) => text.value.clone(),
            other => other
                .children()
                .map_or_else(String::new, |nested| plain_text(nested)),
        })
        .collect()
}

pub(super) struct ReferencePlugin {
    panel: WeakEntity<ChatPanel>,
}

impl ReferencePlugin {
    pub(super) fn new(panel: WeakEntity<ChatPanel>) -> ReferencePlugin {
        ReferencePlugin { panel }
    }
}

impl MarkdownPlugin for ReferencePlugin {
    fn name(&self) -> &str {
        "zenkai-reference"
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        _: &gpui_kit::component::text::MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let reference = match node {
            markdown_ast::Node::Link(link) if link.url == LINK_TARGET => {
                reference_scan::parse_exact(&plain_text(&link.children))?
            }
            markdown_ast::Node::InlineCode(code) => reference_scan::parse_exact(&code.value)?,
            _ => return None,
        };
        let text = reference.text();
        Some(MarkdownNode::new("zenkai-reference", reference).text(text))
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        _: &InlineRenderContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let reference = node.data::<Reference>()?;
        Some(InlineElement::new(badge(
            reference,
            None,
            self.panel.clone(),
            cx,
        )))
    }
}

fn mention_chip(
    chip: &Chip,
    book: Option<&str>,
    panel: &WeakEntity<ChatPanel>,
    cx: &App,
) -> AnyElement {
    let label = Label::of_mention(&chip.mention, book);
    let id = SharedString::from(format!("mention-{}-{}", chip.start, label.full));
    match chip.reference() {
        Some(reference) => frame(id, &label, true, cx)
            .role(Role::Button)
            .cursor_pointer()
            .on_click(open_on_click(panel.clone(), reference.clone()))
            .into_any_element(),
        None => frame(id, &label, false, cx).into_any_element(),
    }
}

fn push_text(lines: &mut Vec<Vec<AnyElement>>, text: &str) {
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            lines.push(Vec::new());
        }
        if let Some(current) = lines.last_mut() {
            current.extend(
                line.split_inclusive(' ')
                    .map(|word| div().child(word.to_string()).into_any_element()),
            );
        }
    }
}

fn push_element(lines: &mut [Vec<AnyElement>], element: AnyElement) {
    if let Some(current) = lines.last_mut() {
        current.push(element);
    }
}

// Pushes the text, drawing cell references as badges; true when it found any.
fn push_plain(
    lines: &mut Vec<Vec<AnyElement>>,
    text: &str,
    book: Option<&str>,
    panel: &WeakEntity<ChatPanel>,
    cx: &App,
) -> bool {
    let mut found = false;
    for part in reference_scan::split(text) {
        match part {
            reference_scan::Part::Text(text) => push_text(lines, &text),
            reference_scan::Part::Reference(reference) => {
                found = true;
                push_element(
                    lines,
                    badge(&reference, book, panel.clone(), cx).into_any_element(),
                );
            }
        }
    }
    found
}

pub(super) fn user_message(
    text: &str,
    book: Option<&str>,
    panel: &WeakEntity<ChatPanel>,
    cx: &App,
) -> Option<AnyElement> {
    let chips = mention_parse::chips(text);
    let mut styled = !chips.is_empty();
    let mut lines: Vec<Vec<AnyElement>> = vec![Vec::new()];
    let mut taken = 0;
    for chip in chips {
        styled |= push_plain(&mut lines, &text[taken..chip.start], book, panel, cx);
        push_element(&mut lines, mention_chip(&chip, book, panel, cx));
        taken = chip.end;
    }
    styled |= push_plain(&mut lines, &text[taken..], book, panel, cx);
    if !styled {
        return None;
    }
    Some(
        v_flex()
            .children(lines.into_iter().map(|line| {
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .min_h(rem_px(BADGE_HEIGHT))
                    .children(line)
            }))
            .into_any_element(),
    )
}
