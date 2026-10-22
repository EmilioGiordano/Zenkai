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
use crate::pdf::protocol::Job;

pub(super) const SIDE_WIDTH: f32 = 120.0;
const THUMBNAIL_WIDTH: f32 = 84.0;
const THUMBNAIL_SLOT: f32 = 140.0;
const OUTLINE_ROW: f32 = 28.0;
const OUTLINE_INDENT: f32 = 10.0;

impl PdfView {
    fn thumbnail_pixels(&self, page: PageIdx) -> (u32, u32) {
        let size = self.size_of(page);
        let height = THUMBNAIL_WIDTH * size.height / size.width.max(1.0);
        render_size(THUMBNAIL_WIDTH, height, self.device_scale)
    }

    fn request_thumbnails(&mut self, range: Range<usize>) {
        for index in range {
            let page = PageIdx(index);
            let (pixel_width, pixel_height) = self.thumbnail_pixels(page);
            let key = BitmapKey { page, pixel_width };
            if self.thumbnails.contains(key) || !self.pending_thumbnails.insert(key) {
                continue;
            }
            self.send(Job::Render {
                page,
                pixel_width,
                pixel_height,
                thumbnail: true,
                current: self.thumbnail_token.clone(),
                generation: self.thumbnail_token.load(Ordering::Relaxed),
            });
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
        let border = cx.theme().border;
        let background = cx.theme().sidebar;
        let showing_outline = self.side == SideList::Outline;
        let has_outline = !self.outline.is_empty();
        v_flex()
            .w(px(SIDE_WIDTH))
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
                self.render_thumbnails(cx).into_any_element()
            })
    }

    fn render_thumbnails(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        uniform_list(
            "pdf-thumbnails",
            self.page_count,
            cx.processor(|this: &mut PdfView, range: Range<usize>, _, cx| {
                this.request_thumbnails(range.clone());
                let current = this.current_page();
                let (ring, muted) = {
                    let theme = cx.theme();
                    (theme.primary, theme.muted_foreground)
                };
                range
                    .map(|index| {
                        let page = PageIdx(index);
                        let (pixel_width, _) = this.thumbnail_pixels(page);
                        let image = this.thumbnails.get(BitmapKey { page, pixel_width });
                        let size = this.size_of(page);
                        let height = THUMBNAIL_WIDTH * size.height / size.width.max(1.0);
                        let selected = page == current;
                        v_flex()
                            .id(("pdf-thumbnail", index))
                            .role(Role::Button)
                            .aria_label(t!("pdf.page_aria", page = index + 1))
                            .h(px(THUMBNAIL_SLOT))
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.go_to_page(PageIdx(index), cx)
                            }))
                            .child(
                                div()
                                    .border_2()
                                    .rounded_sm()
                                    .border_color(if selected {
                                        ring
                                    } else {
                                        gpui_kit::transparent_black()
                                    })
                                    .child(
                                        div()
                                            .w(px(THUMBNAIL_WIDTH))
                                            .h(px(height.min(THUMBNAIL_SLOT - 28.0)))
                                            .bg(white())
                                            .children(image.map(|image| {
                                                img(ImageSource::Render(image)).size_full()
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format!("{}", index + 1)),
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
