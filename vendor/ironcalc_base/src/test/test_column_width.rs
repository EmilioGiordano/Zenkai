#![allow(clippy::unwrap_used)]

use crate::constants::{COLUMN_WIDTH_FACTOR, DEFAULT_COLUMN_WIDTH};
use crate::model::Model;
use crate::test::util::new_empty_model;
use crate::types::Col;
use crate::UserModel;

#[test]
fn test_column_width() {
    let mut model = new_empty_model();
    let cols = vec![Col {
        custom_width: false,
        max: 16384,
        min: 1,
        style: Some(6),
        width: 8.7,
        hidden: false,
    }];
    model.workbook.worksheets[0].cols = cols;
    model
        .workbook
        .worksheet_mut(0)
        .unwrap()
        .set_column_width(2, 30.0)
        .unwrap();
    assert_eq!(model.workbook.worksheets[0].cols.len(), 3);
    let worksheet = model.workbook.worksheet(0).unwrap();
    assert!((worksheet.get_column_width(1).unwrap() - DEFAULT_COLUMN_WIDTH).abs() < f64::EPSILON);
    assert!((worksheet.get_column_width(2).unwrap() - 30.0).abs() < f64::EPSILON);
    assert!((worksheet.get_column_width(3).unwrap() - DEFAULT_COLUMN_WIDTH).abs() < f64::EPSILON);
    assert_eq!(model.get_cell_style_index(0, 23, 2), Ok(6));
}

#[test]
fn test_column_width_lower_edge() {
    let mut model = new_empty_model();
    let cols = vec![Col {
        custom_width: true,
        max: 16,
        min: 5,
        style: Some(1),
        width: 10.0,
        hidden: false,
    }];
    model.workbook.worksheets[0].cols = cols;
    model
        .workbook
        .worksheet_mut(0)
        .unwrap()
        .set_column_width(5, 30.0)
        .unwrap();
    assert_eq!(model.workbook.worksheets[0].cols.len(), 2);
    let worksheet = model.workbook.worksheet(0).unwrap();
    assert!((worksheet.get_column_width(4).unwrap() - DEFAULT_COLUMN_WIDTH).abs() < f64::EPSILON);
    assert!((worksheet.get_column_width(5).unwrap() - 30.0).abs() < f64::EPSILON);
    assert!(
        (worksheet.get_column_width(6).unwrap() - 10.0 * COLUMN_WIDTH_FACTOR).abs() < f64::EPSILON
    );
    assert_eq!(model.get_cell_style_index(0, 23, 5), Ok(1));
}

#[test]
fn test_column_width_higher_edge() {
    let mut model = new_empty_model();
    let cols = vec![Col {
        custom_width: true,
        max: 16,
        min: 5,
        style: Some(1),
        width: 10.0,
        hidden: false,
    }];
    model.workbook.worksheets[0].cols = cols;
    model
        .workbook
        .worksheet_mut(0)
        .unwrap()
        .set_column_width(16, 30.0)
        .unwrap();
    assert_eq!(model.workbook.worksheets[0].cols.len(), 2);
    let worksheet = model.workbook.worksheet(0).unwrap();
    assert!(
        (worksheet.get_column_width(15).unwrap() - 10.0 * COLUMN_WIDTH_FACTOR).abs() < f64::EPSILON
    );
    assert!((worksheet.get_column_width(16).unwrap() - 30.0).abs() < f64::EPSILON);
    assert!((worksheet.get_column_width(17).unwrap() - DEFAULT_COLUMN_WIDTH).abs() < f64::EPSILON);
    assert_eq!(model.get_cell_style_index(0, 23, 16), Ok(1));
}

#[test]
fn test_column_width_negative() {
    let mut model = new_empty_model();
    let result = model
        .workbook
        .worksheet_mut(0)
        .unwrap()
        .set_column_width(16, -1.0);
    assert_eq!(result, Err("Can not set a negative width: -1".to_string()));
    assert_eq!(model.workbook.worksheets[0].cols.len(), 0);
    let worksheet = model.workbook.worksheet(0).unwrap();
    assert_eq!(
        (worksheet.get_column_width(16).unwrap()),
        DEFAULT_COLUMN_WIDTH
    );
    assert_eq!(model.get_cell_style_index(0, 23, 16), Ok(0));
}

fn width_layouts() -> Vec<Vec<Col>> {
    let col = |min, max, width| Col {
        min,
        max,
        width,
        custom_width: true,
        style: None,
        hidden: false,
    };
    let mut layouts = Vec::new();
    for a in 1..=6 {
        for b in a..=6 {
            layouts.push(vec![col(a, b, 10.0)]);
            for c in b + 1..=6 {
                for d in c..=6 {
                    layouts.push(vec![col(a, b, 10.0), col(c, d, 20.0)]);
                }
            }
        }
    }
    layouts
}

fn widths(model: &Model) -> Vec<f64> {
    let worksheet = model.workbook.worksheet(0).unwrap();
    (1..=12)
        .map(|column| worksheet.get_column_width(column).unwrap())
        .collect()
}

fn assert_ranges_valid(model: &Model, case: &str) {
    let cols = &model.workbook.worksheet(0).unwrap().cols;
    for col in cols {
        assert!(col.min <= col.max, "{case}: {cols:?}");
    }
    for pair in cols.windows(2) {
        assert!(pair[0].max < pair[1].min, "{case}: {cols:?}");
    }
}

#[test]
fn test_delete_columns_keeps_width_ranges_valid() {
    let mut model = new_empty_model();
    for layout in width_layouts() {
        for start in 1..=7 {
            for count in 1..=3 {
                model.workbook.worksheet_mut(0).unwrap().cols = layout.clone();
                let before = widths(&model);
                model.delete_columns(0, start, count).unwrap();
                let case = format!("{layout:?} delete {start} x {count}");
                assert_ranges_valid(&model, &case);
                let start = start as usize - 1;
                let expected: Vec<f64> = (0..12)
                    .map(|i| {
                        let source = if i < start { i } else { i + count as usize };
                        before.get(source).copied().unwrap_or(DEFAULT_COLUMN_WIDTH)
                    })
                    .collect();
                assert_eq!(widths(&model), expected, "{case}");
            }
        }
    }
}

#[test]
fn test_insert_columns_keeps_width_ranges_valid() {
    let mut model = new_empty_model();
    for layout in width_layouts() {
        for start in 1..=7 {
            for count in 1..=3 {
                model.workbook.worksheet_mut(0).unwrap().cols = layout.clone();
                let before = widths(&model);
                model.insert_columns(0, start, count).unwrap();
                let case = format!("{layout:?} insert {start} x {count}");
                assert_ranges_valid(&model, &case);
                let after = widths(&model);
                let (start, count) = (start as usize - 1, count as usize);
                assert_eq!(after[..start], before[..start], "{case}");
                assert_eq!(
                    after[start + count..],
                    before[..12 - count][start..],
                    "{case}"
                );
            }
        }
    }
}

#[test]
fn test_delete_column_with_its_own_width_undo_redo() {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.set_columns_width(0, 5, 5, 40.0).unwrap();
    model.delete_columns(0, 5, 1).unwrap();
    assert!(model
        .get_model()
        .workbook
        .worksheet(0)
        .unwrap()
        .cols
        .is_empty());
    assert_eq!(model.get_column_width(0, 5).unwrap(), DEFAULT_COLUMN_WIDTH);
    model.undo().unwrap();
    assert_eq!(model.get_column_width(0, 5).unwrap(), 40.0);
    model.redo().unwrap();
    assert!(model
        .get_model()
        .workbook
        .worksheet(0)
        .unwrap()
        .cols
        .is_empty());
}
