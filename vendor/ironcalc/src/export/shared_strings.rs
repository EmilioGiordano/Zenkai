use ironcalc_base::types::Workbook;

use super::{escape::escape_xml, xml_constants::XML_DECLARATION};

/// The `<si>` element of one shared string, as the shared strings part holds it.
pub fn get_shared_string_item_xml(text: &str) -> String {
    format!("<si><t>{}</t></si>", escape_xml(text))
}

pub(crate) fn get_shared_strings_xml(model: &Workbook) -> String {
    let mut shared_strings: Vec<String> = vec![];
    let count = &model.shared_strings.len();
    let unique_count = &model.shared_strings.len();
    for shared_string in &model.shared_strings {
        shared_strings.push(get_shared_string_item_xml(shared_string));
    }
    format!("{}\n\
      <sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" count=\"{count}\" uniqueCount=\"{unique_count}\">\
        {}\
      </sst>", XML_DECLARATION, shared_strings.join(""))
}
