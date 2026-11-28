use ironcalc_base::expressions::utils::number_to_column;
use ironcalc_base::types::{AutoFilter, FilterCriteria};

use super::escape::escape_xml;

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
