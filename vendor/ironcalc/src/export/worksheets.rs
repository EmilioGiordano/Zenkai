#![allow(clippy::unwrap_used)]

//! # A note on shared formulas
//! Although both Excel and IronCalc uses shared formulas they are used in a slightly different way that cannot be mapped 1-1
//! In IronCalc _all_ formulas are shared and there is a list of shared formulas much like there is a list of shared strings.
//! In Excel the situation in more nuanced. A shared formula is shared amongst a rage of cells.
//! The top left cell would be the "mother" cell that would contain the shared formula:
//! ```xml
//! <c r="F4" t="str">
//!    <f t="shared" ref="F4:F8" si="42">A4+C4</f>
//!    <v>123</v>
//! </c>
//! ```
//! Cells in the range F4:F8 will then link to that formula like so:
//! ```xml
//! <c r="F6">
//!   <f t="shared" si="42"/>
//!   <v>1</v>
//! </c>
//! ```
//! Formula in F6 would then be 'A6+C6'
//! A run of cells down a column that hold the same formula is written this way, so the
//! text is stringified once for the run instead of once per cell.
use std::collections::HashMap;
use std::io::{self, Write};

use ironcalc_base::{
    expressions::{
        parser::{static_analysis::StaticResult, stringify::to_excel_string, Node},
        types::CellReferenceRC,
        utils::number_to_column,
    },
    types::{ArrayFormulaCell, ArrayKind, Cell, FormulaValue, SpillValue, Worksheet},
};

use crate::export::conditional_formatting::get_conditional_formatting_xml;

use super::{escape::escape_xml, xml_constants::XML_DECLARATION};

fn get_range_str(row: i32, column: i32, width: i32, height: i32) -> Option<String> {
    let column1 = number_to_column(column)?;
    let column2 = number_to_column(column + width - 1)?;
    let row2 = row + height - 1;
    Some(format!("{}{}:{}{}", column1, row, column2, row2))
}

fn get_cell_style_attribute(s: i32) -> String {
    if s == 0 {
        "".to_string()
    } else {
        format!(" s=\"{s}\"")
    }
}

fn get_formula_attribute(
    sheet_name: String,
    row: i32,
    column: i32,
    parsed_formula: &Node,
) -> String {
    let cell_ref = CellReferenceRC {
        sheet: sheet_name,
        row,
        column,
    };
    let formula = &to_excel_string(parsed_formula, &cell_ref);
    escape_xml(formula).to_string()
}

#[derive(Clone, Copy)]
enum Shared {
    No,
    Anchor { si: i32, last_row: i32 },
    Follower { si: i32 },
}

// Cells down a column that hold the same formula have it shifted by their row, which
// is what a shared formula stands for. The first cell of a run of two or more writes the
// text and the range, the rest only point at it.
fn shared_formula_of(
    worksheet: &Worksheet,
    row: i32,
    column: i32,
    cell: &Cell,
    runs: &mut HashMap<i32, i32>,
    next_si: &mut i32,
) -> Shared {
    let Cell::CellFormula { f, .. } = cell else {
        return Shared::No;
    };
    let formulas = &worksheet.shared_formulas;
    let same_formula = |row: i32| {
        matches!(
            worksheet.sheet_data.get(row, column),
            Some(Cell::CellFormula { f: other, .. })
                if other == f || formulas.get(*other as usize) == formulas.get(*f as usize)
        )
    };
    if row > 1 && same_formula(row - 1) {
        if let Some(si) = runs.get(&column) {
            return Shared::Follower { si: *si };
        }
    }
    let mut last_row = row;
    while last_row < i32::MAX && same_formula(last_row + 1) {
        last_row += 1;
    }
    if last_row == row {
        return Shared::No;
    }
    let si = *next_si;
    *next_si += 1;
    runs.insert(column, si);
    Shared::Anchor { si, last_row }
}

/// Writes the worksheet part row by row, so a large sheet is never held whole in memory
/// as text.
pub fn write_worksheet_xml<W: Write>(
    writer: &mut W,
    worksheet: &Worksheet,
    parsed_formulas: &[(Node, StaticResult)],
    dimension: &str,
    is_sheet_selected: bool,
) -> io::Result<()> {
    let mut cols_str: Vec<String> = vec![];
    let mut merged_cells_str: Vec<String> = vec![];

    for col in &worksheet.cols {
        // <col min="4" max="4" width="12" customWidth="1"/>
        let min = col.min;
        let max = col.max;
        let width = col.width;
        let custom_width = i32::from(col.custom_width);
        let column_style = match col.style {
            Some(s) => format!(" style=\"{s}\""),
            None => "".to_string(),
        };
        let hidden = if col.hidden { " hidden=\"1\"" } else { "" };
        cols_str.push(format!(
            "<col min=\"{min}\" max=\"{max}\" width=\"{width}\" customWidth=\"{custom_width}\"{column_style}{hidden}/>"
        ));
    }

    // this is a bit of an overkill. A dictionary of the row styles by row_index
    let mut row_style_dict = HashMap::new();
    for row in &worksheet.rows {
        // {
        //     "height": 13,
        //     "r": 7,
        //     "custom_format": false,
        //     "custom_height": true,
        //     "s": 0
        //     "hidden": false,
        //   },
        row_style_dict.insert(row.r, row.clone());
    }

    for merge_cell_ref in &worksheet.merge_cells {
        merged_cells_str.push(format!("<mergeCell ref=\"{merge_cell_ref}\"/>"))
    }
    let merged_cells_count = merged_cells_str.len();

    let cols = cols_str.join("");
    let cols = if cols.is_empty() {
        "".to_string()
    } else {
        format!("<cols>{cols}</cols>")
    };

    let tab_selected = if is_sheet_selected {
        " tabSelected=\"1\""
    } else {
        ""
    };

    let show_grid_lines = if !worksheet.show_grid_lines {
        " showGridLines=\"0\""
    } else {
        ""
    };

    let mut active_cell = "A1".to_string();
    let mut sqref = "A1".to_string();

    let views = &worksheet.views;
    if let Some(view) = views.get(&0) {
        let range = view.range;
        let row = view.row;
        let column = view.column;
        let column_name = number_to_column(column).unwrap_or("A".to_string());
        active_cell = format!("{column_name}{row}");

        let column_start = number_to_column(range[1]).unwrap_or("A".to_string());
        let column_end = number_to_column(range[3]).unwrap_or("A".to_string());
        if range[0] == range[2] && range[1] == range[3] {
            sqref = format!("{column_start}{}", range[0]);
        } else {
            sqref = format!("{}{}:{}{}", column_start, range[0], column_end, range[2]);
        }
    }

    let merge_cells_section = if merged_cells_count > 0 {
        format!(
            "<mergeCells count=\"{}\">{}</mergeCells>",
            merged_cells_count,
            merged_cells_str.join("")
        )
    } else {
        "".to_string()
    };

    let frozen_rows = worksheet.frozen_rows;
    let frozen_columns = worksheet.frozen_columns;

    let pane = if frozen_rows > 0 && frozen_columns > 0 {
        // There are both frozen rows and columns. There are four panes.
        // The first column is the first column after the last frozen column.
        let first_column = number_to_column(frozen_columns + 1).unwrap_or("A".to_string());
        // This is the top left cell of the bottom right pane.
        let top_left_cell = format!("{}{}", first_column, frozen_rows + 1);
        // The meaning of the next two is irrelevant for IronCalc.
        let top_right_active_cell = format!("{first_column}1");
        let bottom_left_active_cell = format!("A{}", frozen_rows + 1);
        // The bottom right active cell is the "true" selected cell and it does not need to reside on this pane.
        format!(
            "<pane xSplit=\"{frozen_columns}\" ySplit=\"{frozen_rows}\" topLeftCell=\"{top_left_cell}\" activePane=\"bottomRight\" state=\"frozen\"/>\
             <selection pane=\"topRight\" activeCell=\"{top_right_active_cell}\" sqref=\"{top_right_active_cell}\"/>\
             <selection pane=\"bottomLeft\" activeCell=\"{bottom_left_active_cell}\" sqref=\"{bottom_left_active_cell}\"/>\
             <selection pane=\"bottomRight\" activeCell=\"{active_cell}\" sqref=\"{sqref}\"/>",
        )
    } else if frozen_rows > 0 {
        // Only frozen rows
        let top_left_cell = format!("A{}", frozen_rows + 1);
        format!(
            "<pane ySplit=\"{frozen_rows}\" topLeftCell=\"{top_left_cell}\" activePane=\"bottomLeft\" state=\"frozen\"/>\
            <selection pane=\"bottomLeft\" activeCell=\"{active_cell}\" sqref=\"{sqref}\"/>",
        )
    } else if frozen_columns > 0 {
        let top_left_cell = format!(
            "{}1",
            number_to_column(frozen_columns + 1).unwrap_or("A".to_string())
        );
        format!(
            "<pane xSplit=\"{frozen_columns}\" topLeftCell=\"{top_left_cell}\" activePane=\"topRight\" state=\"frozen\"/>\
             <selection pane=\"topRight\" activeCell=\"{active_cell}\" sqref=\"{sqref}\"/>"
        )
    } else {
        // No frozen rows or columns
        format!(r#"<selection activeCell="{active_cell}" sqref="{sqref}"/>"#)
    };

    let (cf_sections, cf_ext_lst) =
        get_conditional_formatting_xml(&worksheet.conditional_formatting);

    write!(
        writer,
        "{XML_DECLARATION}\
<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
  <dimension ref=\"{dimension}\"/>\
  <sheetViews>\
    <sheetView workbookViewId=\"0\"{show_grid_lines}{tab_selected}>\
        {pane}\
    </sheetView>\
  </sheetViews>\
  {cols}\
  <sheetData>"
    )?;

    let sheet_name = worksheet.get_name();
    let mut shared_runs = HashMap::new();
    let mut next_si = 0;
    for row_index in &worksheet.sheet_data.rows_sorted() {
        let row_style_str = match row_style_dict.get(row_index) {
            Some(row_style) => {
                let hidden_str = if row_style.hidden {
                    r#" hidden="1""#
                } else {
                    ""
                };
                format!(
                    r#" s="{}" ht="{}" customHeight="{}" customFormat="{}"{}"#,
                    row_style.s,
                    row_style.height,
                    i32::from(row_style.custom_height),
                    i32::from(row_style.custom_format),
                    hidden_str,
                )
            }
            None => "".to_string(),
        };
        write!(writer, "<row r=\"{row_index}\"{row_style_str}>")?;
        for (column_index, cell) in worksheet.sheet_data.row_cells_sorted(*row_index) {
            let shared = shared_formula_of(
                worksheet,
                *row_index,
                column_index,
                cell,
                &mut shared_runs,
                &mut next_si,
            );
            write_cell(
                writer,
                &sheet_name,
                *row_index,
                column_index,
                cell,
                parsed_formulas,
                shared,
            )?;
        }
        write!(writer, "</row>")?;
    }

    write!(
        writer,
        "</sheetData>\
  {merge_cells_section}\
  {cf_sections}\
  {cf_ext_lst}\
</worksheet>"
    )
}

fn write_cell<W: Write>(
    writer: &mut W,
    sheet_name: &str,
    row_index: i32,
    column_index: i32,
    cell: &Cell,
    parsed_formulas: &[(Node, StaticResult)],
    shared: Shared,
) -> io::Result<()> {
    let column_name = number_to_column(column_index).unwrap();
    let cell_name = format!("{column_name}{row_index}");
    let formula_of = |f: &i32| {
        get_formula_attribute(
            sheet_name.to_string(),
            row_index,
            column_index,
            &parsed_formulas[*f as usize].0,
        )
    };
    let formula_xml = |f: &i32| match shared {
        Shared::No => format!("<f>{}</f>", formula_of(f)),
        Shared::Anchor { si, last_row } => format!(
            "<f t=\"shared\" ref=\"{cell_name}:{column_name}{last_row}\" si=\"{si}\">{}</f>",
            formula_of(f)
        ),
        Shared::Follower { si } => format!("<f t=\"shared\" si=\"{si}\"/>"),
    };
    // The parts of an array formula: its text, style, spill range and dynamic marker,
    // or None for one this writer skips.
    let array_of = |f: &i32, s: &i32, r: &(i32, i32), kind: &ArrayKind| {
        let node = parsed_formulas.get(*f as usize)?;
        let formula =
            get_formula_attribute(sheet_name.to_string(), row_index, column_index, &node.0);
        let style = get_cell_style_attribute(*s);
        let range = get_range_str(row_index, column_index, r.0, r.1)?;
        let cm = if matches!(kind, ArrayKind::Dynamic) {
            r#" cm="1""#
        } else {
            ""
        };
        Some((formula, style, range, cm))
    };
    match cell {
        Cell::EmptyCell { s } => {
            // they only hold the style
            let style = get_cell_style_attribute(*s);
            write!(writer, "<c r=\"{cell_name}\"{style}/>")
        }
        Cell::BooleanCell { v, s } => {
            // <c r="A8" t="b" s="1">
            //     <v>1</v>
            // </c>
            let b = i32::from(*v);
            let style = get_cell_style_attribute(*s);
            write!(writer, "<c r=\"{cell_name}\" t=\"b\"{style}><v>{b}</v></c>")
        }
        Cell::NumberCell { v, s } => {
            // Normally the type number is left out. Example:
            // <c r="C6" s="1">
            //     <v>3</v>
            // </c>
            let style = get_cell_style_attribute(*s);
            write!(writer, "<c r=\"{cell_name}\"{style}><v>{v}</v></c>")
        }
        Cell::ErrorCell { ei, s } => {
            let style = get_cell_style_attribute(*s);
            write!(
                writer,
                "<c r=\"{cell_name}\" t=\"e\"{style}><v>{ei}</v></c>"
            )
        }
        Cell::SharedString { si, s } => {
            // Example:
            // <c r="A1" s="1" t="s">
            //    <v>5</v>
            // </c>
            // Cell on A1 contains a string (t="s") of style="1". The string is the 6th in the list of shared strings
            let style = get_cell_style_attribute(*s);
            write!(
                writer,
                "<c r=\"{cell_name}\" t=\"s\"{style}><v>{si}</v></c>"
            )
        }
        Cell::CellFormula {
            v: FormulaValue::Unevaluated,
            ..
        } => Err(io::Error::other(
            "Model needs to be evaluated before saving!",
        )),
        Cell::CellFormula {
            f,
            v: FormulaValue::Boolean(v),
            s,
        } => {
            // <c r="A4" t="b" s="3">
            //   <f>ISTEXT(A5)</f>
            //   <v>1</v>
            // </c>
            let style = get_cell_style_attribute(*s);
            let formula = formula_xml(f);
            let b = i32::from(*v);
            write!(
                writer,
                "<c r=\"{cell_name}\" t=\"b\"{style}>{formula}<v>{b}</v></c>"
            )
        }
        Cell::CellFormula {
            f,
            v: FormulaValue::Number(v),
            s,
        } => {
            // Note again type is skipped
            // <c r="C4" s="3">
            //   <f>A5+C3</f>
            //   <v>123</v>
            // </c>
            let formula = formula_xml(f);
            let style = get_cell_style_attribute(*s);
            write!(
                writer,
                "<c r=\"{cell_name}\"{style}>{formula}<v>{v}</v></c>"
            )
        }
        Cell::CellFormula {
            f,
            v: FormulaValue::Text(v),
            s,
        } => {
            // <c r="C6" t="str" s="5">
            //   <f>CONCATENATE(A1, A2)</f>
            //   <v>Hello world!</v>
            // </c>
            let formula = formula_xml(f);
            let style = get_cell_style_attribute(*s);
            let escaped_v = escape_xml(v);
            write!(
                writer,
                "<c r=\"{cell_name}\" t=\"str\"{style}>{formula}<v>{escaped_v}</v></c>"
            )
        }
        Cell::CellFormula {
            f,
            v: FormulaValue::Error(error),
            s,
        } => {
            let ei = &error.ei;
            // <c r="C6" t="e" s="4">
            //   <f>A1/A3<f/>
            //   <v>#DIV/0!</v>
            // </c>
            let formula = formula_xml(f);
            let style = get_cell_style_attribute(*s);
            write!(
                writer,
                "<c r=\"{cell_name}\" t=\"e\"{style}>{formula}<v>{ei}</v></c>"
            )
        }
        Cell::SpillCell(spill) => {
            let style = get_cell_style_attribute(spill.s);
            match &spill.v {
                SpillValue::Boolean(v) => {
                    let b = i32::from(*v);
                    write!(writer, "<c r=\"{cell_name}\" t=\"b\"{style}><v>{b}</v></c>")
                }
                SpillValue::Number(v) => {
                    write!(writer, "<c r=\"{cell_name}\"{style}><v>{v}</v></c>")
                }
                SpillValue::Error(ei) => write!(
                    writer,
                    "<c r=\"{cell_name}\" t=\"e\"{style}><v>{ei}</v></c>"
                ),
                SpillValue::Text(v) => {
                    // inline string
                    // <c r="C6" t="str" s="5">
                    //   <v>Hello world!</v>
                    // </c>
                    let escaped_v = escape_xml(v);
                    write!(
                        writer,
                        "<c r=\"{cell_name}\" t=\"str\"{style}><v>{escaped_v}</v></c>"
                    )
                }
            }
        }
        Cell::ArrayFormula(array) => {
            let ArrayFormulaCell { f, s, r, kind, v } = &**array;
            match v {
                FormulaValue::Unevaluated => Err(io::Error::other(
                    "Model needs to be evaluated before saving!",
                )),
                // The difference between dynamic and array formulas is that dynamic formulas have cm="1"
                FormulaValue::Boolean(v) => {
                    // Dynamic: <c r="D1" s="3" t="b" cm="1">
                    //            <f t="array" ref="D1:D5">A1:A5</f>
                    //            <v>TRUE</v>
                    //          </c>
                    // CSE:     <c r="D1" s="3" t="b">
                    //            <f t="array" ref="D1:D5">A1:A5</f>
                    //            <v>TRUE</v>
                    //          </c>
                    let Some((formula, style, range, cm)) = array_of(f, s, r, kind) else {
                        return Ok(());
                    };
                    let b = i32::from(*v);
                    write!(
                        writer,
                        r#"<c r="{cell_name}"{style} t="b"{cm}><f t="array" ref="{range}">{formula}</f><v>{b}</v></c>"#
                    )
                }
                FormulaValue::Number(v) => {
                    // Dynamic: <c r="D1" s="3" cm="1">
                    //            <f t="array" ref="D1:D5">A1:A5</f>
                    //            <v>123</v>
                    //          </c>
                    // CSE:     <c r="D1" s="3">
                    //            <f t="array" ref="D1:D5">A1:A5</f>
                    //            <v>123</v>
                    //          </c>
                    let Some((formula, style, range, cm)) = array_of(f, s, r, kind) else {
                        return Ok(());
                    };
                    write!(
                        writer,
                        r#"<c r="{cell_name}"{style}{cm}><f t="array" ref="{range}">{formula}</f><v>{v}</v></c>"#
                    )
                }
                FormulaValue::Text(v) => {
                    // Dynamic: <c r="D1" s="3" t="str" cm="1">
                    //            <f t="array" ref="D1:D5">A1:A5</f>
                    //            <v>Hello world!</v>
                    //          </c>
                    // CSE:     <c r="D1" s="3" t="str">
                    //            <f t="array" ref="D1:D5">A1:A5</f>
                    //            <v>Hello world!</v>
                    //          </c>
                    let Some((formula, style, range, cm)) = array_of(f, s, r, kind) else {
                        return Ok(());
                    };
                    let escaped_v = escape_xml(v);
                    write!(
                        writer,
                        r#"<c r="{cell_name}"{style} t="str"{cm}><f t="array" ref="{range}">{formula}</f><v>{escaped_v}</v></c>"#
                    )
                }
                FormulaValue::Error(error) => {
                    // Dynamic: <c r="D1" s="3" t="e" cm="1">
                    //            <f t="array" ref="D1:D5">A1:A5</f>
                    //            <v>#N/A!</v>
                    //          </c>
                    // CSE:     <c r="D1" s="3" t="e">
                    //            <f t="array" ref="D1:D5">A1:A5</f>
                    //            <v>#N/A!</v>
                    //          </c>
                    let Some((formula, style, range, cm)) = array_of(f, s, r, kind) else {
                        return Ok(());
                    };
                    let ei = &error.ei;
                    write!(
                        writer,
                        r#"<c r="{cell_name}"{style} t="e"{cm}><f t="array" ref="{range}">{formula}</f><v>{ei}</v></c>"#
                    )
                }
            }
        }
    }
}
