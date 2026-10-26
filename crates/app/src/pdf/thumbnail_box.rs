use super::layout::PageSize;

pub const GUTTER: f32 = 16.0;
pub const GAP: f32 = 16.0;
const RATIO_CAP: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub width: f32,
    pub height: f32,
}

fn ratio(page: PageSize) -> f32 {
    page.height / page.width.max(1.0)
}

fn available(side: f32) -> f32 {
    (side - 2.0 * GUTTER).max(1.0)
}

pub fn tallest_ratio(sizes: &[PageSize]) -> f32 {
    if sizes.is_empty() {
        return ratio(PageSize::LETTER);
    }
    sizes
        .iter()
        .map(|page| ratio(*page))
        .fold(0.0, f32::max)
        .min(RATIO_CAP)
}

pub fn row_height(side: f32, tallest_ratio: f32) -> f32 {
    (available(side) * tallest_ratio.min(RATIO_CAP)).round() + GAP
}

pub fn fit(side: f32, page: PageSize) -> Fit {
    let width = available(side);
    let page_ratio = ratio(page);
    if page_ratio <= RATIO_CAP {
        Fit {
            width,
            height: (width * page_ratio).round().max(1.0),
        }
    } else {
        let height = width * RATIO_CAP;
        Fit {
            width: (height / page_ratio).round().max(1.0),
            height: height.round(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4: PageSize = PageSize {
        width: 595.0,
        height: 842.0,
    };
    const SLIDE: PageSize = PageSize {
        width: 800.0,
        height: 600.0,
    };
    const STRIP: PageSize = PageSize {
        width: 100.0,
        height: 500.0,
    };

    #[test]
    fn the_row_fits_the_tallest_page_plus_the_gap() {
        let ratio = tallest_ratio(&[SLIDE, A4]);
        assert_eq!(ratio, 842.0 / 595.0);
        assert_eq!(row_height(300.0, ratio), (268.0 * ratio).round() + GAP);
    }

    #[test]
    fn a_very_tall_page_caps_the_row_ratio_at_two() {
        assert_eq!(tallest_ratio(&[A4, STRIP]), 2.0);
        assert_eq!(row_height(232.0, 2.0), 400.0 + GAP);
    }

    #[test]
    fn a_document_of_slides_gets_short_rows() {
        let ratio = tallest_ratio(&[SLIDE; 5]);
        assert_eq!(ratio, 0.75);
        assert_eq!(row_height(200.0, ratio), 126.0 + GAP);
    }

    #[test]
    fn a_portrait_page_fills_the_panel_minus_the_gutters() {
        let fit = fit(300.0, A4);
        assert_eq!(fit.width, 268.0);
        assert_eq!(fit.height, (268.0_f32 * 842.0 / 595.0).round());
    }

    #[test]
    fn a_landscape_page_keeps_its_ratio_and_is_shorter_than_the_row() {
        let fit = fit(200.0, SLIDE);
        assert_eq!((fit.width, fit.height), (168.0, 126.0));
        assert!(fit.height < row_height(200.0, tallest_ratio(&[SLIDE, A4])) - GAP);
    }

    #[test]
    fn a_page_taller_than_the_cap_is_fit_by_height() {
        let fit = fit(232.0, STRIP);
        assert_eq!((fit.width, fit.height), (80.0, 400.0));
    }

    #[test]
    fn no_box_exceeds_its_row_or_the_panel_gutters() {
        for side in [120.0, 200.0, 300.0, 600.0] {
            let sizes = [A4, SLIDE, STRIP, PageSize::LETTER];
            let row = row_height(side, tallest_ratio(&sizes)) - GAP;
            for page in sizes {
                let fit = fit(side, page);
                assert!(fit.width <= side - 2.0 * GUTTER);
                assert!(fit.height <= row);
            }
        }
    }
}
