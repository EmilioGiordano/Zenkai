use gpui_kit::base::TextViewStyle;
use gpui_kit::component::Theme;
use gpui_kit::*;

const HEADING_BASE_PX: f32 = 14.0;

// gpui-component keeps the style it installs for every text view out of reach, so this repeats
// its derivation from the theme. Only the sizes may differ from it; `zooming_changes_only_sizes`
// guards that.
fn themed(theme: &Theme) -> TextViewStyle {
    let radius = theme.semantic_tokens().radius.md;
    let table = StyleRefinement::default().rounded(radius);
    let code_block = StyleRefinement::default().rounded(radius);
    let table_head = StyleRefinement::default()
        .bg(theme.table_head)
        .text_color(theme.table_head_foreground);

    TextViewStyle::default()
        .with_foreground(theme.foreground)
        .with_muted_foreground(theme.muted_foreground)
        .with_link(theme.link)
        .with_selection(theme.selection)
        .with_code_background(theme.muted)
        .with_border(theme.border)
        .with_code_block(code_block)
        .with_table(table)
        .with_table_head(table_head)
        .with_inline_code(HighlightStyle {
            background_color: Some(theme.accent),
            ..Default::default()
        })
        .with_dark(theme.is_dark())
}

// Headings, paragraph gaps and code blocks are sized in absolute pixels by gpui-component, so
// the text size alone does not scale them.
fn scaled(style: TextViewStyle, code_size: Pixels, factor: f32) -> TextViewStyle {
    let code_block = style.code_block().clone().text_size(code_size * factor);
    style
        .with_paragraph_gap(rems(factor))
        .with_heading(move |level| {
            let ems = match level {
                1 => 2.0,
                2 => 1.5,
                3 => 1.25,
                4 => 1.125,
                _ => 1.0,
            };
            StyleRefinement::default().text_size(px(ems * HEADING_BASE_PX * factor))
        })
        .with_code_block(code_block)
}

pub(super) fn zoomed(theme: &Theme, factor: f32) -> TextViewStyle {
    scaled(themed(theme), theme.mono_font_size, factor)
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::Theme;
    use gpui_kit::{StyleRefinement, Styled, px, rems};

    use super::{themed, zoomed};

    #[test]
    fn zooming_changes_only_sizes() {
        let theme = Theme::default();
        let plain = themed(&theme);
        let zoomed = zoomed(&theme, 2.0);
        assert!(zoomed != plain);
        let restored = zoomed
            .with_paragraph_gap(rems(1.))
            .with_heading(|_| StyleRefinement::default())
            .with_code_block(plain.code_block().clone());
        assert!(restored == plain);
    }

    #[test]
    fn headings_and_code_follow_the_factor() {
        let theme = Theme::default();
        let doubled = zoomed(&theme, 2.0);
        assert_eq!(
            doubled.heading(1),
            StyleRefinement::default().text_size(px(56.))
        );
        assert_eq!(
            doubled.code_block().text.font_size,
            Some((theme.mono_font_size * 2.0).into())
        );
    }
}
