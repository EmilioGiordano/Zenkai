mod catalog;
mod line_limit;
mod server;

use std::path::{Path, PathBuf};
use std::time::Duration;

use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions};
use rmcp::ServiceExt;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

pub use catalog::{CallError, parse_call, tools};
pub use line_limit::MAX_LINE_BYTES;
pub use server::ZenkaiServer;

use line_limit::LineLimited;

use crate::tools::ToolEndpoint;

pub const PIPE_VARIABLE: &str = "ZENKAI_MCP_PIPE";
pub const TOKEN_VARIABLE: &str = "ZENKAI_MCP_TOKEN";
pub const ENDPOINT_FILE: &str = "mcp-endpoint.txt";
const TOKEN_BYTES: usize = 32;
const TOKEN_TIMEOUT: Duration = Duration::from_secs(5);
// Only the owner of the pipe (the user running Zenkai) may open it.
#[cfg(windows)]
const OWNER_ONLY: &str = "D:P(A;;GA;;;OW)";

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("could not create a random pipe name or token: {0}")]
    Random(String),
    #[error("could not start the MCP bridge: {0}")]
    Start(std::io::Error),
    #[error("could not write {path}: {source}")]
    EndpointFile {
        path: PathBuf,
        source: std::io::Error,
    },
}

fn random_hex(bytes: usize) -> Result<String, BridgeError> {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).map_err(|error| BridgeError::Random(error.to_string()))?;
    Ok(buffer.iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeAddress {
    pub pipe: String,
    pub token: String,
}

impl BridgeAddress {
    fn random() -> Result<BridgeAddress, BridgeError> {
        Ok(BridgeAddress {
            pipe: format!("zenkai-mcp-{}", random_hex(16)?),
            token: random_hex(TOKEN_BYTES)?,
        })
    }

    pub fn parse_endpoint(text: &str) -> Option<BridgeAddress> {
        let mut lines = text.lines().map(str::trim);
        let pipe = lines.next().filter(|line| !line.is_empty())?.to_string();
        let token = lines.next().filter(|line| !line.is_empty())?.to_string();
        Some(BridgeAddress { pipe, token })
    }
}

fn tokens_match(expected: &[u8], given: &[u8]) -> bool {
    // Compared in constant time so the reply time says nothing about the token.
    expected.len() == given.len()
        && expected
            .iter()
            .zip(given)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn listener_options(pipe: &str) -> std::io::Result<ListenerOptions<'static>> {
    let name = pipe.to_string().to_ns_name::<GenericNamespaced>()?;
    let options = ListenerOptions::new().name(name);
    #[cfg(windows)]
    let options = {
        use interprocess::os::windows::local_socket::ListenerOptionsExt;
        use interprocess::os::windows::security_descriptor::SecurityDescriptor;
        let sddl = widestring::U16CString::from_str(OWNER_ONLY)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        options.security_descriptor(SecurityDescriptor::deserialize(&sddl)?)
    };
    Ok(options)
}

async fn serve_connection(stream: LocalSocketStream, token: String, endpoint: ToolEndpoint) {
    let (receive, send) = stream.split();
    let mut reader = BufReader::new(LineLimited::new(receive));
    let mut line = Vec::new();
    let limit = (TOKEN_BYTES * 2 + 2) as u64;
    let read = tokio::time::timeout(
        TOKEN_TIMEOUT,
        (&mut reader).take(limit).read_until(b'\n', &mut line),
    )
    .await;
    if !matches!(read, Ok(Ok(_))) || !tokens_match(token.as_bytes(), line.trim_ascii()) {
        tracing::warn!("refused an MCP client without the session token");
        return;
    }
    match (ZenkaiServer { endpoint }).serve((reader, send)).await {
        Ok(service) => {
            if let Err(error) = service.waiting().await {
                tracing::debug!(%error, "MCP client session ended with an error");
            }
        }
        Err(error) => tracing::warn!(%error, "MCP client failed to initialize"),
    }
}

// Serves MCP on its own thread and tokio runtime; GPUI never runs tokio. Dropping it
// closes the pipe, ends every session and removes the endpoint file.
pub struct Bridge {
    address: BridgeAddress,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    endpoint_file: Option<PathBuf>,
}

impl Bridge {
    pub fn start(
        endpoint: ToolEndpoint,
        endpoint_file: Option<&Path>,
    ) -> Result<Bridge, BridgeError> {
        let address = BridgeAddress::random()?;
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let (ready, started) = std::sync::mpsc::channel::<std::io::Result<()>>();
        let pipe = address.pipe.clone();
        let token = address.token.clone();
        std::thread::Builder::new()
            .name("mcp-bridge".to_string())
            .spawn(move || run(pipe, token, endpoint, stopped, ready))
            .map_err(BridgeError::Start)?;
        started
            .recv()
            .map_err(|_| BridgeError::Start(std::io::Error::other("bridge thread stopped")))?
            .map_err(BridgeError::Start)?;
        let mut bridge = Bridge {
            address,
            stop: Some(stop),
            endpoint_file: None,
        };
        if let Some(path) = endpoint_file {
            write_endpoint(path, &bridge.address)?;
            bridge.endpoint_file = Some(path.to_path_buf());
        }
        Ok(bridge)
    }

    pub fn address(&self) -> &BridgeAddress {
        &self.address
    }
}

fn run(
    pipe: String,
    token: String,
    endpoint: ToolEndpoint,
    mut stopped: tokio::sync::oneshot::Receiver<()>,
    ready: std::sync::mpsc::Sender<std::io::Result<()>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            if ready.send(Err(error)).is_err() {
                tracing::debug!("bridge start was abandoned");
            }
            return;
        }
    };
    runtime.block_on(async move {
        let listener = match listener_options(&pipe).and_then(ListenerOptions::create_tokio) {
            Ok(listener) => listener,
            Err(error) => {
                if ready.send(Err(error)).is_err() {
                    tracing::debug!("bridge start was abandoned");
                }
                return;
            }
        };
        if ready.send(Ok(())).is_err() {
            return;
        }
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                accepted = listener.accept() => match accepted {
                    Ok(stream) => {
                        tokio::spawn(serve_connection(stream, token.clone(), endpoint.clone()));
                    }
                    Err(error) => tracing::warn!(%error, "MCP bridge could not accept a client"),
                },
            }
        }
    });
}

fn write_endpoint(path: &Path, address: &BridgeAddress) -> Result<(), BridgeError> {
    let failed = |source| BridgeError::EndpointFile {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(failed)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let text = format!("{}\n{}\n", address.pipe, address.token);
    options
        .open(path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, text.as_bytes()))
        .map_err(failed)
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take()
            && stop.send(()).is_err()
        {
            tracing::debug!("MCP bridge thread already stopped");
        }
        if let Some(path) = &self.endpoint_file
            && let Err(error) = std::fs::remove_file(path)
        {
            tracing::warn!(%error, "could not remove the MCP endpoint file");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_compare_whole() {
        assert!(tokens_match(b"abcd", b"abcd"));
        assert!(!tokens_match(b"abcd", b"abce"));
        assert!(!tokens_match(b"abcd", b"abc"));
        assert!(!tokens_match(b"abcd", b""));
    }

    #[test]
    fn endpoint_files_round_trip() {
        let address = BridgeAddress::random().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(ENDPOINT_FILE);
        write_endpoint(&path, &address).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(BridgeAddress::parse_endpoint(&text), Some(address));
        assert_eq!(BridgeAddress::parse_endpoint("only-a-pipe\n"), None);
    }

    #[test]
    fn each_bridge_gets_its_own_name_and_token() {
        let first = BridgeAddress::random().unwrap();
        let second = BridgeAddress::random().unwrap();
        assert_ne!(first.pipe, second.pipe);
        assert_ne!(first.token, second.token);
        assert_eq!(first.token.len(), TOKEN_BYTES * 2);
    }
}
