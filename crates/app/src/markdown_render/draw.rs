use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::h_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

use super::blocks::{Block, Kind, Style};
use crate::interface_scale::rem_px;

const INDENT_STEP: f32 = 18.0;
const MARKER_WIDTH: f32 = 20.0;
const LINE_HEIGHT: f32 = 1.55;

// What a click on a link does with the address the document wrote.
pub type OnLink = Rc<dyn Fn(&str, &mut Window, &mut App)>;

// What drawing a line needs from the theme and the window, read once per frame.
pub struct Look {
    font: Font,
    mono: SharedString,
    foreground: Hsla,
    muted: Hsla,
    link: Hsla,
    code_background: Hsla,
    border: Hsla,
    body_size: Pixels,
    code_size: Pixels,
}

impl Look {
    pub fn new(window: &Window, cx: &App, factor: f32) -> Look {
        let theme = cx.theme();
        Look {
            font: window.text_style().font(),
            mono: theme.mono_font_family.clone(),
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            link: theme.link,
            code_background: theme.muted,
            border: theme.border,
            body_size: theme.font_size * factor,
            code_size: theme.mono_font_size * factor,
        }
    }

    fn run(&self, len: usize, style: Style, kind: Kind, color: Hsla) -> TextRun {
        let mut font = self.font.clone();
        if style.bold || matches!(kind, Kind::Heading(_)) {
            font.weight = FontWeight::BOLD;
        }
        if style.italic {
            font.style = FontStyle::Italic;
        }
        if style.code {
            font.family = self.mono.clone();
        }
        TextRun {
            len,
            font,
            color: if style.link { self.link } else { color },
            background_color: (style.code && kind != Kind::Code).then_some(self.code_background),
            underline: style.link.then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(self.link),
                wavy: false,
            }),
            strikethrough: None,
        }
    }
}

// One line of a text: plain GPUI text with its runs, no text view. A list item has its marker
// beside the text, a quote a rule at its left, a code block a monospace box, and a link, when
// there is a handler for it, is clickable. `id` tells the lines of a list apart.
pub fn draw_block(
    block: &Block,
    text: &SharedString,
    look: &Look,
    id: usize,
    on_link: Option<&OnLink>,
) -> AnyElement {
    let color = if block.quoted {
        look.muted
    } else {
        look.foreground
    };
    let runs = block
        .spans
        .iter()
        .map(|span| look.run(span.len, span.style, block.kind, color))
        .collect();
    let styled = StyledText::new(text.clone()).with_runs(runs);
    let words = match on_link {
        Some(on_link) if !block.links.is_empty() => {
            let ranges: Vec<Range<usize>> =
                block.links.iter().map(|link| link.range.clone()).collect();
            let addresses: Vec<String> = block.links.iter().map(|link| link.url.clone()).collect();
            let on_link = on_link.clone();
            InteractiveText::new(("markdown-line", id), styled)
                .on_click(ranges, move |index, window, cx| {
                    if let Some(address) = addresses.get(index) {
                        on_link(address, window, cx);
                    }
                })
                .into_any_element()
        }
        _ => styled.into_any_element(),
    };
    let (size, content) = match block.kind {
        Kind::Heading(1) => (look.body_size * 1.25, div()),
        Kind::Heading(2) => (look.body_size * 1.1, div()),
        Kind::Heading(_) | Kind::Paragraph => (look.body_size, div()),
        Kind::Code => (
            look.code_size,
            div().px_2().py_1().rounded_md().bg(look.code_background),
        ),
    };
    let content = content
        .flex_1()
        .min_w_0()
        .text_size(size)
        .line_height(relative(LINE_HEIGHT))
        .child(words);
    let mut line = h_flex().w_full().items_start().gap_2();
    if block.quoted {
        line = line.border_l_2().border_color(look.border).pl_3();
    }
    if let Some(marker) = &block.marker {
        line = line.child(
            div()
                .flex_none()
                .w(rem_px(MARKER_WIDTH))
                .text_size(look.body_size)
                .text_color(color)
                .child(marker.clone()),
        );
    }
    div()
        .w_full()
        .pl(rem_px(INDENT_STEP * f32::from(block.indent)))
        .child(line.child(content))
        .into_any_element()
}
