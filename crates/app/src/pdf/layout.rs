use std::ops::Range;

use serde::{Deserialize, Serialize};

pub const PAGE_GAP: f32 = 14.0;
pub const CANVAS_PADDING: f32 = 16.0;
const POINTS_TO_PIXELS: f32 = 96.0 / 72.0;
const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 8.0;
const ZOOM_STEPS: [u16; 11] = [25, 50, 75, 100, 125, 150, 200, 250, 300, 400, 500];

pub use zenkai_pdf_host::wire::{PageIdx, PageSize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoomMode {
    FitPage,
    #[default]
    FitWidth,
    Percent(u16),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PdfPosition {
    pub page: u32,
    #[serde(deserialize_with = "zoom_or_default")]
    pub zoom: ZoomMode,
}

// A zoom this version does not know falls back to the default instead of failing the whole
// session file, which would drop every tab.
fn zoom_or_default<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<ZoomMode, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
}

// Pixels per PDF point that a zoom mode gives for the page on screen.
pub fn scale_for(mode: ZoomMode, page: PageSize, viewport: Viewport) -> f32 {
    let scale = match mode {
        ZoomMode::Percent(percent) => f32::from(percent) / 100.0 * POINTS_TO_PIXELS,
        ZoomMode::FitWidth => (viewport.width - 2.0 * CANVAS_PADDING) / page.width,
        ZoomMode::FitPage => ((viewport.width - 2.0 * CANVAS_PADDING) / page.width)
            .min((viewport.height - 2.0 * CANVAS_PADDING) / page.height),
    };
    if scale.is_finite() {
        scale.clamp(MIN_SCALE, MAX_SCALE)
    } else {
        POINTS_TO_PIXELS
    }
}

pub fn percent_of(scale: f32) -> u16 {
    (scale / POINTS_TO_PIXELS * 100.0)
        .round()
        .clamp(1.0, f32::from(u16::MAX)) as u16
}

// The next preset above or below the zoom now in effect, so zooming from a fit mode starts
// from what the user sees.
pub fn step_zoom(current_percent: u16, zoom_in: bool) -> ZoomMode {
    let next = if zoom_in {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|step| *step > current_percent)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < current_percent)
            .unwrap_or(ZOOM_STEPS[0])
    };
    ZoomMode::Percent(next)
}

#[derive(Clone, Debug)]
pub struct Layout {
    scale: f32,
    tops: Vec<f32>,
    heights: Vec<f32>,
    widths: Vec<f32>,
    total_height: f32,
    content_width: f32,
}

impl Layout {
    // A page whose size is not known yet takes the first page's, so a large file lays out
    // from its first page alone and corrects itself as the sizes arrive.
    pub fn new(sizes: &[PageSize], count: usize, scale: f32) -> Layout {
        let fallback = sizes.first().copied().unwrap_or(PageSize::LETTER);
        let mut tops = Vec::with_capacity(count);
        let mut heights = Vec::with_capacity(count);
        let mut widths = Vec::with_capacity(count);
        let mut y = CANVAS_PADDING;
        let mut widest = 0.0f32;
        for index in 0..count {
            let size = sizes.get(index).copied().unwrap_or(fallback);
            let (width, height) = (size.width * scale, size.height * scale);
            tops.push(y);
            heights.push(height);
            widths.push(width);
            widest = widest.max(width);
            y += height + PAGE_GAP;
        }
        let total_height = if count == 0 {
            0.0
        } else {
            y - PAGE_GAP + CANVAS_PADDING
        };
        Layout {
            scale,
            tops,
            heights,
            widths,
            total_height,
            content_width: widest + 2.0 * CANVAS_PADDING,
        }
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn page_count(&self) -> usize {
        self.tops.len()
    }

    pub fn top_of(&self, page: PageIdx) -> f32 {
        self.tops.get(page.0).copied().unwrap_or(0.0)
    }

    pub fn height_of(&self, page: PageIdx) -> f32 {
        self.heights.get(page.0).copied().unwrap_or(0.0)
    }

    pub fn width_of(&self, page: PageIdx) -> f32 {
        self.widths.get(page.0).copied().unwrap_or(0.0)
    }

    // Pages are centred in the canvas; when the canvas is wider than the content the left
    // edge moves right, and when narrower the horizontal scroll moves it left.
    pub fn left_of(&self, page: PageIdx, canvas_width: f32, scroll_left: f32) -> f32 {
        let room = (canvas_width - self.width_of(page)).max(0.0) / 2.0;
        let centred = if self.content_width > canvas_width {
            CANVAS_PADDING
        } else {
            room
        };
        centred - scroll_left
    }

    pub fn max_scroll_top(&self, viewport_height: f32) -> f32 {
        (self.total_height - viewport_height).max(0.0)
    }

    pub fn max_scroll_left(&self, viewport_width: f32) -> f32 {
        (self.content_width - viewport_width).max(0.0)
    }

    // The page whose area, or the gap after it, holds `offset`.
    pub fn page_at(&self, offset: f32) -> PageIdx {
        let after = self.tops.partition_point(|top| *top <= offset);
        PageIdx(after.saturating_sub(1))
    }

    // Pages touching the viewport plus `neighbours` more on each side.
    pub fn visible(
        &self,
        scroll_top: f32,
        viewport_height: f32,
        neighbours: usize,
    ) -> Range<usize> {
        let count = self.page_count();
        if count == 0 {
            return 0..0;
        }
        let first = self.page_at(scroll_top).0;
        let last = self.page_at(scroll_top + viewport_height).0;
        first.saturating_sub(neighbours)..(last + 1 + neighbours).min(count)
    }

    // The page showing the most of the viewport, the first on a tie.
    pub fn current_page(&self, scroll_top: f32, viewport_height: f32) -> PageIdx {
        let viewport = scroll_top..scroll_top + viewport_height;
        let range = self.visible(scroll_top, viewport_height, 0);
        let mut best = (PageIdx(range.start), f32::MIN);
        for index in range {
            let top = self.tops[index];
            let overlap = (top + self.heights[index]).min(viewport.end) - top.max(viewport.start);
            if overlap > best.1 {
                best = (PageIdx(index), overlap);
            }
        }
        best.0
    }

    // The scroll offset in this layout that shows what `scroll_top` showed in `previous`: the
    // same page at the same distance from its top, in PDF points.
    pub fn scroll_after_rescale(&self, previous: &Layout, scroll_top: f32) -> f32 {
        if previous.page_count() == 0 {
            return 0.0;
        }
        let page = previous.page_at(scroll_top);
        let into_page = (scroll_top - previous.top_of(page)) * self.scale / previous.scale;
        self.top_of(page) + into_page
    }

    pub fn scroll_to_page(&self, page: PageIdx, viewport_height: f32) -> f32 {
        let top = (self.top_of(page) - CANVAS_PADDING).max(0.0);
        top.min(self.max_scroll_top(viewport_height))
    }
}

const MAX_RENDER_PIXELS: f32 = 16_000_000.0;

// The bitmap size for a page shown at `width` by `height` logical pixels. A page zoomed past
// the cap renders smaller and is stretched, so one page never takes more than ~64 MB.
pub fn render_size(width: f32, height: f32, device_scale: f32) -> (u32, u32) {
    let (mut width, mut height) = (width * device_scale, height * device_scale);
    let pixels = width * height;
    if pixels > MAX_RENDER_PIXELS {
        let shrink = (MAX_RENDER_PIXELS / pixels).sqrt();
        width *= shrink;
        height *= shrink;
    }
    (
        width.round().max(1.0) as u32,
        height.round().max(1.0) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(count: usize) -> Layout {
        Layout::new(&[PageSize::LETTER], count, 1.0)
    }

    #[test]
    fn pages_stack_with_a_gap_and_padding() {
        let layout = pages(3);
        assert_eq!(layout.top_of(PageIdx(0)), CANVAS_PADDING);
        assert_eq!(layout.top_of(PageIdx(1)), CANVAS_PADDING + 792.0 + PAGE_GAP);
        let expected = 2.0 * CANVAS_PADDING + 3.0 * 792.0 + 2.0 * PAGE_GAP;
        assert_eq!(layout.total_height, expected);
    }

    #[test]
    fn an_unknown_size_takes_the_first_pages_size() {
        let layout = Layout::new(
            &[PageSize {
                width: 100.0,
                height: 200.0,
            }],
            3,
            2.0,
        );
        assert_eq!(layout.height_of(PageIdx(2)), 400.0);
        assert_eq!(layout.width_of(PageIdx(2)), 200.0);
    }

    #[test]
    fn a_known_size_replaces_the_fallback() {
        let sizes = [
            PageSize::LETTER,
            PageSize {
                width: 300.0,
                height: 400.0,
            },
        ];
        let layout = Layout::new(&sizes, 2, 1.0);
        assert_eq!(layout.height_of(PageIdx(1)), 400.0);
        assert_eq!(layout.content_width, 612.0 + 2.0 * CANVAS_PADDING);
    }

    #[test]
    fn no_pages_make_an_empty_layout() {
        let layout = Layout::new(&[], 0, 1.0);
        assert_eq!(layout.total_height, 0.0);
        assert_eq!(layout.visible(0.0, 500.0, 2), 0..0);
        assert_eq!(layout.page_at(100.0), PageIdx(0));
    }

    #[test]
    fn the_page_at_an_offset_includes_the_gap_after_it() {
        let layout = pages(3);
        assert_eq!(layout.page_at(0.0), PageIdx(0));
        assert_eq!(layout.page_at(CANVAS_PADDING + 792.0 + 5.0), PageIdx(0));
        assert_eq!(layout.page_at(layout.top_of(PageIdx(1))), PageIdx(1));
        assert_eq!(layout.page_at(1_000_000.0), PageIdx(2));
    }

    #[test]
    fn visible_pages_grow_by_the_neighbours_and_stop_at_the_ends() {
        let layout = pages(10);
        let at = layout.top_of(PageIdx(4));
        assert_eq!(layout.visible(at, 100.0, 0), 4..5);
        assert_eq!(layout.visible(at, 100.0, 1), 3..6);
        assert_eq!(layout.visible(0.0, 100.0, 2), 0..3);
        assert_eq!(layout.visible(layout.total_height, 100.0, 3), 6..10);
    }

    #[test]
    fn a_tall_viewport_spans_several_pages() {
        let layout = pages(10);
        let range = layout.visible(0.0, 2000.0, 0);
        assert_eq!(range, 0..3);
    }

    #[test]
    fn the_current_page_is_the_one_showing_most_of_the_viewport() {
        let layout = pages(4);
        let second = layout.top_of(PageIdx(1));
        assert_eq!(layout.current_page(0.0, 600.0), PageIdx(0));
        assert_eq!(layout.current_page(second - 100.0, 600.0), PageIdx(1));
        assert_eq!(layout.current_page(second + 300.0, 600.0), PageIdx(1));
    }

    #[test]
    fn a_rescale_keeps_the_reader_at_the_same_place_in_the_same_page() {
        let sizes = [PageSize {
            width: 600.0,
            height: 800.0,
        }; 5];
        let before = Layout::new(&sizes, 5, 1.0);
        let scroll = before.top_of(PageIdx(2)) + 300.0;
        let after = Layout::new(&sizes, 5, 0.5);
        let moved = after.scroll_after_rescale(&before, scroll);
        assert_eq!(moved, after.top_of(PageIdx(2)) + 150.0);
        assert_eq!(after.page_at(moved), PageIdx(2));
        let same = Layout::new(&sizes, 5, 1.0);
        assert_eq!(same.scroll_after_rescale(&before, scroll), scroll);
    }

    #[test]
    fn scrolling_to_a_page_clamps_to_the_end() {
        let layout = pages(3);
        assert_eq!(layout.scroll_to_page(PageIdx(0), 500.0), 0.0);
        assert_eq!(
            layout.scroll_to_page(PageIdx(1), 500.0),
            layout.top_of(PageIdx(1)) - CANVAS_PADDING
        );
        let end = layout.max_scroll_top(2000.0);
        assert_eq!(layout.scroll_to_page(PageIdx(2), 2000.0), end);
    }

    #[test]
    fn fit_width_fills_the_viewport_minus_padding() {
        let viewport = Viewport {
            width: 632.0,
            height: 400.0,
        };
        let scale = scale_for(ZoomMode::FitWidth, PageSize::LETTER, viewport);
        assert!((scale * 612.0 - (632.0 - 2.0 * CANVAS_PADDING)).abs() < 0.01);
    }

    #[test]
    fn fit_page_is_limited_by_the_tighter_side() {
        let viewport = Viewport {
            width: 2000.0,
            height: 500.0,
        };
        let scale = scale_for(ZoomMode::FitPage, PageSize::LETTER, viewport);
        assert!((scale * 792.0 - (500.0 - 2.0 * CANVAS_PADDING)).abs() < 0.01);
    }

    #[test]
    fn one_hundred_percent_is_the_actual_size_at_96_dpi() {
        let viewport = Viewport {
            width: 800.0,
            height: 600.0,
        };
        let scale = scale_for(ZoomMode::Percent(100), PageSize::LETTER, viewport);
        assert!((scale - 96.0 / 72.0).abs() < 1e-6);
        assert_eq!(percent_of(scale), 100);
    }

    #[test]
    fn a_degenerate_viewport_still_gives_a_usable_scale() {
        let viewport = Viewport {
            width: 0.0,
            height: 0.0,
        };
        let scale = scale_for(ZoomMode::FitPage, PageSize::LETTER, viewport);
        assert!(scale >= MIN_SCALE);
        let zero_page = PageSize {
            width: 0.0,
            height: 0.0,
        };
        assert!(scale_for(ZoomMode::FitWidth, zero_page, viewport).is_finite());
    }

    #[test]
    fn zoom_steps_move_to_the_next_preset_from_any_value() {
        assert_eq!(step_zoom(100, true), ZoomMode::Percent(125));
        assert_eq!(step_zoom(100, false), ZoomMode::Percent(75));
        assert_eq!(step_zoom(110, true), ZoomMode::Percent(125));
        assert_eq!(step_zoom(110, false), ZoomMode::Percent(100));
        assert_eq!(step_zoom(500, true), ZoomMode::Percent(500));
        assert_eq!(step_zoom(25, false), ZoomMode::Percent(25));
    }

    #[test]
    fn pages_centre_when_narrower_than_the_canvas_and_scroll_when_wider() {
        let layout = pages(1);
        let left = layout.left_of(PageIdx(0), 1000.0, 0.0);
        assert!((left - (1000.0 - 612.0) / 2.0).abs() < 1e-3);
        assert_eq!(
            layout.left_of(PageIdx(0), 300.0, 40.0),
            CANVAS_PADDING - 40.0
        );
        assert_eq!(layout.max_scroll_left(300.0), layout.content_width - 300.0);
    }

    #[test]
    fn a_partial_or_unknown_position_reads_with_defaults() {
        let empty: PdfPosition = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, PdfPosition::default());
        let page_only: PdfPosition = serde_json::from_str(r#"{"page":4}"#).unwrap();
        assert_eq!(page_only.page, 4);
        assert_eq!(page_only.zoom, ZoomMode::default());
        let odd: PdfPosition = serde_json::from_str(r#"{"page":2,"zoom":"fit_height"}"#).unwrap();
        assert_eq!((odd.page, odd.zoom), (2, ZoomMode::default()));
        let wrong: PdfPosition = serde_json::from_str(r#"{"zoom":{"percent":"x"}}"#).unwrap();
        assert_eq!(wrong.zoom, ZoomMode::default());
    }

    #[test]
    fn the_position_round_trips_through_json() {
        for zoom in [
            ZoomMode::FitPage,
            ZoomMode::FitWidth,
            ZoomMode::Percent(150),
        ] {
            let position = PdfPosition { page: 7, zoom };
            let text = serde_json::to_string(&position).unwrap();
            assert_eq!(
                serde_json::from_str::<PdfPosition>(&text).unwrap(),
                position
            );
        }
    }

    #[test]
    fn a_huge_page_renders_under_the_pixel_cap_with_its_proportions() {
        let (width, height) = render_size(8000.0, 12000.0, 2.0);
        assert!(u64::from(width) * u64::from(height) <= 16_100_000);
        let ratio = width as f32 / height as f32;
        assert!((ratio - 8000.0 / 12000.0).abs() < 0.01);
        assert_eq!(render_size(600.0, 800.0, 2.0), (1200, 1600));
        assert_eq!(render_size(0.0, 0.0, 1.0), (1, 1));
    }
}
