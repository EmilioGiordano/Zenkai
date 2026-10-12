use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::text::{
    InlineElement, InlineRenderContext, MarkdownNode, MarkdownPlugin, markdown_ast,
};
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::*;
use zenkai_i18n::t;

use super::reference::Reference;
use super::reference_scan::{self, LINK_TARGET};
use super::{ChatPanel, MONO};

const BADGE_HEIGHT: f32 = 20.0;
const BADGE_ICON: f32 = 11.0;

fn frame(id: impl Into<ElementId>, reference: &Reference, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .flex_shrink_0()
        .gap_1()
        .items_center()
        .role(Role::Button)
        .aria_label(reference.text())
        .h(px(BADGE_HEIGHT))
        .pl_1()
        .pr_1p5()
        .rounded_md()
        .border_1()
        .border_color(theme.success.opacity(0.35))
        .bg(theme.success.opacity(0.14))
        .text_color(theme.success)
        .font_family(MONO)
        .text_size(px(11.5))
        .whitespace_nowrap()
        .child(Icon::new(IconName::Grid2x2).size(px(BADGE_ICON)))
        .child(reference.text())
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
    panel: WeakEntity<ChatPanel>,
    cx: &App,
) -> impl IntoElement {
    let text = reference.text();
    frame(
        SharedString::from(format!("reference-{text}")),
        reference,
        cx,
    )
    .cursor_pointer()
    .on_click(open_on_click(panel, reference.clone()))
}

pub(super) fn chip(
    index: usize,
    reference: &Reference,
    panel: WeakEntity<ChatPanel>,
    cx: &App,
) -> impl IntoElement {
    let target = reference.clone();
    let removal = panel.clone();
    frame(("chat-chip", index), reference, cx)
        .h(px(BADGE_HEIGHT + 2.0))
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
        Some(InlineElement::new(badge(reference, self.panel.clone(), cx)))
    }
}

pub(super) fn user_message(
    text: &str,
    panel: &WeakEntity<ChatPanel>,
    cx: &App,
) -> Option<AnyElement> {
    let parts = reference_scan::split(text);
    if parts
        .iter()
        .all(|part| matches!(part, reference_scan::Part::Text(_)))
    {
        return None;
    }
    let mut lines: Vec<Vec<AnyElement>> = vec![Vec::new()];
    for part in parts {
        match part {
            reference_scan::Part::Text(text) => {
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
            reference_scan::Part::Reference(reference) => {
                if let Some(current) = lines.last_mut() {
                    current.push(badge(&reference, panel.clone(), cx).into_any_element());
                }
            }
        }
    }
    Some(
        v_flex()
            .children(lines.into_iter().map(|line| {
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .min_h(px(BADGE_HEIGHT))
                    .children(line)
            }))
            .into_any_element(),
    )
}
