use gpui_kit::*;

use super::{KEY_SCROLL, PdfEvent, PdfView, WHEEL_LINE};
use crate::pdf::layout::{PageIdx, PdfPosition, ZoomMode, percent_of, step_zoom};

impl PdfView {
    pub(super) fn moved(&mut self, cx: &mut Context<Self>) {
        self.clamp_scroll();
        self.request_visible();
        let position = PdfPosition {
            page: u32::try_from(self.current_page().0).unwrap_or(0),
            zoom: self.zoom,
        };
        if position != self.reported {
            self.reported = position;
            cx.emit(PdfEvent::Moved(position));
            self.thumbnail_scroll
                .scroll_to_item(position.page as usize, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    pub fn go_to_page(&mut self, page: PageIdx, cx: &mut Context<Self>) {
        if !self.is_ready() || self.page_count == 0 {
            return;
        }
        let page = PageIdx(page.0.min(self.page_count - 1));
        self.scroll_top = self.layout.scroll_to_page(page, self.viewport().height);
        self.moved(cx);
    }

    pub fn step_page(&mut self, forward: bool, cx: &mut Context<Self>) {
        let current = self.current_page().0;
        let target = if forward {
            current + 1
        } else {
            current.saturating_sub(1)
        };
        self.go_to_page(PageIdx(target), cx);
    }

    pub fn last_page(&mut self, cx: &mut Context<Self>) {
        self.go_to_page(PageIdx(self.page_count.saturating_sub(1)), cx);
    }

    pub(super) fn go_from_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.page_input.read(cx).value().to_string();
        if let Ok(number) = typed.trim().parse::<usize>()
            && number >= 1
        {
            self.go_to_page(PageIdx(number - 1), cx);
        }
        self.sync_page_input(window, cx);
        window.focus(&self.focus, cx);
    }

    pub(super) fn sync_page_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = (self.current_page().0 + 1).to_string();
        self.page_input
            .update(cx, |input, cx| input.set_value(shown, window, cx));
    }

    pub fn focus_page_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_page_input(window, cx);
        self.page_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    pub fn actual_size(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(ZoomMode::Percent(100), cx);
    }

    pub fn set_zoom(&mut self, zoom: ZoomMode, cx: &mut Context<Self>) {
        if self.zoom == zoom {
            return;
        }
        self.zoom = zoom;
        self.relayout();
        self.moved(cx);
    }

    pub fn step_zoom(&mut self, zoom_in: bool, cx: &mut Context<Self>) {
        let percent = percent_of(self.layout.scale());
        self.set_zoom(step_zoom(percent, zoom_in), cx);
    }

    pub fn scroll_by(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        self.scroll_top += dy;
        self.scroll_left += dx;
        self.moved(cx);
    }

    pub(super) fn on_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(WHEEL_LINE));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.modifiers.control {
            self.step_zoom(dy > 0.0, cx);
        } else if event.modifiers.shift {
            self.scroll_by(-dy, 0.0, cx);
        } else {
            self.scroll_by(-dx, -dy, cx);
        }
    }

    pub(super) fn on_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.shift {
            return;
        }
        let (dx, dy) = match keystroke.key.as_str() {
            "up" => (0.0, -KEY_SCROLL),
            "down" => (0.0, KEY_SCROLL),
            "left" => (-KEY_SCROLL, 0.0),
            "right" => (KEY_SCROLL, 0.0),
            _ => return,
        };
        self.scroll_by(dx, dy, cx);
        cx.stop_propagation();
    }
}
