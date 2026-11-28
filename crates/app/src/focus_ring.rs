use gpui_kit::component::Theme;
use gpui_kit::{BoxShadow, px};

const GAP: f32 = 2.0;
const RING: f32 = 2.0;
const OUTLINE: f32 = 1.0;

fn layer(color: gpui_kit::Hsla, reach: f32) -> BoxShadow {
    BoxShadow::new(px(0.0), px(0.0), color).spread_radius(px(reach))
}

// Shadows paint in list order, so the outermost layer comes first and each smaller one
// covers the inside of the last: what is left is a 1px `foreground` line around a 2px
// `ring` line around a 2px gap in the background color.
pub(crate) fn focus_ring(theme: &Theme) -> Vec<BoxShadow> {
    vec![
        layer(theme.foreground, GAP + RING + OUTLINE),
        layer(theme.ring, GAP + RING),
        layer(theme.background, GAP),
    ]
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::Theme;
    use gpui_kit::px;

    use super::focus_ring;

    #[test]
    fn the_ring_is_a_gap_then_ring_then_outline_painted_outermost_first() {
        let theme = Theme::default();
        let layers = focus_ring(&theme);
        let spreads: Vec<_> = layers.iter().map(|layer| layer.spread_radius).collect();
        assert_eq!(spreads, [px(5.0), px(4.0), px(2.0)]);
        assert_eq!(layers[0].color, theme.foreground);
        assert_eq!(layers[1].color, theme.ring);
        assert_eq!(layers[2].color, theme.background);
        assert!(layers.iter().all(|layer| !layer.inset));
    }
}
