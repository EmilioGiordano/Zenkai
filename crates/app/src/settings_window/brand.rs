use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

use crate::assets::{CLAUDE_MARK, GEMINI_MARK};

const CLAUDE_COLOR: u32 = 0xd9_77_57;
const GEMINI_COLOR: u32 = 0x8e_75_b2;
const TILE: f32 = 32.0;
const GLYPH: f32 = 20.0;

pub enum Mark {
    Claude,
    Gemini,
    Letter(char),
}

impl Mark {
    pub fn of_preset(id: &str, name: &str) -> Mark {
        match id {
            "claude" => Mark::Claude,
            "gemini" => Mark::Gemini,
            _ => Mark::Letter(name.chars().next().unwrap_or('?')),
        }
    }
}

fn tile(color: Hsla) -> Div {
    div()
        .size(px(TILE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .bg(color.opacity(0.16))
}

fn glyph(path: &'static str, color: Hsla) -> Svg {
    svg().path(path).size(px(GLYPH)).text_color(color)
}

pub fn mark(mark: &Mark, cx: &App) -> AnyElement {
    match mark {
        Mark::Claude => {
            let color: Hsla = rgb(CLAUDE_COLOR).into();
            tile(color)
                .child(glyph(CLAUDE_MARK, color))
                .into_any_element()
        }
        Mark::Gemini => {
            let color: Hsla = rgb(GEMINI_COLOR).into();
            tile(color)
                .child(glyph(GEMINI_MARK, color))
                .into_any_element()
        }
        Mark::Letter(letter) => {
            let theme = cx.theme();
            div()
                .size(px(TILE))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded_lg()
                .bg(theme.secondary)
                .text_color(theme.muted_foreground)
                .font_weight(FontWeight::SEMIBOLD)
                .child(letter.to_uppercase().to_string())
                .into_any_element()
        }
    }
}
