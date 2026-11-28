use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_engine::Engine;
use zenkai_i18n::t;
use zenkai_types::Rgb;

use super::Workspace;
use crate::actions::{
    ApplyTabColor, CloseTabColor, FocusNextControl, FocusPreviousControl, TabColorDown,
    TabColorEnd, TabColorHome, TabColorLeft, TabColorRight, TabColorUp,
};
use crate::document::Document;
use crate::focus_ring::focus_ring;
use crate::interface_scale::rem_px;
use crate::tab_color_card::{
    Caption, Focus, LABEL_GAP, LABEL_HEIGHT, Move, REGION_PADDING, SECTION_GAP, SWATCH_GAP,
    SWATCH_SIZE, TINT_GAP, TabColorCard, scrolled_to, top_of,
};
use crate::tab_colors::{COLUMNS, SWATCHES, Swatch, THEME_ROWS, glyph_color, hex};

const CARD_WIDTH: f32 = 300.0;
const CARD_TOP: f32 = 96.0;
const CARD_BOTTOM_MARGIN: f32 = 16.0;
const NONE_ROW_HEIGHT: f32 = 28.0;
const CHECK: &str = "✓";

fn paint(color: Rgb) -> Hsla {
    rgb(color.0).into()
}

fn spacer(height: f32) -> Div {
    div().h(rem_px(height)).flex_none()
}

impl Workspace {
    pub(super) fn open_tab_color(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.active() else {
            return;
        };
        let (sheet, generation) = (document.sheet, document.generation());
        let color = document
            .sheets
            .iter()
            .find(|info| info.id == sheet)
            .and_then(|info| info.color);
        let swatch_focus = (0..SWATCHES).map(|_| cx.focus_handle()).collect();
        let card = TabColorCard::new(
            sheet,
            generation,
            color,
            cx.focus_handle(),
            swatch_focus,
            cx.focus_handle(),
        );
        self.unhide = None;
        if let Some(handle) = card.grid_handle() {
            window.focus(handle, cx);
        }
        self.tab_color = Some(card);
        // The card is laid out two frames from now; only then can it know how far to scroll.
        cx.on_next_frame(window, |_, window, cx| {
            cx.on_next_frame(window, |this, window, cx| {
                this.reveal_focused_swatch(window, cx)
            });
        });
        cx.notify();
    }

    pub(super) fn close_tab_color(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_color.take().is_none() {
            return;
        }
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn reveal_focused_swatch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(card) = &self.tab_color
            && let Focus::Swatch(swatch) = card.focus
        {
            reveal(card, swatch, window);
            cx.notify();
        }
    }

    // The card applies to the sheet it opened on; anything else makes it a no-op.
    pub(super) fn apply_tab_color(
        &mut self,
        color: Option<Rgb>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = &self.tab_color else {
            return;
        };
        let (sheet, generation) = (card.sheet, card.generation);
        let unchanged = self.documents.active().is_some_and(|document: &Document| {
            document.generation() == generation && document.sheet == sheet
        });
        self.close_tab_color(window, cx);
        if unchanged {
            self.edit(window, cx, move |wb| wb.set_sheet_color(sheet, color));
        }
    }

    pub(super) fn apply_focused_tab_color(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = &self.tab_color else {
            return;
        };
        let color = match card.focus {
            Focus::Swatch(swatch) => Some(swatch.rgb()),
            Focus::NoColor => None,
        };
        self.apply_tab_color(color, window, cx);
    }

    pub(super) fn move_tab_color(
        &mut self,
        key: Move,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.tab_color.as_mut() else {
            return;
        };
        let (focus, back) = crate::tab_color_card::moved(card.focus, card.back, key);
        card.focus = focus;
        card.back = back;
        card.moved = true;
        let handle = match focus {
            Focus::Swatch(swatch) => card.handle_of(swatch).cloned(),
            Focus::NoColor => Some(card.none_focus.clone()),
        };
        if let Focus::Swatch(swatch) = focus {
            reveal(card, swatch, window);
        }
        if let Some(handle) = handle {
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    // Tab has two stops, the grid and No color, and both directions switch between them.
    pub(super) fn cycle_tab_color_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self.tab_color.as_mut() else {
            return;
        };
        let handle = match card.focus {
            Focus::NoColor => {
                let swatch = card.back;
                card.focus = Focus::Swatch(swatch);
                card.handle_of(swatch).cloned()
            }
            Focus::Swatch(swatch) => {
                card.back = swatch;
                card.focus = Focus::NoColor;
                Some(card.none_focus.clone())
            }
        };
        if let Some(handle) = handle {
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    // The keys of the card, which its own context binds to these actions.
    pub(super) fn with_tab_color_actions<E: InteractiveElement>(
        root: E,
        cx: &mut Context<Self>,
    ) -> E {
        root.on_action(cx.listener(|this, _: &TabColorLeft, window, cx| {
            this.move_tab_color(Move::Left, window, cx)
        }))
        .on_action(cx.listener(|this, _: &TabColorRight, window, cx| {
            this.move_tab_color(Move::Right, window, cx)
        }))
        .on_action(
            cx.listener(|this, _: &TabColorUp, window, cx| {
                this.move_tab_color(Move::Up, window, cx)
            }),
        )
        .on_action(cx.listener(|this, _: &TabColorDown, window, cx| {
            this.move_tab_color(Move::Down, window, cx)
        }))
        .on_action(cx.listener(|this, _: &TabColorHome, window, cx| {
            this.move_tab_color(Move::Home, window, cx)
        }))
        .on_action(cx.listener(|this, _: &TabColorEnd, window, cx| {
            this.move_tab_color(Move::End, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ApplyTabColor, window, cx| {
            this.apply_focused_tab_color(window, cx)
        }))
        .on_action(
            cx.listener(|this, _: &CloseTabColor, window, cx| this.close_tab_color(window, cx)),
        )
    }

    fn hover_swatch(&mut self, swatch: Swatch, hovered: bool, cx: &mut Context<Self>) {
        let Some(card) = self.tab_color.as_mut() else {
            return;
        };
        if hovered {
            card.hovered = Some(swatch);
        } else if card.hovered == Some(swatch) {
            card.hovered = None;
        }
        cx.notify();
    }

    fn render_swatch(
        &self,
        card: &TabColorCard,
        swatch: Swatch,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let color = swatch.rgb();
        let marked = card.marked() == Some(swatch);
        let focused = card
            .handle_of(swatch)
            .is_some_and(|handle| handle.is_focused(window));
        let mut element =
            div()
                .id(("tab-swatch", swatch.index()))
                .role(Role::Button)
                .aria_label(swatch.name())
                .w(rem_px(SWATCH_SIZE))
                .h(rem_px(SWATCH_SIZE))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .bg(paint(color))
                .border_1()
                .border_color(theme.muted_foreground)
                .cursor_pointer()
                .when(marked, |el| el.border_2().border_color(theme.foreground))
                .when(focused, |el| el.shadow(focus_ring(theme)))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    this.hover_swatch(swatch, *hovered, cx)
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.apply_tab_color(Some(color), window, cx)
                }))
                .when(marked, |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(paint(glyph_color(color)))
                            .child(CHECK),
                    )
                });
        if let Some(handle) = card.handle_of(swatch) {
            element = element.track_focus(handle);
        }
        element
    }

    fn render_swatch_row(
        &self,
        card: &TabColorCard,
        swatches: impl Iterator<Item = Swatch>,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .gap(rem_px(SWATCH_GAP))
            .h(rem_px(SWATCH_SIZE))
            .flex_none()
            .children(
                swatches
                    .map(|swatch| self.render_swatch(card, swatch, window, cx))
                    .collect::<Vec<_>>(),
            )
    }

    fn render_swatch_region(
        &self,
        card: &TabColorCard,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let label = |text: &'static str| {
            div()
                .h(rem_px(LABEL_HEIGHT))
                .flex_none()
                .flex()
                .items_center()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(text)
        };
        let mut region = v_flex()
            .id("tab-color-region")
            .role(Role::Grid)
            .aria_label(t!("tab_color.theme"))
            .track_scroll(&card.scroll)
            .overflow_y_scroll()
            .min_h_0()
            .p(rem_px(REGION_PADDING))
            .child(label(t!("tab_color.theme")))
            .child(spacer(LABEL_GAP));
        for row in 0..THEME_ROWS {
            if row == 1 {
                region = region.child(spacer(SWATCH_GAP + TINT_GAP));
            } else if row > 1 {
                region = region.child(spacer(SWATCH_GAP));
            }
            let cells = (0..COLUMNS).map(move |col| Swatch::Theme { row, col });
            region = region.child(self.render_swatch_row(card, cells, window, cx));
        }
        region
            .child(spacer(SECTION_GAP))
            .child(label(t!("tab_color.standard")))
            .child(spacer(LABEL_GAP))
            .child(self.render_swatch_row(
                card,
                (0..COLUMNS).map(|col| Swatch::Standard { col }),
                window,
                cx,
            ))
    }

    pub(super) fn render_tab_color(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement> {
        let card = self.tab_color.as_ref()?;
        let region = self.render_swatch_region(card, window, cx);
        let theme = cx.theme();
        let caption = match card.caption() {
            Caption::Swatch(swatch) => swatch.name(),
            Caption::Outside(color) => t!("tab_color.current", hex = hex(color)),
            Caption::Blank => String::new(),
        };
        let none_focused = card.none_focus.is_focused(window);
        let cap = window.viewport_size().height
            - rem_px(CARD_TOP + CARD_BOTTOM_MARGIN).to_pixels(window.rem_size());
        let none_row = h_flex()
            .id("tab-color-none")
            .track_focus(&card.none_focus)
            .role(Role::Button)
            .aria_label(t!("tab_color.none"))
            .h(rem_px(NONE_ROW_HEIGHT))
            .flex_none()
            .px_2()
            .gap_2()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme.muted_foreground)
            .cursor_pointer()
            .when(none_focused, |row| row.shadow(focus_ring(theme)))
            .on_click(cx.listener(|this, _, window, cx| this.apply_tab_color(None, window, cx)))
            .child(
                div()
                    .w(rem_px(SWATCH_SIZE * 0.8))
                    .child(if card.no_color_marked() { CHECK } else { "" }),
            )
            .child(div().text_sm().child(t!("tab_color.none")));
        let panel = v_flex()
            .id("tab-color-card")
            .key_context("TabColorCard")
            .track_focus(&card.card_focus)
            .role(Role::Dialog)
            .aria_label(t!("tab_color.title"))
            .occlude()
            .on_action(cx.listener(|this, _: &FocusNextControl, window, cx| {
                this.cycle_tab_color_focus(window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusPreviousControl, window, cx| {
                this.cycle_tab_color_focus(window, cx)
            }))
            .w(rem_px(CARD_WIDTH))
            .max_h(cap)
            .p_4()
            .gap_3()
            .bg(theme.background)
            .text_color(theme.foreground)
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("tab_color.title")),
            )
            .child(region)
            .child(
                div()
                    .h(rem_px(LABEL_HEIGHT))
                    .flex_none()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(caption),
            )
            .child(none_row)
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("tab_color.hint")),
            );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.dispatch_action(Box::new(CloseTabColor), cx)
                })
                .on_mouse_down(MouseButton::Right, |_, window, cx| {
                    window.dispatch_action(Box::new(CloseTabColor), cx)
                })
                .child(
                    div()
                        .absolute()
                        .top(rem_px(CARD_TOP))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(panel),
                ),
        )
    }
}

// Brings the swatch fully into view, ring included.
fn reveal(card: &TabColorCard, swatch: Swatch, window: &Window) {
    let unit = f32::from(rem_px(1.0).to_pixels(window.rem_size()));
    let view = f32::from(card.scroll.bounds().size.height) / unit;
    if view <= 0.0 {
        return;
    }
    let offset = -f32::from(card.scroll.offset().y) / unit;
    let top = top_of(swatch);
    let next = scrolled_to(offset, view, top, top + SWATCH_SIZE);
    if (next - offset).abs() > f32::EPSILON {
        card.scroll.set_offset(point(px(0.0), px(-next * unit)));
    }
}
