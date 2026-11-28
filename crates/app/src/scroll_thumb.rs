use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::{Div, Hsla, ParentElement, ScrollHandle, Styled, div};

// The overlay `ScrollableElement::vertical_scrollbar` adds, with a thumb of the given color
// that stays on screen, so a list or grid that scrolls says so without being scrolled first.
pub(crate) fn overlay(handle: &ScrollHandle, id: &'static str, thumb: Hsla) -> Div {
    div().absolute().inset_0().child(
        Scrollbar::vertical(handle)
            .id(id)
            .mode(ScrollbarMode::Always)
            .viewport_from_layout()
            .styles(|styles| {
                styles
                    .thumb(|style| style.bg(thumb))
                    .thumb_hover(|style| style.bg(thumb))
                    .thumb_active(|style| style.bg(thumb))
            }),
    )
}
