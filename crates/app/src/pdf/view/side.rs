use std::ops::Range;
use std::sync::atomic::Ordering;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::Button;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::{PdfView, SideList};
use crate::actions::{PdfOutlineClose, PdfOutlineNext, PdfOutlineOpen, PdfOutlinePrevious};
use crate::pdf::cache::BitmapKey;
use crate::pdf::layout::{PageIdx, render_size};
use crate::pdf::thumbnail_box;
use zenkai_pdf_host::wire::Job;

const THUMBNAIL_RADIUS: f32 = 6.0;
const RING: f32 = 3.0;
const PILL_OFFSET: f32 = 6.0;
const OUTLINE_ROW: f32 = 28.0;
const OUTLINE_INDENT: f32 = 10.0;

const THUMBNAIL_KEPT_MARGIN: usize = 4;

pub(super) fn thumbnail_kept(visible: &Range<usize>, page: PageIdx) -> bool {
    page.0 + THUMBNAIL_KEPT_MARGIN >= visible.start && page.0 < visible.end + THUMBNAIL_KEPT_MARGIN
}

impl PdfView {
    fn thumbnail_pixels(&self, page: PageIdx) -> (u32, u32) {
        let side = self.settled_side.unwrap_or_else(|| self.fitted_side());
        let fit = thumbnail_box::fit(side, self.size_of(page));
        render_size(fit.width, fit.height, self.device_scale)
    }

    fn request_thumbnails(&mut self, range: Range<usize>) {
        self.thumbnail_visible = range.clone();
        for index in range {
            let page = PageIdx(index);
            let (pixel_width, pixel_height) = self.thumbnail_pixels(page);
            let key = BitmapKey { page, pixel_width };
            if self.thumbnails.contains(key) || !self.pending_thumbnails.insert(key) {
                continue;
            }
            self.send_with(
                Job::Render {
                    page,
                    pixel_width,
                    pixel_height,
                    thumbnail: true,
                    generation: self.thumbnail_token.load(Ordering::Relaxed),
                },
                Some(self.thumbnail_token.clone()),
            );
        }
    }

    pub fn toggle_outline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.outline.is_empty() {
            return;
        }
        match self.side {
            SideList::Thumbnails => {
                self.side = SideList::Outline;
                self.outline_cursor = self
                    .outline
                    .iter()
                    .rposition(|item| item.page <= self.current_page())
                    .unwrap_or(0);
                window.focus(&self.outline_focus, cx);
            }
            SideList::Outline => {
                self.side = SideList::Thumbnails;
                window.focus(&self.focus, cx);
            }
        }
        cx.notify();
    }

    pub fn move_outline(&mut self, forward: bool, cx: &mut Context<Self>) {
        let last = self.outline.len().saturating_sub(1);
        self.outline_cursor = if forward {
            (self.outline_cursor + 1).min(last)
        } else {
            self.outline_cursor.saturating_sub(1)
        };
        self.outline_scroll
            .scroll_to_item(self.outline_cursor, ScrollStrategy::Nearest);
        cx.notify();
    }

    pub fn open_outline_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.outline.get(self.outline_cursor) {
            let page = item.page;
            self.go_to_page(page, cx);
            window.focus(&self.focus, cx);
        }
    }

    pub fn leave_outline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn render_side(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let side_width = self.fitted_side();
        self.drawn_side = side_width;
        if self.settled_side.is_none() && self.bounds.size.width > px(0.0) {
            self.settled_side = Some(side_width);
        }
        let border = cx.theme().border;
        let background = cx.theme().sidebar;
        let showing_outline = self.side == SideList::Outline;
        let has_outline = !self.outline.is_empty();
        v_flex()
            .relative()
            .w(px(side_width))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(border)
            .bg(background)
            .when(has_outline, |side| {
                side.child(
                    div().p_1().child(
                        Button::new("pdf-side-toggle")
                            .small()
                            .w_full()
                            .label(if showing_outline {
                                t!("pdf.side.show_pages")
                            } else {
                                t!("pdf.side.show_outline")
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_outline(window, cx)),
                            ),
                    ),
                )
            })
            .child(if showing_outline && has_outline {
                self.render_outline(window, cx).into_any_element()
            } else {
                self.render_thumbnails(side_width, cx).into_any_element()
            })
            .child(self.render_side_edge(window, cx))
    }

    fn render_thumbnails(&mut self, side_width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let row = thumbnail_box::row_height(side_width, self.thumbnail_ratio);
        uniform_list(
            "pdf-thumbnails",
            self.page_count,
            cx.processor(move |this: &mut PdfView, range: Range<usize>, _, cx| {
                this.request_thumbnails(range.clone());
                let current = this.current_page();
                let (primary, primary_text, border, hover_ring) = {
                    let theme = cx.theme();
                    (
                        theme.primary,
                        theme.primary_foreground,
                        theme.border,
                        theme.muted_foreground,
                    )
                };
                range
                    .map(|index| {
                        let page = PageIdx(index);
                        let (pixel_width, _) = this.thumbnail_pixels(page);
                        let image = this.thumbnails.get(BitmapKey { page, pixel_width });
                        let fit = thumbnail_box::fit(side_width, this.size_of(page));
                        let selected = page == current;
                        let group = SharedString::from(format!("pdf-thumbnail-{index}"));
                        h_flex()
                            .id(("pdf-thumbnail", index))
                            .role(Role::Button)
                            .aria_label(t!("pdf.page_aria", page = index + 1))
                            .group(group.clone())
                            .h(px(row))
                            .w_full()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.go_to_page(PageIdx(index), cx)
                            }))
                            .child(
                                div()
                                    .relative()
                                    .w(px(fit.width))
                                    .h(px(fit.height))
                                    .flex_none()
                                    .child(
                                        div()
                                            .size_full()
                                            .rounded(px(THUMBNAIL_RADIUS))
                                            .overflow_hidden()
                                            .border_1()
                                            .border_color(border)
                                            .shadow_sm()
                                            .bg(white())
                                            .children(image.map(|image| {
                                                img(ImageSource::Render(image)).size_full()
                                            })),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .top(px(-RING))
                                            .left(px(-RING))
                                            .w(px(fit.width + 2.0 * RING))
                                            .h(px(fit.height + 2.0 * RING))
                                            .rounded(px(THUMBNAIL_RADIUS + RING))
                                            .border(px(RING))
                                            .border_color(if selected {
                                                primary
                                            } else {
                                                gpui_kit::transparent_black()
                                            })
                                            .when(!selected, |ring| {
                                                ring.group_hover(group, move |ring| {
                                                    ring.border_color(hover_ring)
                                                })
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .absolute()
                                            .bottom(px(PILL_OFFSET))
                                            .left_0()
                                            .right_0()
                                            .justify_center()
                                            .child(
                                                div()
                                                    .px(px(8.0))
                                                    .rounded_full()
                                                    .text_xs()
                                                    .bg(if selected {
                                                        primary
                                                    } else {
                                                        hsla(0.0, 0.0, 0.0, 0.65)
                                                    })
                                                    .text_color(if selected {
                                                        primary_text
                                                    } else {
                                                        white()
                                                    })
                                                    .child(format!("{}", index + 1)),
                                            ),
                                    ),
                            )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.thumbnail_scroll)
        .flex_1()
        .min_h_0()
    }

    fn render_outline(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.outline_focus.clone();
        let ring = cx.theme().ring;
        let list = uniform_list(
            "pdf-outline",
            self.outline.len(),
            cx.processor(|this: &mut PdfView, range: Range<usize>, window, cx| {
                let (hover, selected_background, ring) = {
                    let theme = cx.theme();
                    (theme.list_hover, theme.list_active, theme.ring)
                };
                let focused = this.outline_focus.contains_focused(window, cx);
                range
                    .filter_map(|index| {
                        let item = this.outline.get(index)?;
                        let page = item.page;
                        let at_cursor = index == this.outline_cursor;
                        Some(
                            h_flex()
                                .id(("pdf-outline-row", index))
                                .h(px(OUTLINE_ROW))
                                .pl(px(6.0 + f32::from(item.depth) * OUTLINE_INDENT))
                                .pr_1()
                                .items_center()
                                .cursor_pointer()
                                .hover(move |row| row.bg(hover))
                                .when(at_cursor, |row| {
                                    row.bg(selected_background).border_l_2().border_color(
                                        if focused { ring } else { selected_background },
                                    )
                                })
                                .text_sm()
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.go_to_page(page, cx)),
                                )
                                .child(div().truncate().child(item.title.clone())),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.outline_scroll)
        .flex_1()
        .min_h_0();
        v_flex()
            .id("pdf-outline-list")
            .key_context("PdfOutline")
            .track_focus(&focus)
            .on_action(cx.listener(|this, _: &PdfOutlineNext, _, cx| this.move_outline(true, cx)))
            .on_action(
                cx.listener(|this, _: &PdfOutlinePrevious, _, cx| this.move_outline(false, cx)),
            )
            .on_action(cx.listener(|this, _: &PdfOutlineOpen, window, cx| {
                this.open_outline_item(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &PdfOutlineClose, window, cx| this.leave_outline(window, cx)),
            )
            .when(focus.contains_focused(window, cx), |list| {
                list.border_2().border_color(ring)
            })
            .flex_1()
            .min_h_0()
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::{PageIdx, thumbnail_kept};

    #[test]
    fn only_thumbnails_near_the_visible_rows_are_kept() {
        let visible = 20..30;
        assert!(thumbnail_kept(&visible, PageIdx(20)));
        assert!(thumbnail_kept(&visible, PageIdx(16)));
        assert!(thumbnail_kept(&visible, PageIdx(33)));
        assert!(!thumbnail_kept(&visible, PageIdx(15)));
        assert!(!thumbnail_kept(&visible, PageIdx(34)));
        assert!(!thumbnail_kept(&(0..0), PageIdx(10)));
    }
}
