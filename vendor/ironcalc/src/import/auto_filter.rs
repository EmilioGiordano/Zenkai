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
// Excel sorts by at most 64 levels.
const MAX_SORT_CONDITIONS: usize = 64;

// 18.3.1.2 autoFilter
// <autoFilter ref="A1:C10">
//   <filterColumn colId="1">
//     <filters blank="1"><filter val="North"/></filters>
//   </filterColumn>
//   <sortState ref="A2:C10"><sortCondition descending="1" ref="B2:B10"/></sortState>
// </autoFilter>
// A rule other than a list of values is kept as written; the rows the file hides stay
// hidden.
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
        columns.push(FilterColumn {
            column: id,
            criteria: load_criteria(column),
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

// Extension lists count as part of a rule, so a rule holding one is kept whole.
fn rule_elements<'a, 'input>(node: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(|n| n.is_element())
}

fn load_criteria(column: Node) -> FilterCriteria {
    let rules: Vec<Node> = rule_elements(column).collect();
    match rules.as_slice() {
        [] => FilterCriteria::None,
        [filters] if is_value_list(*filters) => {
            let values = rule_elements(*filters)
                .filter_map(|filter| filter.attribute("val"))
                .filter(|value| value.chars().count() <= MAX_VALUE_CHARS)
                .take(MAX_VALUES)
                .map(decode_xlsx_escapes)
                .collect();
            FilterCriteria::Values {
                values,
                blank: get_bool_false(*filters, "blank"),
            }
        }
        _ => FilterCriteria::Kept(kept_rule(&rules)),
    }
}

// Only `<filters>` holding `<filter>` values, without a calendar (which only date groups
// use), is a plain list of values.
fn is_value_list(filters: Node) -> bool {
    filters.tag_name().namespace() == Some(MAIN)
        && filters.has_tag_name("filters")
        && filters.attributes().all(|a| a.name() == "blank")
        && rule_elements(filters)
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
    // The element or text, and the default namespace where it is written.
    Open(Node<'a, 'input>, Option<&'a str>),
    Close(Node<'a, 'input>),
}

const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

// Each attribute after a space; one in a namespace gets a prefix declared on the element.
fn attributes_with_namespaces(node: Node, out: &mut String) {
    let mut prefixes: Vec<&str> = Vec::new();
    for attribute in node.attributes() {
        out.push(' ');
        match attribute.namespace() {
            None => {}
            Some(XML_NAMESPACE) => out.push_str("xml:"),
            Some(uri) => {
                let index = match prefixes.iter().position(|known| *known == uri) {
                    Some(index) => index,
                    None => {
                        prefixes.push(uri);
                        let index = prefixes.len() - 1;
                        out.push_str(&format!("xmlns:n{index}=\""));
                        escape_markup(uri, out);
                        out.push_str("\" ");
                        index
                    }
                };
                out.push_str(&format!("n{index}:"));
            }
        }
        out.push_str(attribute.name());
        out.push_str("=\"");
        escape_markup(attribute.value(), out);
        out.push('"');
    }
}

// The rule as it reads, rebuilt from the parsed elements, never copied as text. Every
// element and attribute keeps its namespace, declared where it changes, so the rule means
// the same in the file written. Its size is bounded by the part's, which the open budget
// charges. Walks with its own stack, however deep.
fn kept_rule(rules: &[Node]) -> String {
    let mut xml = String::new();
    let mut pending: Vec<Visit> = rules
        .iter()
        .rev()
        .map(|n| Visit::Open(*n, Some(MAIN)))
        .collect();
    while let Some(visit) = pending.pop() {
        match visit {
            Visit::Open(n, _) if n.is_text() => {
                escape_markup(n.text().unwrap_or_default(), &mut xml)
            }
            Visit::Open(n, default) if n.is_element() => {
                let namespace = n.tag_name().namespace();
                xml.push('<');
                xml.push_str(n.tag_name().name());
                if namespace != default {
                    xml.push_str(" xmlns=\"");
                    escape_markup(namespace.unwrap_or_default(), &mut xml);
                    xml.push('"');
                }
                attributes_with_namespaces(n, &mut xml);
                if n.children().any(|c| c.is_element() || c.is_text()) {
                    xml.push('>');
                    pending.push(Visit::Close(n));
                    let mut child = n.last_child();
                    while let Some(next) = child {
                        pending.push(Visit::Open(next, namespace));
                        child = next.prev_sibling();
                    }
                } else {
                    xml.push_str("/>");
                }
            }
            Visit::Open(..) => {}
            Visit::Close(n) => {
                xml.push_str("</");
                xml.push_str(n.tag_name().name());
                xml.push('>');
            }
        }
    }
    xml
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
        assert_eq!(filter.columns.len(), 2);
        assert_eq!(filter.columns[0].criteria, FilterCriteria::None);
        assert_eq!(
            filter.columns[1].criteria,
            FilterCriteria::Kept(r#"<rule xmlns="urn:x"/>"#.to_string())
        );
        assert_eq!(filter.sort_state, None);
    }

    #[test]
    fn a_rule_keeps_its_namespaces_and_extension_lists() {
        let filter = filter_of(concat!(
            r#"<autoFilter ref="A1:B2"><filterColumn colId="0"><x:top xml:lang="en" "#,
            r#"x:val="1" val="2"><x:inner/><dynamicFilter type="today"/></x:top></filterColumn>"#,
            r#"<filterColumn colId="1"><filters><filter val="a"/></filters>"#,
            r#"<extLst><ext uri="{1}"><x:more/></ext></extLst></filterColumn></autoFilter>"#
        ))
        .unwrap();
        assert_eq!(
            filter.columns[0].criteria,
            FilterCriteria::Kept(
                concat!(
                    r#"<top xmlns="urn:x" xml:lang="en" xmlns:n0="urn:x" n0:val="1" val="2">"#,
                    r#"<inner/><dynamicFilter xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" "#,
                    r#"type="today"/></top>"#
                )
                .to_string()
            )
        );
        assert_eq!(
            filter.columns[1].criteria,
            FilterCriteria::Kept(
                concat!(
                    r#"<filters><filter val="a"/></filters><extLst><ext uri="{1}">"#,
                    r#"<more xmlns="urn:x"/></ext></extLst>"#
                )
                .to_string()
            )
        );
    }

    #[test]
    fn long_deep_and_nested_rules_are_kept_without_overflowing() {
        let long = format!(
            r#"<autoFilter ref="A1:B2"><filterColumn colId="0"><customFilters><customFilter val="{}"/></customFilters></filterColumn></autoFilter>"#,
            "v".repeat(1 << 20)
        );
        assert_eq!(filter_of(&long).unwrap().columns.len(), 1);
        let deep = format!(
            r#"<autoFilter ref="A1:B2"><filterColumn colId="0">{}{}</filterColumn></autoFilter>"#,
            "<top10>".repeat(500),
            "</top10>".repeat(500)
        );
        assert_eq!(filter_of(&deep).unwrap().columns.len(), 1);
        let nested = r#"<autoFilter ref="A1:B2"><filterColumn colId="0"><top10><autoFilter/></top10></filterColumn></autoFilter>"#;
        assert_eq!(
            filter_of(nested).unwrap().columns[0].criteria,
            FilterCriteria::Kept("<top10><autoFilter/></top10>".to_string())
        );
    }
}
