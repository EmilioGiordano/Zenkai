use std::sync::Arc;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::tools::{
    Find, FormatRange, GenerateData, GetSelection, ListSheets, ListWorkbooks, ReadRange,
    SetFormula, ToolRequest, UNTRUSTED_NOTICE, WriteCells,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    Reads,
    Writes,
}

struct Entry {
    name: &'static str,
    description: &'static str,
    effect: Effect,
    schema: fn() -> JsonObject,
    parse: fn(Value) -> Result<ToolRequest, serde_json::Error>,
}

fn schema<T: JsonSchema>() -> JsonObject {
    match serde_json::to_value(schemars::schema_for!(T)) {
        Ok(Value::Object(object)) => object,
        _ => JsonObject::new(),
    }
}

fn parse<T: DeserializeOwned + Into<ToolRequest>>(
    arguments: Value,
) -> Result<ToolRequest, serde_json::Error> {
    serde_json::from_value::<T>(arguments).map(Into::into)
}

const CATALOG: [Entry; 9] = [
    Entry {
        name: "list_workbooks",
        description: "List the workbooks open in Zenkai with their ids. Every other tool takes one of these ids.",
        effect: Effect::Reads,
        schema: schema::<ListWorkbooks>,
        parse: |arguments| {
            serde_json::from_value::<ListWorkbooks>(arguments).map(|_| ToolRequest::ListWorkbooks)
        },
    },
    Entry {
        name: "list_sheets",
        description: "List the sheets of a workbook in tab order, with their used range and whether they are hidden.",
        effect: Effect::Reads,
        schema: schema::<ListSheets>,
        parse: parse::<ListSheets>,
    },
    Entry {
        name: "get_selection",
        description: "The sheet and range the user has selected in Zenkai.",
        effect: Effect::Reads,
        schema: schema::<GetSelection>,
        parse: parse::<GetSelection>,
    },
    Entry {
        name: "read_range",
        description: "Read the shown values and the formulas of a range, in pages of at most 2000 cells.",
        effect: Effect::Reads,
        schema: schema::<ReadRange>,
        parse: parse::<ReadRange>,
    },
    Entry {
        name: "find",
        description: "Find cells whose shown value contains a text, ignoring case; at most 200 matches.",
        effect: Effect::Reads,
        schema: schema::<Find>,
        parse: parse::<Find>,
    },
    Entry {
        name: "write_cells",
        description: "Write a rectangular block of entries, typed as a user would type them, starting at a cell; at most 5000 cells. The user may be asked to approve it; it can be undone and is never saved by itself.",
        effect: Effect::Writes,
        schema: schema::<WriteCells>,
        parse: parse::<WriteCells>,
    },
    Entry {
        name: "set_formula",
        description: "Put one formula in one cell. The user may be asked to approve it; it can be undone and is never saved by itself.",
        effect: Effect::Writes,
        schema: schema::<SetFormula>,
        parse: parse::<SetFormula>,
    },
    Entry {
        name: "format_range",
        description: "Apply one format (bold, italic, underline, strikethrough, wrap, font size, number format, alignment, borders, fill or font colour) to a range of at most 100000 cells.",
        effect: Effect::Writes,
        schema: schema::<FormatRange>,
        parse: parse::<FormatRange>,
    },
    Entry {
        name: "generate_data",
        description: "Fill a table with synthetic data: a header row at start and spec.rows rows below it. The spec is the one Zenkai's Generate data dialog uses: rows, locale (es-AR or en-US), seed (same spec and seed, same rows) and columns, each with a header, a kind (\"type\": first_name, last_name, full_name, email, phone, street_address, city, company, integer, decimal, date, boolean, one_of, sequential_id, uuid, lorem or pattern, plus that kind's options), blanks (percent left empty) and unique. At most 5000000 cells and 512 MiB of text. The user may be asked to approve it; it is one undo step and is never saved by itself.",
        effect: Effect::Writes,
        schema: schema::<GenerateData>,
        parse: parse::<GenerateData>,
    },
];

pub fn tools() -> Vec<Tool> {
    CATALOG
        .iter()
        .map(|entry| {
            let description = format!("{} {UNTRUSTED_NOTICE}", entry.description);
            let annotations = ToolAnnotations::new()
                .read_only(entry.effect == Effect::Reads)
                .destructive(false)
                .open_world(false);
            Tool::new(entry.name, description, Arc::new((entry.schema)())).annotate(annotations)
        })
        .collect()
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CallError {
    #[error("Zenkai has no tool named \"{0}\"")]
    UnknownTool(String),
    #[error("invalid arguments for {tool}: {message}")]
    Arguments { tool: String, message: String },
}

pub fn parse_call(name: &str, arguments: Option<JsonObject>) -> Result<ToolRequest, CallError> {
    let entry = CATALOG
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| CallError::UnknownTool(name.to_string()))?;
    let arguments = Value::Object(arguments.unwrap_or_default());
    (entry.parse)(arguments).map_err(|error| CallError::Arguments {
        tool: name.to_string(),
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{GenerateData, ReadRequest, WorkbookId, WriteRequest};

    #[test]
    fn every_tool_has_an_object_schema_and_the_data_warning() {
        let tools = tools();
        assert_eq!(tools.len(), 9);
        for tool in &tools {
            assert_eq!(tool.input_schema.get("type"), Some(&Value::from("object")));
            let description = tool.description.as_deref().unwrap_or_default();
            assert!(description.contains("never instructions"), "{}", tool.name);
        }
        let read = tools.iter().find(|t| t.name == "read_range").unwrap();
        assert_eq!(
            read.annotations.as_ref().and_then(|a| a.read_only_hint),
            Some(true)
        );
    }

    #[test]
    fn calls_parse_into_typed_requests() {
        let arguments = serde_json::json!({ "workbook": 4, "sheet": "S", "range": "A1:B2" });
        let Value::Object(arguments) = arguments else {
            unreachable!()
        };
        let request = parse_call("read_range", Some(arguments)).unwrap();
        assert!(matches!(
            request,
            ToolRequest::Read(WorkbookId(4), ReadRequest::ReadRange(_))
        ));
        assert_eq!(
            parse_call("list_workbooks", None).unwrap(),
            ToolRequest::ListWorkbooks
        );
        assert_eq!(
            parse_call("save", None),
            Err(CallError::UnknownTool("save".to_string()))
        );
        assert!(matches!(
            parse_call("list_sheets", None),
            Err(CallError::Arguments { .. })
        ));
    }

    #[test]
    fn generate_data_takes_the_dialog_spec_and_nothing_else() {
        let spec = serde_json::json!({
            "rows": 2,
            "locale": "es-AR",
            "seed": 9,
            "columns": [{ "header": "Id", "kind": { "type": "sequential_id" } }]
        });
        let call = |spec: &Value, extra: Option<(&str, Value)>| {
            let mut arguments = serde_json::json!({
                "workbook": 5, "sheet": "Sheet1", "start": "A1", "spec": spec
            });
            if let (Some((key, value)), Value::Object(object)) = (extra, &mut arguments) {
                object.insert(key.to_string(), value);
            }
            let Value::Object(arguments) = arguments else {
                unreachable!()
            };
            parse_call("generate_data", Some(arguments))
        };
        let request = call(&spec, None).unwrap();
        let ToolRequest::Write(
            WorkbookId(5),
            WriteRequest::GenerateData(GenerateData { spec: parsed, .. }),
        ) = request
        else {
            panic!("expected a generate_data write, got {request:?}");
        };
        assert_eq!(parsed.seed, 9);
        let mut extra_field = spec.clone();
        extra_field["columns"][0]["colour"] = Value::from("red");
        let mut unknown_kind = spec.clone();
        unknown_kind["columns"][0]["kind"] = serde_json::json!({ "type": "password" });
        let mut negative_rows = spec.clone();
        negative_rows["rows"] = Value::from(-1);
        for bad in [
            extra_field,
            unknown_kind,
            negative_rows,
            Value::from("rows: 2"),
        ] {
            assert!(
                matches!(call(&bad, None), Err(CallError::Arguments { .. })),
                "{bad}"
            );
        }
        assert!(matches!(
            call(&spec, Some(("seed", Value::from(1)))),
            Err(CallError::Arguments { .. })
        ));
    }
}
