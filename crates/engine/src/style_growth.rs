// A patch save keeps the file's styles part and appends the styles the edits created, so
// every style a copied sheet points at keeps its index and its bytes. IronCalc only ever
// appends to its fonts, fills, borders, number formats and cell formats; anything else
// changing a style that exists (a named style updated, a conditional format's dxf added)
// is a change the part is written whole for.

use std::hash::{DefaultHasher, Hasher};

use ironcalc::base::types::Styles;
use ironcalc::export::{StyleItemCounts, get_style_items_xml};
use zenkai_xlsx_reader::{StyleAppendix, StyleCounts, StylesShape};

use crate::error::EngineError;
use crate::fingerprint::hash_debug;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StyleGrowth {
    Appends,
    Never,
}

// What the file's styles part holds, and a print of the model's styles over it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StylesBase {
    pub(crate) counts: StyleCounts,
    growth: StyleGrowth,
    print: u64,
}

pub(crate) fn model_counts(styles: &Styles) -> StyleCounts {
    StyleCounts {
        num_fmts: styles.num_fmts.len(),
        fonts: styles.fonts.len(),
        fills: styles.fills.len(),
        borders: styles.borders.len(),
        cell_style_xfs: styles.cell_style_xfs.len(),
        cell_xfs: styles.cell_xfs.len(),
        cell_styles: styles.cell_styles.len(),
        dxfs: styles.dxfs.len(),
        shape: StylesShape::Appendable,
    }
}

fn prefix<T>(items: &[T], kept: usize) -> (bool, &[T]) {
    (items.len() >= kept, &items[..kept.min(items.len())])
}

// With `Appends`, the first items of each list the file holds and every item of the lists
// that cannot grow; with `Never`, everything.
fn print(styles: &Styles, growth: StyleGrowth, file: &StyleCounts) -> Result<u64, EngineError> {
    let Styles {
        num_fmts,
        fonts,
        fills,
        borders,
        cell_style_xfs,
        cell_xfs,
        cell_styles,
        dxfs,
    } = styles;
    let mut hasher = DefaultHasher::new();
    match growth {
        StyleGrowth::Never => hash_debug(&mut hasher, styles)?,
        StyleGrowth::Appends => hash_debug(
            &mut hasher,
            &(
                prefix(num_fmts, file.num_fmts),
                prefix(fonts, file.fonts),
                prefix(fills, file.fills),
                prefix(borders, file.borders),
                prefix(cell_xfs, file.cell_xfs),
                (cell_style_xfs, cell_styles, dxfs),
            ),
        )?,
    }
    Ok(hasher.finish())
}

impl StylesBase {
    // The file was just written from `styles` by IronCalc.
    pub(crate) fn written(styles: &Styles) -> Result<StylesBase, EngineError> {
        let counts = model_counts(styles);
        Ok(StylesBase {
            counts,
            growth: StyleGrowth::Appends,
            print: print(styles, StyleGrowth::Appends, &counts)?,
        })
    }

    // None when the sheets of the file would point into other styles than the model's.
    pub(crate) fn opened(
        file: StyleCounts,
        styles: &Styles,
    ) -> Result<Option<StylesBase>, EngineError> {
        let model = model_counts(styles);
        if file.cell_xfs != model.cell_xfs || file.dxfs != model.dxfs {
            return Ok(None);
        }
        let lined_up = StyleCounts {
            shape: file.shape,
            ..model
        } == file;
        // The model starts its fills with two defaults the file must hold too.
        let growth = match (lined_up, file.shape, file.fills >= 2) {
            (true, StylesShape::Appendable, true) => StyleGrowth::Appends,
            _ => StyleGrowth::Never,
        };
        Ok(Some(StylesBase {
            counts: file,
            growth,
            print: print(styles, growth, &file)?,
        }))
    }

    // Whether the styles the file holds are still the model's first ones.
    pub(crate) fn unchanged(&self, styles: &Styles) -> Result<bool, EngineError> {
        Ok(print(styles, self.growth, &self.counts)? == self.print)
    }

    // What the file holds once the styles added since are in it, and those styles.
    pub(crate) fn grown(
        &self,
        styles: &Styles,
    ) -> Result<(StylesBase, Option<StyleAppendix>), EngineError> {
        let model = model_counts(styles);
        let file = self.counts;
        let added = (
            model.num_fmts,
            model.fonts,
            model.fills,
            model.borders,
            model.cell_xfs,
        ) != (
            file.num_fmts,
            file.fonts,
            file.fills,
            file.borders,
            file.cell_xfs,
        );
        if self.growth == StyleGrowth::Never || !added {
            return Ok((*self, None));
        }
        let items = get_style_items_xml(
            styles,
            StyleItemCounts {
                num_fmts: file.num_fmts,
                fonts: file.fonts,
                fills: file.fills,
                borders: file.borders,
                cell_xfs: file.cell_xfs,
            },
        );
        let after = StyleCounts {
            shape: file.shape,
            ..model
        };
        let appendix = StyleAppendix {
            after,
            num_fmts: items.num_fmts,
            fonts: items.fonts,
            fills: items.fills,
            borders: items.borders,
            cell_xfs: items.cell_xfs,
        };
        let base = StylesBase {
            counts: after,
            growth: self.growth,
            print: print(styles, self.growth, &after)?,
        };
        Ok((base, Some(appendix)))
    }
}

#[cfg(test)]
mod tests {
    use ironcalc::base::Model;
    use ironcalc::base::types::Style;

    use super::*;

    fn written() -> (Model<'static>, StylesBase) {
        let model = Model::new_empty("b", "en", "UTC", "en").unwrap();
        let base = StylesBase::written(&model.workbook.styles).unwrap();
        (model, base)
    }

    #[test]
    fn styles_added_after_the_file_was_written_are_appended() {
        let (mut model, base) = written();
        assert!(base.unchanged(&model.workbook.styles).unwrap());
        let mut style = Style::default();
        style.font.b = true;
        model.set_cell_style(0, 1, 1, &style).unwrap();
        let styles = &model.workbook.styles;
        assert!(base.unchanged(styles).unwrap());
        let (grown, appendix) = base.grown(styles).unwrap();
        let appendix = appendix.unwrap();
        assert_eq!(appendix.after.cell_xfs, base.counts.cell_xfs + 1);
        assert_eq!(appendix.after.fonts, base.counts.fonts + 1);
        assert!(appendix.fonts.contains("<b/>"));
        assert!(grown.unchanged(styles).unwrap());
        assert!(grown.grown(styles).unwrap().1.is_none());
    }

    #[test]
    fn a_style_that_existed_changing_is_not_an_append() {
        let (mut model, base) = written();
        model.workbook.styles.cell_xfs[0].num_fmt_id = 4;
        assert!(!base.unchanged(&model.workbook.styles).unwrap());
        let (mut model, base) = written();
        model.workbook.styles.cell_styles[0].name = "Other".to_string();
        assert!(!base.unchanged(&model.workbook.styles).unwrap());
    }

    #[test]
    fn a_file_whose_lists_do_not_line_up_with_the_model_takes_no_new_styles() {
        let (mut model, _) = written();
        let mut file = model_counts(&model.workbook.styles);
        file.fonts += 1;
        let base = StylesBase::opened(file, &model.workbook.styles)
            .unwrap()
            .unwrap();
        let mut style = Style::default();
        style.font.i = true;
        model.set_cell_style(0, 1, 1, &style).unwrap();
        assert!(!base.unchanged(&model.workbook.styles).unwrap());
        file.cell_xfs += 2;
        assert!(
            StylesBase::opened(file, &model.workbook.styles)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_part_that_is_not_appendable_takes_no_new_styles() {
        let (mut model, _) = written();
        let mut file = model_counts(&model.workbook.styles);
        file.shape = StylesShape::Opaque;
        let base = StylesBase::opened(file, &model.workbook.styles)
            .unwrap()
            .unwrap();
        let mut style = Style::default();
        style.font.b = true;
        model.set_cell_style(0, 1, 1, &style).unwrap();
        assert!(!base.unchanged(&model.workbook.styles).unwrap());
    }
}
