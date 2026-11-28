use ironcalc_base::expressions::utils::{number_to_column, quote_name};
use ironcalc_base::types::{AutoFilter, FilterCriteria, Worksheet};

use super::escape::escape_xml;

/// The hidden, sheet-scoped name in which Excel keeps the range of a sheet's AutoFilter.
pub(crate) const FILTER_DATABASE: &str = "_xlnm._FilterDatabase";

pub(crate) fn is_filter_database(name: &str) -> bool {
    name.eq_ignore_ascii_case(FILTER_DATABASE)
}

/// A `_xlnm._FilterDatabase` defined name for each sheet with an AutoFilter, written from
/// the filter so it always names the filter's current range.
pub(crate) fn get_filter_database_xml(worksheets: &[Worksheet]) -> String {
    let mut xml = String::new();
    for (index, worksheet) in worksheets.iter().enumerate() {
        let Some(filter) = &worksheet.auto_filter else {
            continue;
        };
        let (Some(first), Some(last)) = (
            number_to_column(filter.first_column),
            number_to_column(filter.last_column),
        ) else {
            continue;
        };
        let reference = format!(
            "{}!${first}${}:${last}${}",
            quote_name(&worksheet.name),
            filter.first_row,
            filter.last_row
        );
        xml.push_str(&format!(
            "<definedName name=\"{FILTER_DATABASE}\" localSheetId=\"{index}\" hidden=\"1\">{}</definedName>",
            escape_xml(&reference)
        ));
    }
    xml
}

fn area(first_row: i32, first_column: i32, last_row: i32, last_column: i32) -> Option<String> {
    Some(format!(
        "{}{first_row}:{}{last_row}",
        number_to_column(first_column)?,
        number_to_column(last_column)?
    ))
}

/// The sheet's `<autoFilter>` element, which goes right after `</sheetData>`; empty when
/// the sheet has none.
pub(crate) fn get_auto_filter_xml(filter: Option<&AutoFilter>) -> String {
    let Some(filter) = filter else {
        return String::new();
    };
    let Some(reference) = area(
        filter.first_row,
        filter.first_column,
        filter.last_row,
        filter.last_column,
    ) else {
        return String::new();
    };
    let mut inner = String::new();
    for column in &filter.columns {
        let hidden_button = if column.hidden_button {
            " hiddenButton=\"1\""
        } else {
            ""
        };
        let show_button = if column.show_button {
            ""
        } else {
            " showButton=\"0\""
        };
        let criteria = match &column.criteria {
            FilterCriteria::None => String::new(),
            FilterCriteria::Values { values, blank } => {
                let blank = if *blank { " blank=\"1\"" } else { "" };
                let values: String = values
                    .iter()
                    .map(|value| format!("<filter val=\"{}\"/>", escape_xml(value)))
                    .collect();
                format!("<filters{blank}>{values}</filters>")
            }
            FilterCriteria::Kept(xml) => xml.clone(),
        };
        let attributes = format!("colId=\"{}\"{hidden_button}{show_button}", column.column);
        if criteria.is_empty() {
            inner.push_str(&format!("<filterColumn {attributes}/>"));
        } else {
            inner.push_str(&format!(
                "<filterColumn {attributes}>{criteria}</filterColumn>"
            ));
        }
    }
    if let Some(state) = &filter.sort_state {
        let conditions: Option<String> = state
            .conditions
            .iter()
            .map(|condition| {
                let reference = area(
                    condition.first_row,
                    condition.first_column,
                    condition.last_row,
                    condition.last_column,
                )?;
                let descending = if condition.descending {
                    " descending=\"1\""
                } else {
                    ""
                };
                Some(format!(
                    "<sortCondition{descending}{} ref=\"{reference}\"/>",
                    condition.attributes
                ))
            })
            .collect();
        let state_reference = area(
            state.first_row,
            state.first_column,
            state.last_row,
            state.last_column,
        );
        if let (Some(conditions), Some(state_reference)) = (conditions, state_reference) {
            inner.push_str(&format!(
                "<sortState ref=\"{state_reference}\"{}>{conditions}</sortState>",
                state.attributes
            ));
        }
    }
    if inner.is_empty() {
        format!("<autoFilter ref=\"{reference}\"/>")
    } else {
        format!("<autoFilter ref=\"{reference}\">{inner}</autoFilter>")
    }
}
