use gpui_kit::{Bounds, Hsla, PathBuilder, Pixels, Window, point, px, size};
use zenkai_types::{CellPos, ColIdx, FilterView, RowIdx};

use crate::paint_failure::PaintFailure;

const BUTTON_SIZE: f32 = 16.0;
const MIN_COLUMN: f32 = 18.0;
const MIN_BOX: f32 = 8.0;
const HIT_GROWTH: f32 = 2.0;
const MIN_HIT: f32 = 16.0;
// Header text stops this far before the button.
const TEXT_GAP: f32 = 6.0;

// What the grid draws for the sheet's AutoFilter, with the text each button announces.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterBar {
    pub view: FilterView,
    // One entry per column of the filter range, in order: the button's tooltip and name.
    pub labels: Vec<gpui_kit::SharedString>,
}

impl FilterBar {
    pub fn header_row(&self) -> RowIdx {
        self.view.range.start.row
    }

    pub fn is_header_cell(&self, pos: CellPos) -> bool {
        pos.row == self.header_row()
            && (self.view.range.start.col..=self.view.range.end.col).contains(&pos.col)
    }

    pub fn has_button(&self, col: ColIdx) -> bool {
        (self.view.range.start.col..=self.view.range.end.col).contains(&col)
            && !self.view.without_button.contains(&col)
    }

    pub fn is_filtered(&self, col: ColIdx) -> bool {
        self.view.criteria.iter().any(|(column, _)| *column == col)
    }

    pub fn label(&self, col: ColIdx) -> Option<&gpui_kit::SharedString> {
        let first = self.view.range.start.col;
        self.labels
            .get(usize::from(col.get().checked_sub(first.get())?))
    }

    // Row numbers from the header row to the last row of the range turn blue while any
    // column has criteria.
    pub fn blue_rows(&self) -> Option<(RowIdx, RowIdx)> {
        self.view
            .is_filtering()
            .then_some((self.view.range.start.row, self.view.range.end.row))
    }
}

// A rectangle in pixels of the grid's own coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    fn bounds(self, origin: gpui_kit::Point<Pixels>) -> Bounds<Pixels> {
        Bounds::new(
            point(origin.x + px(self.x), origin.y + px(self.y)),
            size(px(self.width), px(self.height)),
        )
    }
}

// The button sits in the bottom-right corner of the header cell, one pixel in from the
// gridlines. `cell` is the cell as drawn: already scaled by the grid zoom.
pub fn button_box(cell: Rect, zoom: f32) -> Option<Rect> {
    let side = (BUTTON_SIZE * zoom)
        .min(cell.height - 2.0)
        .min(cell.width - 4.0);
    (cell.width >= MIN_COLUMN && side >= MIN_BOX).then(|| Rect {
        x: cell.x + cell.width - 2.0 - side,
        y: cell.y + cell.height - 2.0 - side,
        width: side,
        height: side,
    })
}

// The box grown by two pixels each side, at least 16 by 16, slid inside the cell and cut to
// it where the cell is smaller.
pub fn hit_area(cell: Rect, button: Rect) -> Rect {
    let along = |centre: f32, wanted: f32, start: f32, length: f32| {
        if wanted >= length {
            (start, length)
        } else {
            let first = (centre - wanted / 2.0).clamp(start, start + length - wanted);
            (first, wanted)
        }
    };
    let (x, width) = along(
        button.x + button.width / 2.0,
        (button.width + 2.0 * HIT_GROWTH).max(MIN_HIT),
        cell.x,
        cell.width,
    );
    let (y, height) = along(
        button.y + button.height / 2.0,
        (button.height + 2.0 * HIT_GROWTH).max(MIN_HIT),
        cell.y,
        cell.height,
    );
    Rect {
        x,
        y,
        width,
        height,
    }
}

// How much narrower than the cell its text area is, so the header text ends before the
// button and right-aligned text keeps the same gap.
pub fn text_reserve(cell: Rect, zoom: f32, padding: f32) -> f32 {
    button_box(cell, zoom).map_or(0.0, |button| (button.width + TEXT_GAP - padding).max(0.0))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonState {
    Idle,
    Hover,
    Open,
}

#[derive(Clone, Copy)]
pub struct ButtonColors {
    pub face: Hsla,
    pub face_hover: Hsla,
    pub border: Hsla,
    pub glyph: Hsla,
    pub glyph_hover: Hsla,
    pub strong: Hsla,
    pub filter_blue: Hsla,
}

// Excel's filter button: a small button with a down triangle, which becomes a funnel while
// the column has criteria. The funnel and the blue border carry the state, not colour alone.
pub fn paint_button(
    button: Rect,
    origin: gpui_kit::Point<Pixels>,
    state: ButtonState,
    filtered: bool,
    colors: ButtonColors,
    window: &mut Window,
) {
    let area = button.bounds(origin);
    let face = if state == ButtonState::Idle {
        colors.face
    } else {
        colors.face_hover
    };
    let border = match (state, filtered) {
        (ButtonState::Open, _) => colors.strong,
        (_, true) => colors.filter_blue,
        _ => colors.border,
    };
    window.paint_quad(gpui_kit::quad(
        area,
        px(2.0),
        face,
        px(1.0),
        border,
        gpui_kit::BorderStyle::Solid,
    ));
    let glyph = match (filtered, state) {
        (true, _) => colors.filter_blue,
        (false, ButtonState::Idle) => colors.glyph,
        (false, _) => colors.glyph_hover,
    };
    let mut builder = PathBuilder::fill();
    let unit = button.width / 16.0;
    let at = |x: f32, y: f32| point(area.origin.x + px(x * unit), area.origin.y + px(y * unit));
    if filtered {
        builder.move_to(at(3.5, 4.0));
        builder.line_to(at(12.5, 4.0));
        builder.line_to(at(9.0, 8.5));
        builder.line_to(at(9.0, 12.5));
        builder.line_to(at(7.0, 11.5));
        builder.line_to(at(7.0, 8.5));
    } else {
        builder.move_to(at(4.0, 6.0));
        builder.line_to(at(12.0, 6.0));
        builder.line_to(at(8.0, 11.0));
    }
    builder.close();
    match builder.build() {
        Ok(path) => window.paint_path(path, glyph),
        Err(error) => {
            if PaintFailure::FilterGlyph.first_occurrence() {
                tracing::warn!(%error, "failed to build the filter button glyph");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use zenkai_types::{CellPos, ColIdx, ColumnCriteria, FilterView, ShownValues};

    use super::{FilterBar, Rect, button_box, hit_area, text_reserve};

    fn bar(criteria: Vec<(ColIdx, ColumnCriteria)>) -> FilterBar {
        FilterBar {
            view: FilterView {
                range: zenkai_types::Range::parse_a1("B2:D9").unwrap(),
                criteria,
                without_button: vec![ColIdx::clamped(2)],
                filtered_out: Vec::new(),
                found: 8,
                total: 8,
            },
            labels: vec!["B".into(), "C".into(), "D".into()],
        }
    }

    #[test]
    fn buttons_are_on_the_header_cells_of_the_range_except_the_columns_the_file_leaves_bare() {
        let bar = bar(Vec::new());
        assert!(bar.has_button(ColIdx::clamped(1)));
        assert!(!bar.has_button(ColIdx::clamped(2)));
        assert!(!bar.has_button(ColIdx::clamped(0)));
        assert!(bar.is_header_cell(CellPos::parse_a1("C2").unwrap()));
        assert!(!bar.is_header_cell(CellPos::parse_a1("C3").unwrap()));
        assert_eq!(
            bar.label(ColIdx::clamped(3)).map(|label| label.as_ref()),
            Some("D")
        );
        assert_eq!(bar.label(ColIdx::clamped(0)), None);
    }

    #[test]
    fn row_numbers_turn_blue_only_while_a_column_has_criteria() {
        assert_eq!(bar(Vec::new()).blue_rows(), None);
        let shown = ColumnCriteria::Values(ShownValues::default());
        let filtered = bar(vec![(ColIdx::clamped(1), shown)]);
        let rows = filtered.blue_rows().unwrap();
        assert_eq!((rows.0.get(), rows.1.get()), (1, 8));
        assert!(filtered.is_filtered(ColIdx::clamped(1)));
        assert!(!filtered.is_filtered(ColIdx::clamped(3)));
    }

    fn cell(width: f32, height: f32) -> Rect {
        Rect {
            x: 100.0,
            y: 40.0,
            width,
            height,
        }
    }

    #[test]
    fn the_box_follows_the_zoom_between_8_and_16_pixels() {
        let size = |zoom: f32| button_box(cell(64.0 * zoom, 20.0 * zoom), zoom).map(|b| b.width);
        assert_eq!(size(0.5), Some(8.0));
        assert_eq!(size(0.75), Some(12.0));
        assert_eq!(size(1.0), Some(16.0));
        assert_eq!(size(2.0), Some(32.0));
    }

    #[test]
    fn a_narrow_column_shrinks_the_box_and_a_very_narrow_one_has_none() {
        assert_eq!(
            button_box(cell(18.0, 20.0), 1.0).map(|b| b.width),
            Some(14.0)
        );
        assert_eq!(button_box(cell(17.0, 20.0), 1.0), None);
        assert_eq!(button_box(cell(64.0, 9.0), 1.0), None);
    }

    #[test]
    fn the_box_sits_one_pixel_in_from_the_gridlines() {
        let cell = cell(64.0, 20.0);
        let button = button_box(cell, 1.0).unwrap();
        assert_eq!(button.x + button.width, cell.x + cell.width - 2.0);
        assert_eq!(button.y + button.height, cell.y + cell.height - 2.0);
    }

    #[test]
    fn the_hit_area_is_at_least_16_square_and_stays_in_the_cell() {
        let hit = |zoom: f32| {
            let cell = cell(64.0 * zoom, 20.0 * zoom);
            let hit = hit_area(cell, button_box(cell, zoom).unwrap());
            (hit.width, hit.height)
        };
        assert_eq!(hit(0.5), (16.0, 10.0));
        assert_eq!(hit(0.75), (16.0, 15.0));
        assert_eq!(hit(1.0), (20.0, 20.0));
        let wide = cell(64.0, 20.0);
        let area = hit_area(wide, button_box(wide, 1.0).unwrap());
        assert!(area.x >= wide.x && area.x + area.width <= wide.x + wide.width);
    }

    #[test]
    fn header_text_ends_six_pixels_before_the_button() {
        let cell = cell(64.0, 20.0);
        assert_eq!(text_reserve(cell, 1.0, 4.0), 18.0);
        assert_eq!(text_reserve(self::cell(10.0, 20.0), 1.0, 4.0), 0.0);
    }
}
