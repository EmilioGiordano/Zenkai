use gpui_kit::assets::IconName;
use gpui_kit::base::h_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants, Toggle, ToggleGroup};
use gpui_kit::component::input::Input;
use gpui_kit::*;
use zenkai_i18n::t;

use super::PdfView;
use crate::interface_scale::rem_px;
use crate::pdf::layout::ZoomMode;

const TOOLBAR_HEIGHT: f32 = 40.0;
const PAGE_BOX_WIDTH: f32 = 52.0;
const FIND_BOX_WIDTH: f32 = 220.0;
const ZOOMS: [ZoomMode; 3] = [
    ZoomMode::FitPage,
    ZoomMode::FitWidth,
    ZoomMode::Percent(100),
];

fn zoom_label(zoom: ZoomMode) -> &'static str {
    match zoom {
        ZoomMode::FitPage => t!("pdf.zoom.fit_page"),
        ZoomMode::FitWidth => t!("pdf.zoom.fit_width"),
        ZoomMode::Percent(_) => t!("pdf.zoom.actual"),
    }
}

fn zoom_id(zoom: ZoomMode) -> &'static str {
    match zoom {
        ZoomMode::FitPage => "pdf-zoom-fit-page",
        ZoomMode::FitWidth => "pdf-zoom-fit-width",
        ZoomMode::Percent(_) => "pdf-zoom-actual",
    }
}

impl PdfView {
    pub(super) fn render_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.keep_page_box_current(window, cx);
        let (border, muted) = {
            let theme = cx.theme();
            (theme.border, theme.muted_foreground)
        };
        let status = self.find_status(cx);
        h_flex()
            .h(rem_px(TOOLBAR_HEIGHT))
            .flex_none()
            .px_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(border)
            .child(
                Button::new("pdf-previous-page")
                    .ghost()
                    .compact()
                    .icon(IconName::ChevronLeft)
                    .tooltip(t!("pdf.toolbar.previous_page"))
                    .on_click(cx.listener(|this, _, _, cx| this.step_page(false, cx))),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(t!("pdf.toolbar.page"))
                    .child(
                        div()
                            .w(rem_px(PAGE_BOX_WIDTH))
                            .child(Input::new(&self.page_input).small()),
                    )
                    .child(
                        div()
                            .text_color(muted)
                            .child(t!("pdf.toolbar.of", total = self.page_count)),
                    ),
            )
            .child(
                Button::new("pdf-next-page")
                    .ghost()
                    .compact()
                    .icon(IconName::ChevronRight)
                    .tooltip(t!("pdf.toolbar.next_page"))
                    .on_click(cx.listener(|this, _, _, cx| this.step_page(true, cx))),
            )
            .child(self.render_zoom_group(cx))
            .child(div().flex_1())
            .child(
                h_flex()
                    .key_context("PdfFindBar")
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .w(rem_px(FIND_BOX_WIDTH))
                            .child(Input::new(&self.finder.input).small()),
                    )
                    .child(
                        div()
                            .min_w(rem_px(64.0))
                            .text_sm()
                            .text_color(muted)
                            .child(status),
                    )
                    .child(
                        Button::new("pdf-find-previous")
                            .ghost()
                            .compact()
                            .icon(IconName::ChevronUp)
                            .tooltip(t!("pdf.find.previous"))
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.step_find(false, window, cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new("pdf-find-next")
                            .ghost()
                            .compact()
                            .icon(IconName::ChevronDown)
                            .tooltip(t!("pdf.find.next"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.step_find(true, window, cx)),
                            ),
                    ),
            )
    }

    fn render_zoom_group(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().downgrade();
        let current = self.zoom;
        ToggleGroup::new("pdf-zoom")
            .segmented()
            .small()
            .children(ZOOMS.map(|zoom| {
                Toggle::new(zoom_id(zoom))
                    .label(zoom_label(zoom))
                    .checked(zoom == current)
            }))
            .on_click(move |next, _, cx| {
                let chosen = ZOOMS
                    .iter()
                    .zip(next)
                    .find(|(zoom, checked)| (**zoom == current) != **checked)
                    .map(|(zoom, _)| *zoom);
                if let Some(zoom) = chosen
                    && let Err(error) = entity.update(cx, |view, cx| view.set_zoom(zoom, cx))
                {
                    tracing::debug!(%error, "pdf view dropped");
                }
            })
    }

    // The page box shows the page in view unless the user is typing a page number in it.
    fn keep_page_box_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page_input.focus_handle(cx).is_focused(window) {
            return;
        }
        let shown = (self.current_page().0 + 1).to_string();
        if self.page_input.read(cx).value().as_ref() != shown {
            self.page_input
                .update(cx, |input, cx| input.set_value(shown, window, cx));
        }
    }
}
