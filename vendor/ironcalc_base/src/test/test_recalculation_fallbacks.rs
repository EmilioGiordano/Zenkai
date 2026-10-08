#![allow(clippy::unwrap_used)]

use crate::cf_types::{CfRuleInput, Cfvo, ColorScaleThreshold};
use crate::types::Color;
use crate::user_model::recalculation::PendingRecalculation;
use crate::UserModel;

fn color_scale() -> CfRuleInput {
    CfRuleInput::ColorScale {
        thresholds: vec![
            ColorScaleThreshold {
                cfvo: Cfvo::Min,
                color: Color::Rgb("#FF0000".to_string()),
            },
            ColorScaleThreshold {
                cfvo: Cfvo::Max,
                color: Color::Rgb("#00FF00".to_string()),
            },
        ],
    }
}

fn evaluated_model() -> UserModel<'static> {
    let mut model = UserModel::new_empty("model", "en", "UTC", "en").unwrap();
    model.set_user_input(0, 1, 1, "1").unwrap();
    model
        .set_user_input(0, 1, 2, "=SUBTOTAL(109,A1:A5)")
        .unwrap();
    model.new_sheet().unwrap();
    model
        .new_defined_name("total", None, "Sheet1!$A$1")
        .unwrap();
    model
        .add_conditional_formatting(0, "A1:A5", color_scale())
        .unwrap();
    model
        .add_conditional_formatting(0, "B1:B5", color_scale())
        .unwrap();
    model.evaluate();
    model.pause_evaluation();
    model
}

// With evaluation paused, what the change leaves for the next evaluation is observable.
fn recalculates_everything(change: impl FnOnce(&mut UserModel)) -> bool {
    let mut model = evaluated_model();
    change(&mut model);
    matches!(model.pending_recalculation, PendingRecalculation::Workbook)
}

#[test]
fn content_and_presentation_changes_stay_incremental() {
    assert!(!recalculates_everything(|m| m
        .set_user_input(0, 2, 1, "5")
        .unwrap()));
    assert!(!recalculates_everything(|m| m
        .set_columns_width(0, 1, 1, 120.0)
        .unwrap()));
    assert!(!recalculates_everything(|m| m
        .set_rows_height(0, 1, 1, 40.0)
        .unwrap()));
    assert!(!recalculates_everything(|m| m
        .set_frozen_rows_count(0, 1)
        .unwrap()));
}

#[test]
fn hidden_rows_and_columns() {
    assert!(recalculates_everything(|m| m
        .set_rows_hidden(0, 2, 3, true)
        .unwrap()));
    assert!(recalculates_everything(|m| m
        .set_columns_hidden(0, 3, 3, true)
        .unwrap()));
}

#[test]
fn inserted_deleted_and_moved_rows_and_columns() {
    assert!(recalculates_everything(|m| m.insert_rows(0, 1, 1).unwrap()));
    assert!(recalculates_everything(|m| m.delete_rows(0, 3, 1).unwrap()));
    assert!(recalculates_everything(|m| m
        .insert_columns(0, 1, 1)
        .unwrap()));
    assert!(recalculates_everything(|m| m
        .delete_columns(0, 3, 1)
        .unwrap()));
    assert!(recalculates_everything(|m| m
        .move_rows_action(0, 1, 1, 1)
        .unwrap()));
    assert!(recalculates_everything(|m| m
        .move_columns_action(0, 1, 1, 1)
        .unwrap()));
}

#[test]
fn sheets() {
    assert!(recalculates_everything(|m| m.new_sheet().unwrap()));
    assert!(recalculates_everything(|m| m.duplicate_sheet(0).unwrap()));
    assert!(recalculates_everything(|m| m.delete_sheet(1).unwrap()));
    assert!(recalculates_everything(|m| m
        .rename_sheet(1, "Data")
        .unwrap()));
    assert!(recalculates_everything(|m| m.move_sheet(1, 0).unwrap()));
    assert!(recalculates_everything(|m| m.hide_sheet(1).unwrap()));
}

#[test]
fn defined_names() {
    assert!(recalculates_everything(|m| m
        .new_defined_name("other", None, "Sheet1!$A$2")
        .unwrap()));
    assert!(recalculates_everything(|m| m
        .update_defined_name("total", None, "total", None, "Sheet1!$A$3")
        .unwrap()));
    assert!(recalculates_everything(|m| m
        .delete_defined_name("total", None)
        .unwrap()));
}

#[test]
fn array_formulas() {
    assert!(recalculates_everything(|m| m
        .set_user_array_formula(0, 4, 4, 1, 2, "=A1:A2")
        .unwrap()));
}

#[test]
fn locale_and_timezone() {
    assert!(recalculates_everything(|m| m.set_locale("de").unwrap()));
    assert!(recalculates_everything(|m| m
        .set_timezone("Europe/Berlin")
        .unwrap()));
}

#[test]
fn conditional_formatting_rules() {
    assert!(recalculates_everything(|m| m
        .add_conditional_formatting(0, "C1:C5", color_scale())
        .unwrap()));
    assert!(recalculates_everything(|m| m
        .delete_conditional_formatting(0, 0)
        .unwrap()));
    assert!(recalculates_everything(|m| m
        .update_conditional_formatting(0, 0, "A1:A9", color_scale())
        .unwrap()));
    assert!(recalculates_everything(|m| m
        .raise_conditional_formatting_priority(0, 0)
        .unwrap()));
}

#[test]
fn undoing_a_structural_change() {
    let mut model = evaluated_model();
    model.resume_evaluation();
    model.insert_rows(0, 1, 1).unwrap();
    model.pause_evaluation();
    model.undo().unwrap();
    assert!(matches!(
        model.pending_recalculation,
        PendingRecalculation::Workbook
    ));
}
