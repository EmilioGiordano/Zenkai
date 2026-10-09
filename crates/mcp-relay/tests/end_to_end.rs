#![allow(
    clippy::unwrap_used,
    reason = "test helpers fail the test by panicking; clippy only exempts #[test] functions"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, Stream};
use serde_json::{Value, json};
use zenkai_agent::bridge::{Bridge, ENDPOINT_FILE, MAX_LINE_BYTES, PIPE_VARIABLE, TOKEN_VARIABLE};
use zenkai_agent::tools::{AgentAccess, LocalHost, WorkbookId, channel};
use zenkai_engine::{Engine, Workbook};
use zenkai_types::{CellPos, Range, SheetId};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Client {
    fn start(bridge: &Bridge, token: &str) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_zenkai-mcp"))
            .env(PIPE_VARIABLE, &bridge.address().pipe)
            .env(TOKEN_VARIABLE, token)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Client {
            child,
            stdin,
            stdout,
        }
    }

    fn from_endpoint_file(local_app_data: &std::path::Path) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_zenkai-mcp"))
            .env_remove(PIPE_VARIABLE)
            .env_remove(TOKEN_VARIABLE)
            .env("LOCALAPPDATA", local_app_data)
            .env("XDG_DATA_HOME", local_app_data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Client {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "relay closed"
            );
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    fn call(&mut self, id: u64, tool: &str, arguments: Value) -> Value {
        let reply = self.request(
            id,
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        reply["result"].clone()
    }

    fn initialize(&mut self) -> Value {
        let reply = self.request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "scripted-client", "version": "1" }
            }),
        );
        self.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        reply
    }
}

fn text_of(result: &Value) -> String {
    result["content"][0]["text"].as_str().unwrap().to_string()
}

fn host() -> LocalHost {
    let mut workbook = Workbook::new_empty().unwrap();
    let rows = vec![vec!["Tea".to_string(), "3".to_string()]];
    workbook
        .set_inputs(SheetId(0), CellPos::default(), &rows)
        .unwrap();
    LocalHost {
        id: WorkbookId(9),
        name: "Prices.xlsx".to_string(),
        workbook,
        selection: (SheetId(0), Range::parse_a1("A1").unwrap()),
        access: AgentAccess::Editable,
    }
}

#[test]
fn an_mcp_client_reads_and_edits_the_open_workbook_through_the_relay() {
    let (endpoint, calls) = channel();
    let server = std::thread::spawn(move || host().serve(calls));
    let bridge = Bridge::start(endpoint, None).unwrap();
    let mut client = Client::start(&bridge, &bridge.address().token);

    let info = client.initialize();
    assert_eq!(info["result"]["serverInfo"]["name"], "zenkai");

    let listed = client.request(2, "tools/list", json!({}));
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "list_workbooks",
            "list_sheets",
            "get_selection",
            "read_range",
            "find",
            "write_cells",
            "set_formula",
            "format_range"
        ]
    );

    let workbooks = text_of(&client.call(3, "list_workbooks", json!({})));
    assert!(workbooks.contains("Workbook id 9"), "{workbooks}");
    assert!(workbooks.contains("<<<UNTRUSTED SPREADSHEET DATA"));

    let written = client.call(
        4,
        "write_cells",
        json!({ "workbook": 9, "sheet": "Sheet1", "start": "A2", "rows": [["Cake", "4"], ["Total", "=SUM(B1:B2)"]] }),
    );
    assert_ne!(written["isError"], json!(true), "{written}");

    let read = text_of(&client.call(
        5,
        "read_range",
        json!({ "workbook": 9, "sheet": "Sheet1", "range": "A1:B3" }),
    ));
    assert!(read.contains(r#"["Total","7"]"#), "{read}");

    let stale = client.call(
        6,
        "read_range",
        json!({ "workbook": 8, "sheet": "Sheet1", "range": "A1" }),
    );
    assert_eq!(stale["isError"], json!(true));

    let saving = client.request(7, "tools/call", json!({ "name": "save", "arguments": {} }));
    assert_eq!(saving["result"]["isError"], json!(true));

    client.child.kill().unwrap();
    client.child.wait().unwrap();
    drop(bridge);
    let mut host = server.join().unwrap();
    let sheet = SheetId(0);
    let a3 = CellPos::parse_a1("A3").unwrap();
    assert_eq!(host.workbook.input(sheet, a3), "Total");
    host.workbook.undo().unwrap();
    assert_eq!(host.workbook.input(sheet, a3), "");
    assert_eq!(
        host.workbook.input(sheet, CellPos::parse_a1("A2").unwrap()),
        ""
    );
    assert_eq!(host.workbook.input(sheet, CellPos::default()), "Tea");
}

#[test]
fn a_client_without_the_token_gets_nothing() {
    let (endpoint, calls) = channel();
    let server = std::thread::spawn(move || host().serve(calls));
    let bridge = Bridge::start(endpoint, None).unwrap();
    let mut client = Client::start(&bridge, "not-the-token");
    client.send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "x", "version": "1" } }
    }));
    let mut line = String::new();
    let read = client.stdout.read_line(&mut line).unwrap_or(0);
    assert_eq!(read, 0, "unexpected reply {line}");
    client.child.kill().ok();
    client.child.wait().unwrap();
    drop(bridge);
    server.join().unwrap();
}

#[test]
fn the_endpoint_file_exists_only_while_the_bridge_runs() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Zenkai").join("mcp-endpoint.txt");
    let (endpoint, _calls) = channel();
    let bridge = Bridge::start(endpoint, Some(&file)).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains(&bridge.address().pipe));
    drop(bridge);
    assert!(!file.exists());
}

#[test]
fn the_relay_finds_zenkai_through_the_endpoint_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Zenkai").join(ENDPOINT_FILE);
    let (endpoint, calls) = channel();
    let server = std::thread::spawn(move || host().serve(calls));
    let bridge = Bridge::start(endpoint, Some(&file)).unwrap();
    let mut client = Client::from_endpoint_file(dir.path());
    let info = client.initialize();
    assert_eq!(info["result"]["serverInfo"]["name"], "zenkai");
    client.child.kill().unwrap();
    client.child.wait().unwrap();
    drop(bridge);
    server.join().unwrap();
}

#[test]
fn the_relay_stops_at_an_oversized_message() {
    let (endpoint, calls) = channel();
    let server = std::thread::spawn(move || host().serve(calls));
    let bridge = Bridge::start(endpoint, None).unwrap();
    let mut client = Client::start(&bridge, &bridge.address().token);
    let huge = vec![b'x'; MAX_LINE_BYTES + 1];
    let written = client.stdin.write_all(&huge);
    let status = client.child.wait().unwrap();
    assert!(!status.success(), "{written:?}");
    drop(bridge);
    server.join().unwrap();
}

#[test]
fn the_bridge_drops_a_client_sending_an_oversized_message() {
    let (endpoint, calls) = channel();
    let server = std::thread::spawn(move || host().serve(calls));
    let bridge = Bridge::start(endpoint, None).unwrap();
    let name = bridge
        .address()
        .pipe
        .clone()
        .to_ns_name::<GenericNamespaced>()
        .unwrap();
    let stream = Stream::connect(name).unwrap();
    let (mut receive, mut send) = stream.split();
    send.write_all(format!("{}\n", bridge.address().token).as_bytes())
        .unwrap();
    let sent = send.write_all(&vec![b'x'; MAX_LINE_BYTES + 4096]);
    let mut reply = Vec::new();
    let read = receive.read_to_end(&mut reply);
    assert!(reply.is_empty(), "{sent:?} {read:?}");
    drop(bridge);
    server.join().unwrap();
}
