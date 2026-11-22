// Which conditional formatting rules of a sheet's extension list IronCalc's importer reads
// (`vendor/ironcalc/src/import/conditional_formatting.rs`); a save that writes the sheet
// again loses every other one. Each check mirrors one the importer makes.

use std::collections::HashSet;

use ironcalc::base::cf_types::icon_set_icons;
use roxmltree::{Document, Node};

// Shown as repeated icons; IronCalc reads these without looking the set up.
const RATING_SETS: [&str; 7] = [
    "3Stars",
    "3Rating",
    "4Rating",
    "4Boxes",
    "5Quarters",
    "5Boxes",
    "5Rating",
];
const DEFAULT_ICON_SET: &str = "3TrafficLights1";
// A cfvo of another type, or autoMin and autoMax, sets no threshold.
const THRESHOLD_TYPES: [&str; 6] = ["min", "max", "num", "percent", "percentile", "formula"];

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|n| n.has_tag_name(name))
}

// The extension ids the sheet's own data bars point at: an extended data bar is merged into
// the data bar pointing at it, and one nothing points at is dropped.
pub(crate) fn data_bar_ids<'a>(document: &'a Document<'_>) -> HashSet<&'a str> {
    document
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("conditionalFormatting"))
        .flat_map(|formatting| formatting.children())
        .filter(|rule| rule.has_tag_name("cfRule") && rule.attribute("type") == Some("dataBar"))
        .filter(|rule| child(*rule, "dataBar").is_some())
        .filter_map(|rule| {
            let extension = child(child(rule, "extLst")?, "ext")?;
            child(extension, "id")?.text()
        })
        .collect()
}

// `rule` is a cfRule; only one in conditionalFormattings > conditionalFormatting is an
// extension rule.
pub(crate) fn is_dropped(rule: Node<'_, '_>, data_bar_ids: &HashSet<&str>) -> bool {
    let Some(formatting) = rule
        .parent_element()
        .filter(|parent| parent.has_tag_name("conditionalFormatting"))
    else {
        return false;
    };
    if !formatting
        .parent_element()
        .is_some_and(|list| list.has_tag_name("conditionalFormattings"))
    {
        return false;
    }
    let has_range = child(formatting, "sqref")
        .and_then(|range| range.text())
        .is_some();
    match rule.attribute("type") {
        Some("dataBar") => {
            let merged = rule
                .attribute("id")
                .is_some_and(|id| data_bar_ids.contains(id));
            !(merged && child(rule, "dataBar").is_some())
        }
        Some("iconSet") => !(has_range && is_icon_set_read(rule)),
        Some("expression") => !has_range,
        _ => true,
    }
}

fn is_icon_set_read(rule: Node<'_, '_>) -> bool {
    let Some(icons) = child(rule, "iconSet") else {
        return false;
    };
    let has_threshold = icons.children().any(|cfvo| {
        cfvo.has_tag_name("cfvo")
            && THRESHOLD_TYPES.contains(&cfvo.attribute("type").unwrap_or("num"))
    });
    if !has_threshold {
        return false;
    }
    let set = icons.attribute("iconSet").unwrap_or(DEFAULT_ICON_SET);
    if icons.attribute("custom") == Some("1") {
        icons.children().any(|icon| icon.has_tag_name("cfIcon"))
    } else {
        RATING_SETS.contains(&set) || icon_set_icons(set).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = concat!(
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" "#,
        r#"xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" "#,
        r#"xmlns:xm="http://schemas.microsoft.com/office/excel/2006/main"><sheetData/>"#,
        r#"<conditionalFormatting sqref="A1:A3"><cfRule type="dataBar" priority="1"><dataBar>"#,
        r#"<cfvo type="min"/><cfvo type="max"/><color rgb="FF638EC6"/></dataBar><extLst><ext>"#,
        r#"<x14:id>{1}</x14:id></ext></extLst></cfRule></conditionalFormatting><extLst><ext>"#,
        r#"<x14:conditionalFormattings><x14:conditionalFormatting>RULE"#,
        r#"</x14:conditionalFormatting></x14:conditionalFormattings></ext></extLst></worksheet>"#
    );

    fn dropped(rule: &str) -> bool {
        let xml = SHEET.replace("RULE", rule);
        let document = Document::parse(&xml).unwrap();
        let ids = data_bar_ids(&document);
        let rule = document
            .descendants()
            .find(|n| {
                n.has_tag_name("cfRule")
                    && n.ancestors()
                        .any(|a| a.has_tag_name("conditionalFormattings"))
            })
            .unwrap();
        is_dropped(rule, &ids)
    }

    const RANGE: &str = "<xm:sqref>A1:A3</xm:sqref>";

    #[test]
    fn a_data_bar_is_read_only_when_a_data_bar_of_the_sheet_points_at_it() {
        let bar = |id: &str| {
            format!(r#"<x14:cfRule type="dataBar" id="{id}"><x14:dataBar/></x14:cfRule>"#)
        };
        assert!(!dropped(&bar("{1}")));
        assert!(dropped(&bar("{2}")));
        assert!(dropped(r#"<x14:cfRule type="dataBar" id="{1}"/>"#));
    }

    #[test]
    fn an_icon_set_is_read_only_as_ironcalc_reads_it() {
        let icons = |set: &str, cfvo: &str| {
            format!(
                r#"<x14:cfRule type="iconSet"><x14:iconSet{set}>{cfvo}</x14:iconSet></x14:cfRule>{RANGE}"#
            )
        };
        let threshold = r#"<x14:cfvo type="percent"><xm:f>0</xm:f></x14:cfvo>"#;
        assert!(!dropped(&icons("", threshold)));
        assert!(!dropped(&icons(r#" iconSet="3Stars""#, threshold)));
        assert!(dropped(&icons(r#" iconSet="5Signal""#, threshold)));
        assert!(dropped(&icons("", r#"<x14:cfvo type="autoMin"/>"#)));
        assert!(dropped(&icons(r#" custom="1""#, threshold)));
        let custom = format!(r#"{threshold}<x14:cfIcon iconSet="3Flags" iconId="0"/>"#);
        assert!(!dropped(&icons(r#" custom="1""#, &custom)));
        assert!(dropped(&icons("", threshold).replace(RANGE, "")));
    }

    #[test]
    fn an_expression_needs_its_range_and_other_types_are_dropped() {
        let rule = |kind: &str| {
            format!(r#"<x14:cfRule type="{kind}"><xm:f>Other!A1</xm:f></x14:cfRule>{RANGE}"#)
        };
        assert!(!dropped(&rule("expression")));
        assert!(dropped(&rule("expression").replace(RANGE, "")));
        assert!(dropped(&rule("cellIs")));
    }
}
