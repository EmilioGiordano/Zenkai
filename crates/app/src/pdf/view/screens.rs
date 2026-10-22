use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::Sizable;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::paint::paint;
use super::{CONTEXT, PdfView, Phase};
use crate::actions::{
    ClosePdfFind, Copy, Find, GoTo, PdfActualSize, PdfFirstPage, PdfFitPage, PdfFitWidth,
    PdfLastPage, PdfNextPage, PdfPreviousPage, SelectPdfPage, TogglePdfOutline, ZoomIn, ZoomOut,
    ZoomReset,
};
use crate::interface_scale::rem_px;
use crate::pdf::layout::ZoomMode;
use crate::pdf::protocol::{BrokenFile, OpenError};

const MESSAGE_WIDTH: f32 = 420.0;

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

pub(super) fn failure_text(error: &OpenError) -> String {
    match error {
        OpenError::LibraryMissing => t!("pdf.unavailable.missing").to_string(),
        OpenError::LibraryFailed(reason) => t!("pdf.unavailable.failed", reason = reason),
        OpenError::PasswordRequired | OpenError::WrongPassword => {
            t!("pdf.password.title").to_string()
        }
        OpenError::Broken(BrokenFile::NotAPdf) => t!("pdf.broken.not_a_pdf").to_string(),
        OpenError::Broken(BrokenFile::Unreadable(reason)) => {
            t!("pdf.broken.unreadable", reason = reason)
        }
        OpenError::Broken(BrokenFile::NoPages) => t!("pdf.broken.no_pages").to_string(),
        OpenError::Broken(BrokenFile::Other) => t!("pdf.broken.other").to_string(),
    }
}

impl PdfView {
    fn render_message(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (muted, warning) = {
            let theme = cx.theme();
            (theme.muted_foreground, theme.warning)
        };
        let content = match &self.phase {
            Phase::Opening | Phase::Ready => v_flex().child(
                div()
                    .text_color(muted)
                    .child(t!("pdf.opening", name = file_name(&self.path))),
            ),
            Phase::Password { wrong } => v_flex()
                .gap_3()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("pdf.password.title")),
                )
                .when(*wrong, |column| {
                    column.child(
                        h_flex()
                            .gap_2()
                            .text_color(warning)
                            .child(Icon::new(IconName::TriangleAlert).size_4())
                            .child(t!("pdf.password.wrong")),
                    )
                })
                .child(Input::new(&self.password_input).mask_toggle())
                .child(
                    Button::new("pdf-password-open")
                        .small()
                        .label(t!("pdf.password.open"))
                        .on_click(cx.listener(|this, _, _, cx| this.submit_password(cx))),
                ),
            Phase::Failed(error) => v_flex().child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size_4()
                            .flex_shrink_0()
                            .text_color(warning),
                    )
                    .child(failure_text(error)),
            ),
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .child(div().w(rem_px(MESSAGE_WIDTH)).child(content))
    }

    fn text_notice(&self, cx: &App) -> Option<AnyElement> {
        let content = self.contents.get(&self.current_page())?;
        let message = if content.truncated {
            t!("pdf.text_truncated")
        } else if content.chars.iter().all(|found| found.ch.is_whitespace()) {
            t!("pdf.no_text_layer")
        } else {
            return None;
        };
        let theme = cx.theme();
        Some(
            h_flex()
                .absolute()
                .top_2()
                .left_0()
                .right_0()
                .justify_center()
                .child(
                    h_flex()
                        .gap_2()
                        .px_3()
                        .py_1p5()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .text_sm()
                        .child(Icon::new(IconName::Info).size_4())
                        .child(message),
                )
                .into_any_element(),
        )
    }

    fn render_reader(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.frame(cx);
        self.request_visible();
        let weak = cx.entity().downgrade();
        let notice = self.text_notice(cx);
        let toolbar = self.render_toolbar(window, cx).into_any_element();
        let side = self.render_side(window, cx).into_any_element();
        let (canvas_background, pointing) = (cx.theme().muted, self.hover.is_some());
        let surface = canvas(
            move |bounds, _, cx| {
                let weak = weak.clone();
                cx.defer(move |cx| {
                    if let Err(error) = weak.update(cx, |view, cx| view.set_bounds(bounds, cx)) {
                        tracing::debug!(%error, "pdf view dropped before layout");
                    }
                });
                bounds
            },
            move |bounds, _, window, _| paint(&frame, bounds, window),
        )
        .size_full();
        v_flex().flex_1().min_h_0().child(toolbar).child(
            h_flex().flex_1().min_h_0().child(side).child(
                div()
                    .id("pdf-pages")
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .bg(canvas_background)
                    .when(pointing, |canvas| canvas.cursor_pointer())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event, window, cx| {
                            this.on_mouse_down(event, window, cx)
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event, _, cx| this.on_mouse_move(event, cx)))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.on_mouse_up(cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.on_mouse_up(cx)),
                    )
                    .on_scroll_wheel(cx.listener(|this, event, _, cx| this.on_wheel(event, cx)))
                    .child(surface)
                    .children(notice),
            ),
        )
    }
}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        if (scale - self.device_scale).abs() > f32::EPSILON {
            self.device_scale = scale;
            self.release_bitmaps(window);
        }
        let (background, ring) = {
            let theme = cx.theme();
            let focused = self.focus.contains_focused(window, cx);
            (
                theme.background,
                if focused {
                    theme.ring
                } else {
                    theme.background
                },
            )
        };
        let body = if self.is_ready() {
            self.render_reader(window, cx).into_any_element()
        } else {
            self.render_message(cx).into_any_element()
        };
        v_flex()
            .id("pdf-view")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.on_key_down(event, cx)))
            .on_action(cx.listener(|this, _: &PdfPreviousPage, _, cx| this.step_page(false, cx)))
            .on_action(cx.listener(|this, _: &PdfNextPage, _, cx| this.step_page(true, cx)))
            .on_action(cx.listener(|this, _: &PdfFirstPage, _, cx| {
                this.go_to_page(crate::pdf::layout::PageIdx(0), cx)
            }))
            .on_action(cx.listener(|this, _: &PdfLastPage, _, cx| this.last_page(cx)))
            .on_action(cx.listener(|this, _: &SelectPdfPage, _, cx| this.select_page(cx)))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_find(window, cx)))
            .on_action(
                cx.listener(|this, _: &ClosePdfFind, window, cx| this.close_find(window, cx)),
            )
            .on_action(cx.listener(|this, _: &GoTo, window, cx| this.focus_page_input(window, cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.step_zoom(true, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.step_zoom(false, cx)))
            .on_action(
                cx.listener(|this, _: &ZoomReset, _, cx| this.set_zoom(ZoomMode::Percent(100), cx)),
            )
            .on_action(cx.listener(|this, _: &PdfActualSize, _, cx| {
                this.set_zoom(ZoomMode::Percent(100), cx)
            }))
            .on_action(
                cx.listener(|this, _: &PdfFitPage, _, cx| this.set_zoom(ZoomMode::FitPage, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PdfFitWidth, _, cx| this.set_zoom(ZoomMode::FitWidth, cx)),
            )
            .on_action(
                cx.listener(|this, _: &TogglePdfOutline, window, cx| {
                    this.toggle_outline(window, cx)
                }),
            )
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(background)
            .border_2()
            .border_color(ring)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::failure_text;
    use crate::pdf::protocol::{BrokenFile, OpenError};

    #[test]
    fn a_missing_library_says_where_it_should_be() {
        assert_eq!(
            failure_text(&OpenError::LibraryMissing),
            "PDF viewer unavailable: pdfium.dll is missing next to zenkai.exe"
        );
    }

    #[test]
    fn a_broken_file_says_why_it_cannot_open() {
        let texts: Vec<String> = [
            BrokenFile::NotAPdf,
            BrokenFile::Unreadable("access denied".to_string()),
            BrokenFile::NoPages,
            BrokenFile::Other,
        ]
        .into_iter()
        .map(|reason| failure_text(&OpenError::Broken(reason)))
        .collect();
        assert!(texts[1].contains("access denied"));
        for (index, text) in texts.iter().enumerate() {
            assert!(!text.is_empty());
            assert!(texts.iter().skip(index + 1).all(|other| other != text));
        }
    }
}
