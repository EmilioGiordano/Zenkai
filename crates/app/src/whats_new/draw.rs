use gpui_kit::base::h_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

use super::blocks::{Block, Kind, Style};
use crate::interface_scale::rem_px;

const INDENT_STEP: f32 = 18.0;
const MARKER_WIDTH: f32 = 20.0;
const LINE_HEIGHT: f32 = 1.55;

// What drawing a line needs from the theme and the window, read once per frame.
pub(super) struct Look {
    font: Font,
    mono: SharedString,
    foreground: Hsla,
    muted: Hsla,
    code_background: Hsla,
    border: Hsla,
    body_size: Pixels,
    code_size: Pixels,
}

impl Look {
    pub(super) fn new(window: &Window, cx: &App, factor: f32) -> Look {
        let theme = cx.theme();
        Look {
            font: window.text_style().font(),
            mono: theme.mono_font_family.clone(),
            foreground: theme.foreground,
            muted: theme.muted_foreground,
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
            color,
            background_color: (style.code && kind != Kind::Code).then_some(self.code_background),
            underline: None,
            strikethrough: None,
        }
    }
}

// One line of a note: plain GPUI text with its runs, no text view and no links. A list item
// has its marker beside the text, a quote a rule at its left, a code block a monospace box.
pub(super) fn draw_block(block: &Block, text: &SharedString, look: &Look) -> AnyElement {
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
        .child(styled);
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
