use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

pub const THICKNESS: f32 = 14.0;
const MIN_THUMB: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

// Where the viewport sits among the rows or columns past the frozen panes. `used` counts
// them. The range ends when the last used one reaches the top, as in Excel, and runs
// further once the user scrolled beyond it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scroll {
    pub first: u32,
    pub visible: u32,
    pub used: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thumb {
    pub start: f32,
    pub length: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Back,
    Forward,
}

impl Scroll {
    fn scrollable(self) -> u32 {
        self.used.saturating_sub(1).max(self.first)
    }

    pub fn thumb(self, track: f32) -> Thumb {
        let total = (self.scrollable() + self.visible).max(1) as f32;
        let length = (track * self.visible as f32 / total).clamp(MIN_THUMB.min(track), track);
        let travel = track - length;
        let start = match self.scrollable() {
            0 => 0.0,
            scrollable => travel * self.first as f32 / scrollable as f32,
        };
        Thumb { start, length }
    }

    pub fn first_at(self, thumb_start: f32, track: f32) -> u32 {
        let travel = track - self.thumb(track).length;
        if travel <= 0.0 {
            return 0;
        }
        (thumb_start.clamp(0.0, travel) / travel * self.scrollable() as f32).round() as u32
    }

    pub fn paged(self, page: Page) -> u32 {
        let step = self.visible.saturating_sub(1).max(1);
        match page {
            Page::Back => self.first.saturating_sub(step),
            Page::Forward => (self.first + step).min(self.scrollable()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Track {
    pub start: f32,
    pub length: f32,
}

impl Track {
    // `lead` is the header the track starts after; the far end leaves room for the
    // other scrollbar.
    pub fn along(extent: f32, lead: f32) -> Track {
        Track {
            start: lead,
            length: (extent - THICKNESS - lead).max(0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BarHit {
    Thumb { axis: Axis, grab: f32 },
    Track { axis: Axis, page: Page },
}

// `position` and `size` are in pixels, measured from the grid's top left.
pub fn hit(
    axis: Axis,
    position: (f32, f32),
    size: (f32, f32),
    track: Track,
    scroll: Scroll,
) -> Option<BarHit> {
    let ((along, cross), extent_cross) = match axis {
        Axis::Vertical => ((position.1, position.0), size.0),
        Axis::Horizontal => ((position.0, position.1), size.1),
    };
    let on_bar = cross >= extent_cross - THICKNESS
        && along >= track.start
        && along < track.start + track.length;
    if !on_bar {
        return None;
    }
    let thumb = scroll.thumb(track.length);
    let thumb_start = track.start + thumb.start;
    Some(if along < thumb_start {
        BarHit::Track {
            axis,
            page: Page::Back,
        }
    } else if along >= thumb_start + thumb.length {
        BarHit::Track {
            axis,
            page: Page::Forward,
        }
    } else {
        BarHit::Thumb {
            axis,
            grab: along - thumb_start,
        }
    })
}

#[derive(Clone, Copy, Debug)]
pub struct Bar {
    pub scroll: Scroll,
    pub track: Track,
    pub dragging: bool,
}

pub fn paint(
    vertical: Bar,
    horizontal: Bar,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &App,
) {
    let theme = cx.theme();
    let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let at = |x: f32, y: f32, w: f32, h: f32| Bounds {
        origin: bounds.origin + point(px(x), px(y)),
        size: size(px(w), px(h)),
    };
    let corner = at(width - THICKNESS, height - THICKNESS, THICKNESS, THICKNESS);
    window.paint_quad(fill(corner, theme.background));
    window.paint_quad(fill(corner, theme.scrollbar));
    for (axis, bar) in [(Axis::Vertical, vertical), (Axis::Horizontal, horizontal)] {
        let thumb = bar.scroll.thumb(bar.track.length);
        let (track, thumb) = match axis {
            Axis::Vertical => (
                at(
                    width - THICKNESS,
                    bar.track.start,
                    THICKNESS,
                    bar.track.length,
                ),
                at(
                    width - THICKNESS + 2.0,
                    bar.track.start + thumb.start,
                    THICKNESS - 4.0,
                    thumb.length,
                ),
            ),
            Axis::Horizontal => (
                at(
                    bar.track.start,
                    height - THICKNESS,
                    bar.track.length,
                    THICKNESS,
                ),
                at(
                    bar.track.start + thumb.start,
                    height - THICKNESS + 2.0,
                    thumb.length,
                    THICKNESS - 4.0,
                ),
            ),
        };
        window.paint_quad(fill(track, theme.background));
        window.paint_quad(fill(track, theme.scrollbar));
        let color = if bar.dragging {
            theme.scrollbar_thumb_hover
        } else {
            theme.scrollbar_thumb
        };
        window.paint_quad(quad(
            thumb,
            px(3.0),
            color,
            px(0.0),
            transparent_black(),
            BorderStyle::default(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{Axis, BarHit, MIN_THUMB, Page, Scroll, Track, hit};

    fn scroll(first: u32) -> Scroll {
        Scroll {
            first,
            visible: 40,
            used: 200_001,
        }
    }

    #[test]
    fn the_thumb_moves_from_the_top_to_the_bottom_of_the_track() {
        let top = scroll(0).thumb(600.0);
        let bottom = scroll(200_000).thumb(600.0);
        assert_eq!(top.start, 0.0);
        assert_eq!(top.length, bottom.length);
        assert!((bottom.start + bottom.length - 600.0).abs() < 0.01);
    }

    #[test]
    fn the_last_used_row_is_the_furthest_the_thumb_goes() {
        let sheet = |used| Scroll {
            first: 0,
            visible: 40,
            used,
        };
        assert_eq!(sheet(100).first_at(600.0, 600.0), 99);
        assert_eq!(sheet(100).paged(Page::Forward), 39);
        assert_eq!(sheet(1).first_at(600.0, 600.0), 0);
        assert_eq!(sheet(1).paged(Page::Forward), 0);
    }

    #[test]
    fn a_short_sheet_gets_a_long_thumb_and_a_huge_one_stays_grabbable() {
        let short = Scroll {
            first: 0,
            visible: 40,
            used: 10,
        };
        assert!(short.thumb(600.0).length > 400.0);
        assert_eq!(scroll(0).thumb(600.0).length, MIN_THUMB);
    }

    #[test]
    fn dragging_to_a_position_jumps_there_and_back() {
        let state = scroll(0);
        let length = state.thumb(600.0).length;
        assert_eq!(state.first_at(0.0, 600.0), 0);
        assert_eq!(state.first_at(600.0, 600.0), 200_000);
        let half = state.first_at((600.0 - length) / 2.0, 600.0);
        assert_eq!(half, 100_000);
        let placed = scroll(half).thumb(600.0).start;
        assert!((placed - (600.0 - length) / 2.0).abs() < 0.01);
    }

    #[test]
    fn paging_moves_one_screen_less_a_row_and_stops_at_the_ends() {
        assert_eq!(scroll(100).paged(Page::Forward), 139);
        assert_eq!(scroll(100).paged(Page::Back), 61);
        assert_eq!(scroll(10).paged(Page::Back), 0);
        assert_eq!(scroll(199_990).paged(Page::Forward), 200_000);
    }

    #[test]
    fn clicks_land_on_the_thumb_or_either_side_of_it() {
        let track = Track::along(700.0, 22.0);
        let size = (1000.0, 700.0);
        let state = scroll(100_000);
        let thumb = state.thumb(track.length);
        let x = 995.0;
        let on = |y: f32| hit(Axis::Vertical, (x, y), size, track, state);
        let thumb_y = track.start + thumb.start;
        assert!(matches!(
            on(thumb_y + 1.0),
            Some(BarHit::Thumb { grab, .. }) if (grab - 1.0).abs() < 0.01
        ));
        assert!(matches!(
            on(thumb_y - 1.0),
            Some(BarHit::Track {
                page: Page::Back,
                ..
            })
        ));
        assert!(matches!(
            on(thumb_y + thumb.length + 1.0),
            Some(BarHit::Track {
                page: Page::Forward,
                ..
            })
        ));
        assert_eq!(
            hit(Axis::Vertical, (500.0, 100.0), size, track, state),
            None
        );
    }
}
