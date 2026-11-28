use ironcalc_base::expressions::utils::parse_reference_a1;
use ironcalc_base::types::{AutoFilter, FilterColumn, FilterCriteria, SortCondition, SortState};
use roxmltree::Node;

use super::shared_strings::decode_xlsx_escapes;
use super::util::{get_bool, get_bool_false};

const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
// A column cannot show more values than a sheet has rows.
const MAX_VALUES: usize = 1_048_576;
// Excel's longest cell text; a value longer than any cell matches no cell.
const MAX_VALUE_CHARS: usize = 32_767;
// A rule kept as written is a handful of small elements; a larger one is not kept.
const MAX_KEPT_BYTES: usize = 64 * 1024;
// Excel sorts by at most 64 levels.
const MAX_SORT_CONDITIONS: usize = 64;

// 18.3.1.2 autoFilter
// <autoFilter ref="A1:C10">
//   <filterColumn colId="1">
//     <filters blank="1"><filter val="North"/></filters>
//   </filterColumn>
//   <sortState ref="A2:C10"><sortCondition descending="1" ref="B2:B10"/></sortState>
// </autoFilter>
// A column whose rule cannot be kept is left out; the rows the file hides stay hidden.
pub(super) fn load_auto_filter(ws: Node) -> Option<AutoFilter> {
    let node = ws
        .children()
        .find(|n| n.is_element() && n.has_tag_name("autoFilter"))?;
    let (first_row, first_column, last_row, last_column) = parse_area(node.attribute("ref")?)?;
    let width = last_column - first_column + 1;
    let mut columns: Vec<FilterColumn> = Vec::new();
    for column in node
        .children()
        .filter(|n| n.is_element() && n.has_tag_name("filterColumn"))
    {
        let Some(id) = column
            .attribute("colId")
            .and_then(|id| id.trim().parse::<i32>().ok())
            .filter(|id| (0..width).contains(id))
        else {
            continue;
        };
        if columns.iter().any(|known| known.column == id) {
            continue;
        }
        let Some(criteria) = load_criteria(column) else {
            continue;
        };
        columns.push(FilterColumn {
            column: id,
            criteria,
            hidden_button: get_bool_false(column, "hiddenButton"),
            show_button: get_bool(column, "showButton"),
        });
    }
    columns.sort_by_key(|column| column.column);
    let sort_state = node
        .children()
        .find(|n| n.is_element() && n.has_tag_name("sortState"))
        .and_then(load_sort_state);
    let filter = AutoFilter {
        first_row,
        first_column,
        last_row,
        last_column,
        columns,
        filtered_rows: Vec::new(),
        sort_state,
    };
    filter.is_valid().then_some(filter)
}

fn parse_area(reference: &str) -> Option<(i32, i32, i32, i32)> {
    let (start, end) = reference.split_once(':').unwrap_or((reference, reference));
    let start = parse_reference_a1(start.trim())?;
    let end = parse_reference_a1(end.trim())?;
    Some((
        start.row.min(end.row),
        start.column.min(end.column),
        start.row.max(end.row),
        start.column.max(end.column),
    ))
}

fn elements<'a, 'input>(node: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children()
        .filter(|n| n.is_element() && !n.has_tag_name("extLst"))
}

fn load_criteria(column: Node) -> Option<FilterCriteria> {
    let rules: Vec<Node> = elements(column).collect();
    match rules.as_slice() {
        [] => Some(FilterCriteria::None),
        [filters] if is_value_list(*filters) => {
            let values = elements(*filters)
                .filter_map(|filter| filter.attribute("val"))
                .filter(|value| value.chars().count() <= MAX_VALUE_CHARS)
                .take(MAX_VALUES)
                .map(decode_xlsx_escapes)
                .collect();
            Some(FilterCriteria::Values {
                values,
                blank: get_bool_false(*filters, "blank"),
            })
        }
        _ => kept_rule(&rules).map(FilterCriteria::Kept),
    }
}

// Only `<filters>` holding `<filter>` values, without a calendar (which only date groups
// use), is a plain list of values.
fn is_value_list(filters: Node) -> bool {
    filters.tag_name().namespace() == Some(MAIN)
        && filters.has_tag_name("filters")
        && filters.attributes().all(|a| a.name() == "blank")
        && elements(filters)
            .all(|n| n.tag_name().namespace() == Some(MAIN) && n.has_tag_name("filter"))
}

fn escape_markup(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
}

// The attributes of no namespace, escaped, each after a space; the others, such as
// revision ids, mean nothing once the element is written elsewhere.
fn plain_attributes(node: Node, skipped: &[&str], out: &mut String) {
    for attribute in node
        .attributes()
        .filter(|a| a.namespace().is_none() && !skipped.contains(&a.name()))
    {
        out.push(' ');
        out.push_str(attribute.name());
        out.push_str("=\"");
        escape_markup(attribute.value(), out);
        out.push('"');
    }
}

enum Visit<'a, 'input> {
    Open(Node<'a, 'input>),
    Close(Node<'a, 'input>),
}

// The rule as it reads, rebuilt from the parsed elements, never copied as text. An
// element of another namespace would not mean the same in the file written, so the rule
// is not kept; extension lists are left out. Walks with its own stack, however deep.
fn kept_rule(rules: &[Node]) -> Option<String> {
    let mut xml = String::new();
    let mut pending: Vec<Visit> = rules.iter().rev().map(|n| Visit::Open(*n)).collect();
    while let Some(visit) = pending.pop() {
        match visit {
            Visit::Open(n) if n.is_text() => escape_markup(n.text().unwrap_or_default(), &mut xml),
            Visit::Open(n) if n.is_element() => {
                // No rule holds a filter, and a writer may look for the filter's end tag.
                if n.tag_name().namespace() != Some(MAIN) || n.has_tag_name("autoFilter") {
                    return None;
                }
                if n.has_tag_name("extLst") {
                    continue;
                }
                xml.push('<');
                xml.push_str(n.tag_name().name());
                plain_attributes(n, &[], &mut xml);
                if n.children().any(|c| c.is_element() || c.is_text()) {
                    xml.push('>');
                    pending.push(Visit::Close(n));
                    let mut child = n.last_child();
                    while let Some(next) = child {
                        pending.push(Visit::Open(next));
                        child = next.prev_sibling();
                    }
                } else {
                    xml.push_str("/>");
                }
            }
            Visit::Open(_) => {}
            Visit::Close(n) => {
                xml.push_str("</");
                xml.push_str(n.tag_name().name());
                xml.push('>');
            }
        }
        if xml.len() > MAX_KEPT_BYTES {
            return None;
        }
    }
    Some(xml)
}

fn load_sort_state(node: Node) -> Option<SortState> {
    let (first_row, first_column, last_row, last_column) = parse_area(node.attribute("ref")?)?;
    let conditions = elements(node)
        .filter(|n| n.has_tag_name("sortCondition"))
        .take(MAX_SORT_CONDITIONS)
        .map(|condition| {
            let (first_row, first_column, last_row, last_column) =
                parse_area(condition.attribute("ref")?)?;
            let mut attributes = String::new();
            plain_attributes(condition, &["ref", "descending"], &mut attributes);
            Some(SortCondition {
                first_row,
                first_column,
                last_row,
                last_column,
                descending: get_bool_false(condition, "descending"),
                attributes,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let mut attributes = String::new();
    plain_attributes(node, &["ref"], &mut attributes);
    Some(SortState {
        first_row,
        first_column,
        last_row,
        last_column,
        conditions,
        attributes,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn filter_of(inner: &str) -> Option<AutoFilter> {
        let xml =
            format!(r#"<worksheet xmlns="{MAIN}" xmlns:x="urn:x"><sheetData/>{inner}</worksheet>"#);
        let document = roxmltree::Document::parse(&xml).unwrap();
        load_auto_filter(document.root_element())
    }

    #[test]
    fn value_lists_are_read_and_other_rules_kept_as_written() {
        let filter = filter_of(concat!(
            r#"<autoFilter ref="B2:D10" x:uid="1">"#,
            r#"<filterColumn colId="0"><filters blank="1"><filter val="a&amp;b"/>"#,
            r#"<filter val="x_x000D_y"/></filters></filterColumn>"#,
            r#"<filterColumn colId="2" hiddenButton="1"><customFilters and="1">"#,
            r#"<customFilter operator="greaterThan" val="&lt;3"/></customFilters></filterColumn>"#,
            r#"<filterColumn colId="1"><filters><dateGroupItem year="2024" "#,
            r#"dateTimeGrouping="year"/></filters></filterColumn>"#,
            r#"<sortState ref="B3:D10" caseSensitive="1"><sortCondition descending="1" "#,
            r#"ref="C3:C10"/></sortState></autoFilter>"#
        ))
        .unwrap();
        assert_eq!(
            (
                filter.first_row,
                filter.first_column,
                filter.last_row,
                filter.last_column
            ),
            (2, 2, 10, 4)
        );
        assert_eq!(
            filter.columns[0].criteria,
            FilterCriteria::Values {
                values: vec!["a&b".to_string(), "x\ry".to_string()],
                blank: true
            }
        );
        assert_eq!(
            filter.columns[1].criteria,
            FilterCriteria::Kept(
                r#"<filters><dateGroupItem year="2024" dateTimeGrouping="year"/></filters>"#
                    .to_string()
            )
        );
        assert_eq!(
            filter.columns[2].criteria,
            FilterCriteria::Kept(
                r#"<customFilters and="1"><customFilter operator="greaterThan" val="&lt;3"/></customFilters>"#
                    .to_string()
            )
        );
        assert!(filter.columns[2].hidden_button);
        let sort = filter.sort_state.unwrap();
        assert_eq!(sort.attributes, r#" caseSensitive="1""#);
        assert!(sort.conditions[0].descending);
        assert_eq!(sort.conditions[0].first_column, 3);
    }

    #[test]
    fn damaged_filters_are_not_read_or_lose_only_what_is_damaged() {
        assert_eq!(filter_of(r#"<autoFilter/>"#), None);
        assert_eq!(filter_of(r#"<autoFilter ref="A0:B2"/>"#), None);
        let filter = filter_of(concat!(
            r#"<autoFilter ref="A1:B2"><filterColumn colId="5"/><filterColumn colId="x"/>"#,
            r#"<filterColumn colId="1"><x:rule/></filterColumn>"#,
            r#"<filterColumn colId="0"/><filterColumn colId="0"><filters/></filterColumn>"#,
            r#"<sortState ref="nowhere"/></autoFilter>"#
        ))
        .unwrap();
        assert_eq!(filter.columns.len(), 1);
        assert_eq!(filter.columns[0].criteria, FilterCriteria::None);
        assert_eq!(filter.sort_state, None);
    }

    #[test]
    fn a_rule_past_the_kept_size_is_not_kept_and_deep_ones_do_not_overflow() {
        let long = format!(
            r#"<autoFilter ref="A1:B2"><filterColumn colId="0"><customFilters><customFilter val="{}"/></customFilters></filterColumn></autoFilter>"#,
            "v".repeat(MAX_KEPT_BYTES)
        );
        assert!(filter_of(&long).unwrap().columns.is_empty());
        let deep = format!(
            r#"<autoFilter ref="A1:B2"><filterColumn colId="0">{}{}</filterColumn></autoFilter>"#,
            "<top10>".repeat(500),
            "</top10>".repeat(500)
        );
        assert_eq!(filter_of(&deep).unwrap().columns.len(), 1);
        let nested = r#"<autoFilter ref="A1:B2"><filterColumn colId="0"><top10><autoFilter/></top10></filterColumn></autoFilter>"#;
        assert!(filter_of(nested).unwrap().columns.is_empty());
    }
}
