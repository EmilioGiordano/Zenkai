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
use crate::pdf::cache::BitmapKey;
use crate::pdf::layout::PageIdx;
use crate::pdf::protocol::Job;

pub(super) const SIDE_WIDTH: f32 = 120.0;
const THUMBNAIL_WIDTH: f32 = 84.0;
const THUMBNAIL_SLOT: f32 = 140.0;
const OUTLINE_ROW: f32 = 28.0;
const OUTLINE_INDENT: f32 = 10.0;

impl PdfView {
    fn thumbnail_pixels(&self, page: PageIdx) -> (u32, u32) {
        let size = self.size_of(page);
        let width = (THUMBNAIL_WIDTH * self.device_scale).round().max(1.0);
        let height = (width * size.height / size.width.max(1.0)).round().max(1.0);
        (width as u32, height as u32)
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

    pub fn toggle_outline(&mut self, cx: &mut Context<Self>) {
        if self.outline.is_empty() {
            return;
        }
        self.side = match self.side {
            SideList::Thumbnails => SideList::Outline,
            SideList::Outline => SideList::Thumbnails,
        };
        cx.notify();
    }

    pub(super) fn render_side(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_outline(cx))),
                    ),
                )
            })
            .child(if showing_outline && has_outline {
                self.render_outline(cx).into_any_element()
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

    fn render_outline(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        uniform_list(
            "pdf-outline",
            self.outline.len(),
            cx.processor(|this: &mut PdfView, range: Range<usize>, _, cx| {
                let hover = cx.theme().list_hover;
                range
                    .filter_map(|index| {
                        let item = this.outline.get(index)?;
                        let page = item.page;
                        Some(
                            h_flex()
                                .id(("pdf-outline-row", index))
                                .h(px(OUTLINE_ROW))
                                .pl(px(6.0 + f32::from(item.depth) * OUTLINE_INDENT))
                                .pr_1()
                                .items_center()
                                .cursor_pointer()
                                .hover(move |row| row.bg(hover))
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
        .flex_1()
        .min_h_0()
    }
}
