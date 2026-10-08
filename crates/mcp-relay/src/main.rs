#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, Stream};

// Kept in step with zenkai_agent::bridge, which the relay must not link.
const PIPE_VARIABLE: &str = "ZENKAI_MCP_PIPE";
const TOKEN_VARIABLE: &str = "ZENKAI_MCP_TOKEN";
const ENDPOINT_FILE: &str = "mcp-endpoint.txt";

struct Endpoint {
    pipe: String,
    token: String,
}

fn endpoint_file() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })?;
    Some(base.join("Zenkai").join(ENDPOINT_FILE))
}

// A session Zenkai starts passes the endpoint in the environment; an external client
// (such as Claude Code) reads the file Zenkai writes while external agents are allowed.
fn endpoint() -> Result<Endpoint, String> {
    if let (Ok(pipe), Ok(token)) = (std::env::var(PIPE_VARIABLE), std::env::var(TOKEN_VARIABLE)) {
        return Ok(Endpoint { pipe, token });
    }
    let path = endpoint_file().ok_or("no folder for the Zenkai endpoint file")?;
    let text = std::fs::read_to_string(&path).map_err(|_| {
        "Zenkai is not running with external agents allowed. Open Zenkai, then Settings \
         (Ctrl+,) and turn on \"Allow MCP clients outside Zenkai\"."
            .to_string()
    })?;
    let mut lines = text.lines().map(str::trim);
    match (lines.next(), lines.next()) {
        (Some(pipe), Some(token)) if !pipe.is_empty() && !token.is_empty() => Ok(Endpoint {
            pipe: pipe.to_string(),
            token: token.to_string(),
        }),
        _ => Err(format!("{} is not a Zenkai endpoint file", path.display())),
    }
}

fn copy(mut from: impl Read, mut to: impl Write) -> std::io::Result<()> {
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = from.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        to.write_all(&buffer[..read])?;
        to.flush()?;
    }
}

fn relay() -> Result<(), String> {
    let endpoint = endpoint()?;
    let name = endpoint
        .pipe
        .clone()
        .to_ns_name::<GenericNamespaced>()
        .map_err(|error| format!("invalid pipe name: {error}"))?;
    let stream = Stream::connect(name).map_err(|error| {
        format!("could not reach Zenkai ({error}); is it running with agents allowed?")
    })?;
    let (receive, mut send) = stream.split();
    send.write_all(format!("{}\n", endpoint.token).as_bytes())
        .map_err(|error| format!("could not send the session token: {error}"))?;
    // Requests flow on their own thread. The relay ends when Zenkai closes the pipe or
    // when the client closes stdin, which means the client is gone.
    std::thread::spawn(move || {
        let stdin = BufReader::new(std::io::stdin().lock());
        for line in stdin.split(b'\n') {
            let Ok(mut line) = line else { break };
            line.push(b'\n');
            if send.write_all(&line).and_then(|()| send.flush()).is_err() {
                break;
            }
        }
        std::process::exit(0);
    });
    copy(receive, std::io::stdout().lock()).map_err(|error| format!("pipe closed: {error}"))
}

fn main() -> ExitCode {
    match relay() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            // MCP hosts show a server's stderr when it fails to start.
            eprintln!("zenkai-mcp: {message}");
            ExitCode::FAILURE
        }
    }
}
