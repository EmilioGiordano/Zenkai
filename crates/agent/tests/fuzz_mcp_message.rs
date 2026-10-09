// Structured fuzzing of the MCP messages a client sends over the pipe: a damaged JSON-RPC
// message is refused as a message or as a tool call, never a panic.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../test-support/json_mutation.rs"]
mod json_mutation;

use proptest::prelude::*;
use rmcp::model::{ClientJsonRpcMessage, ClientRequest};
use serde_json::{Value, json};
use zenkai_agent::bridge::{CallError, parse_call, tools};

fn call(name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
}

fn messages() -> Vec<Value> {
    vec![
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "claude-code", "version": "1.0" }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }),
        call("list_workbooks", json!({})),
        call("list_sheets", json!({ "workbook": 1 })),
        call("get_selection", json!({ "workbook": 1 })),
        call(
            "read_range",
            json!({ "workbook": 1, "sheet": "Sheet1", "range": "A1:D20", "page": 0 }),
        ),
        call(
            "find",
            json!({ "workbook": 1, "text": "total", "sheet": "Sheet1" }),
        ),
        call(
            "write_cells",
            json!({ "workbook": 1, "sheet": "Sheet1", "start": "B2", "rows": [["1", "=A1*2"], ["x", ""]] }),
        ),
        call(
            "set_formula",
            json!({ "workbook": 1, "sheet": "Sheet1", "cell": "C7", "formula": "=SUM(A1:A6)" }),
        ),
        call(
            "format_range",
            json!({ "workbook": 1, "sheet": "Sheet1", "range": "A1:B2", "format": { "bold": true } }),
        ),
    ]
}

fn handle(bytes: &[u8]) {
    let Ok(message) = serde_json::from_slice::<ClientJsonRpcMessage>(bytes) else {
        return;
    };
    if let ClientJsonRpcMessage::Request(request) = message
        && let ClientRequest::CallToolRequest(call) = request.request
    {
        let params = call.params;
        match parse_call(&params.name, params.arguments) {
            Ok(_) | Err(CallError::UnknownTool(_) | CallError::Arguments { .. }) => {}
        }
    }
}

#[test]
fn every_baseline_message_is_understood() {
    for message in messages() {
        let parsed = serde_json::from_value::<ClientJsonRpcMessage>(message.clone());
        assert!(parsed.is_ok(), "{message}");
    }
    assert_eq!(tools().len(), 8);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn damaged_messages_are_refused_without_a_panic(
        bytes in prop::sample::select(messages()).prop_flat_map(json_mutation::mutated)
    ) {
        handle(&bytes);
    }

    #[test]
    fn arbitrary_bytes_are_refused_without_a_panic(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        handle(&bytes);
    }

    #[test]
    fn arbitrary_tool_calls_are_refused_without_a_panic(
        name in prop_oneof![
            Just("read_range".to_string()),
            Just("write_cells".to_string()),
            Just("format_range".to_string()),
            "[a-z_]{0,12}",
        ],
        bytes in prop::sample::select(messages()).prop_flat_map(json_mutation::mutated),
    ) {
        if let Ok(Value::Object(arguments)) = serde_json::from_slice::<Value>(&bytes) {
            let _ = parse_call(&name, Some(arguments));
        }
    }
}
