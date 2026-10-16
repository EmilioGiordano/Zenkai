use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use zenkai_agent::settings::AgentId;

use crate::assets::{CLAUDE_MARK, CODEX_AGENT_MARK, GEMINI_MARK, GROK_MARK, OPENCODE_MARK};

pub const HEADER_MARK_SIZE: f32 = 20.0;
pub const SESSION_MARK_SIZE: f32 = 28.0;
pub const SETTINGS_MARK_SIZE: f32 = 32.0;

const CLAUDE_COLOUR: u32 = 0xd9_77_57;
const GEMINI_COLOUR: u32 = 0x8e_75_b2;
const GLYPH_RATIO: f32 = 0.625;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentMark {
    Glyph {
        path: &'static str,
        colour: Option<u32>,
    },
    Letter(char),
}

pub fn of_agent(id: &AgentId, name: &str) -> AgentMark {
    match id.as_str() {
        "claude" => AgentMark::Glyph {
            path: CLAUDE_MARK,
            colour: Some(CLAUDE_COLOUR),
        },
        "gemini" => AgentMark::Glyph {
            path: GEMINI_MARK,
            colour: Some(GEMINI_COLOUR),
        },
        "codex" => AgentMark::Glyph {
            path: CODEX_AGENT_MARK,
            colour: None,
        },
        "opencode" => AgentMark::Glyph {
            path: OPENCODE_MARK,
            colour: None,
        },
        "grok" => AgentMark::Glyph {
            path: GROK_MARK,
            colour: None,
        },
        _ => AgentMark::Letter(name.chars().next().unwrap_or('?')),
    }
}

fn glyph_tile(path: &'static str, color: Hsla, size: f32) -> AnyElement {
    div()
        .size(px(size))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .bg(color.opacity(0.16))
        .child(
            svg()
                .path(path)
                .size(px(size * GLYPH_RATIO))
                .text_color(color),
        )
        .into_any_element()
}

fn letter_tile(letter: char, size: f32, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .size(px(size))
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

pub fn mark(mark: &AgentMark, size: f32, cx: &App) -> AnyElement {
    match mark {
        AgentMark::Glyph { path, colour } => {
            let tint = (*colour).map_or_else(|| cx.theme().foreground, |value| rgb(value).into());
            glyph_tile(path, tint, size)
        }
        AgentMark::Letter(letter) => letter_tile(*letter, size, cx),
    }
}

pub fn agent_mark(agent: &AgentId, name: &str, size: f32, cx: &App) -> AnyElement {
    mark(&of_agent(agent, name), size, cx)
}

#[cfg(test)]
mod tests {
    use zenkai_agent::settings::AgentId;

    use super::{AgentMark, of_agent};
    use crate::assets::{CLAUDE_MARK, CODEX_AGENT_MARK, GEMINI_MARK, GROK_MARK, OPENCODE_MARK};

    #[test]
    fn claude_and_gemini_keep_their_brand_glyph_and_colour() {
        assert_eq!(
            of_agent(&AgentId::new("claude"), "Claude"),
            AgentMark::Glyph {
                path: CLAUDE_MARK,
                colour: Some(0xd9_77_57),
            }
        );
        assert_eq!(
            of_agent(&AgentId::new("gemini"), "Gemini CLI"),
            AgentMark::Glyph {
                path: GEMINI_MARK,
                colour: Some(0x8e_75_b2),
            }
        );
    }

    #[test]
    fn codex_opencode_and_grok_use_their_glyph_without_a_brand_colour() {
        assert_eq!(
            of_agent(&AgentId::new("codex"), "Codex"),
            AgentMark::Glyph {
                path: CODEX_AGENT_MARK,
                colour: None,
            }
        );
        assert_eq!(
            of_agent(&AgentId::new("opencode"), "opencode"),
            AgentMark::Glyph {
                path: OPENCODE_MARK,
                colour: None,
            }
        );
        assert_eq!(
            of_agent(&AgentId::new("grok"), "Grok"),
            AgentMark::Glyph {
                path: GROK_MARK,
                colour: None,
            }
        );
    }

    #[test]
    fn custom_agents_keep_the_letter_tile() {
        assert_eq!(
            of_agent(&AgentId::new("mine"), "Mine"),
            AgentMark::Letter('M')
        );
        assert_eq!(of_agent(&AgentId::new("mine"), ""), AgentMark::Letter('?'));
    }
}
