use ironcalc::base::expressions::token::{Error, get_error_by_english_name};
use ironcalc::base::types::{ArrayKind, Cell, FormulaValue, SpillValue};

use super::{CellArray, Scanner, Slot};
use crate::text::decode_xlsx_escapes;

pub(super) struct ValueCell<'a> {
    pub(super) cell_type: &'a str,
    pub(super) value: Option<&'a str>,
    pub(super) metadata: Option<&'a str>,
    pub(super) style: i32,
    pub(super) anchor: Option<(i32, i32)>,
    pub(super) inline: Option<String>,
    pub(super) place: Slot,
}

impl Scanner<'_, '_> {
    pub(super) fn formula_value(
        &self,
        cell_type: &str,
        value: Option<&str>,
        metadata: Option<&str>,
        cell_ref: &str,
        inline: Option<String>,
    ) -> FormulaValue {
        let origin = || format!("{}!{cell_ref}", self.sheet_name);
        match cell_type {
            "b" => FormulaValue::Boolean(value == Some("1")),
            "n" => FormulaValue::Number(value.unwrap_or("0").parse::<f64>().unwrap_or(0.0)),
            "e" => {
                let name = error_name(value, metadata);
                FormulaValue::Error {
                    ei: get_error_by_english_name(name).unwrap_or(Error::ERROR),
                    o: origin(),
                    m: value.unwrap_or("#ERROR!").to_string(),
                }
            }
            "s" | "d" => FormulaValue::Error {
                ei: Error::NIMPL,
                o: origin(),
                m: Error::NIMPL.to_string(),
            },
            "str" => FormulaValue::Text(decode_xlsx_escapes(value.unwrap_or(""))),
            "inlineStr" => FormulaValue::Text(inline.unwrap_or_default()),
            _ => FormulaValue::Error {
                ei: Error::ERROR,
                o: origin(),
                m: Error::ERROR.to_string(),
            },
        }
    }

    pub(super) fn value_cell(&mut self, cell: ValueCell<'_>) -> Cell {
        let ValueCell {
            cell_type,
            value,
            metadata,
            style,
            anchor,
            inline,
            place,
        } = cell;
        let s = style;
        match (cell_type, anchor) {
            ("b", Some(a)) => Cell::SpillCell {
                v: SpillValue::Boolean(value == Some("1")),
                s,
                a,
            },
            ("b", None) => Cell::BooleanCell {
                v: value == Some("1"),
                s,
            },
            ("n", Some(a)) => Cell::SpillCell {
                v: SpillValue::Number(number(value)),
                s,
                a,
            },
            ("n", None) => Cell::NumberCell {
                v: number(value),
                s,
            },
            ("e", anchor) => {
                let ei =
                    get_error_by_english_name(error_name(value, metadata)).unwrap_or(Error::ERROR);
                match anchor {
                    Some(a) => Cell::SpillCell {
                        v: SpillValue::Error(ei),
                        s,
                        a,
                    },
                    None => Cell::ErrorCell { ei, s },
                }
            }
            ("s", _) => Cell::SharedString {
                si: value.unwrap_or("0").parse::<i32>().unwrap_or(0),
                s,
            },
            // IronCalc adds the text to the shared strings even when the cell is a spill.
            ("str", anchor) => {
                let text = decode_xlsx_escapes(value.unwrap_or(""));
                match anchor {
                    Some(a) => {
                        self.intern(text.clone());
                        Cell::SpillCell {
                            v: SpillValue::Text(text),
                            s,
                            a,
                        }
                    }
                    None => self.string_cell(text, s, place),
                }
            }
            ("d", _) => Cell::ErrorCell {
                ei: Error::NIMPL,
                s,
            },
            ("inlineStr", _) => self.string_cell(inline.unwrap_or_default(), s, place),
            ("empty", _) => Cell::EmptyCell { s },
            _ => Cell::ErrorCell {
                ei: Error::ERROR,
                s,
            },
        }
    }
}

pub(super) fn formula_cell(f: i32, s: i32, array: CellArray, v: FormulaValue) -> Cell {
    match array {
        CellArray::None => Cell::CellFormula { f, s, v },
        CellArray::Dynamic(width, height) => Cell::ArrayFormula {
            f,
            s,
            r: (width, height),
            kind: ArrayKind::Dynamic,
            v,
        },
        CellArray::Cse(width, height) => Cell::ArrayFormula {
            f,
            s,
            r: (width, height),
            kind: ArrayKind::Cse,
            v,
        },
    }
}

pub(super) fn number(value: Option<&str>) -> f64 {
    value.unwrap_or("0").parse::<f64>().unwrap_or(0.0)
}

// Excel stores #SPILL! and #CALC! as #VALUE! plus value metadata, for older readers.
pub(super) fn error_name<'v>(value: Option<&'v str>, metadata: Option<&str>) -> &'v str {
    let name = value.unwrap_or("#ERROR!");
    match (name, metadata) {
        ("#VALUE!", Some("1")) => "#CALC!",
        ("#VALUE!", Some("2")) => "#SPILL!",
        _ => name,
    }
}
