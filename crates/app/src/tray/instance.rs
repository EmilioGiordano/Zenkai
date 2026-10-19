use std::io::{BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, Listener, ListenerNonblockingMode, Stream};
use zenkai_agent::bridge::{listener_options, random_hex, tokens_match};

use super::TrayEvent;
use super::handoff::Handoff;

const TOKEN_BYTES: usize = 32;
const MAX_MESSAGE_BYTES: usize = 256 * 1024;
const ACK: &str = "ok";
const CONNECT_WINDOW_MS: u64 = 2_000;
const TOKEN_RETRIES: u64 = 3;
const TOKEN_RETRY_WAIT_MS: u64 = 150;
const READ_DEADLINE_MS: u64 = 2_000;
// The whole attempt: connecting, the token retries, and the token read, the write and the
// acknowledgement, each of which has its own READ_DEADLINE.
const HANDOFF_TIMEOUT: Duration = Duration::from_millis(
    CONNECT_WINDOW_MS + (TOKEN_RETRIES - 1) * TOKEN_RETRY_WAIT_MS + 3 * READ_DEADLINE_MS,
);
const CONNECT_WINDOW: Duration = Duration::from_millis(CONNECT_WINDOW_MS);
const TOKEN_RETRY_WAIT: Duration = Duration::from_millis(TOKEN_RETRY_WAIT_MS);
const READ_DEADLINE: Duration = Duration::from_millis(READ_DEADLINE_MS);
const READ_POLL: Duration = Duration::from_millis(10);
const MAX_HANDLERS: usize = 4;
const CONNECT_RETRY: Duration = Duration::from_millis(25);

pub struct Primary {
    listener: Listener,
    token: String,
    token_file: PathBuf,
}

impl Primary {
    pub fn token_file(&self) -> PathBuf {
        self.token_file.clone()
    }
}

pub enum Claim {
    Primary(Primary),
    HandedOff,
    Unreachable,
    Standalone,
}

// Keyed by user and executable, so another account or a debug build never meets this one.
fn pipe_name(user: &str, exe: &Path) -> String {
    let path = exe.to_string_lossy().to_lowercase();
    let hash = path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    let user: String = user
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("zenkai-instance-{user}-{hash:016x}")
}

fn own_pipe_name() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let user = std::env::var("USERNAME").unwrap_or_default();
    Some(pipe_name(&user, &exe))
}

fn token_path(pipe: &str) -> Option<PathBuf> {
    Some(crate::recovery::session_directory()?.join(format!("{pipe}.token")))
}

pub fn claim(open: Option<&Path>) -> Claim {
    let Some(pipe) = own_pipe_name() else {
        return Claim::Standalone;
    };
    let created = listener_options(&pipe).and_then(|options| {
        options
            .nonblocking(ListenerNonblockingMode::Stream)
            .create_sync()
    });
    match created {
        Ok(listener) => match publish_token(&pipe) {
            Some((token, token_file)) => Claim::Primary(Primary {
                listener,
                token,
                token_file,
            }),
            None => Claim::Standalone,
        },
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::PermissionDenied | ErrorKind::AddrInUse
            ) =>
        {
            tracing::debug!(%error, "the instance pipe exists");
            let handoff = match open.map(std::path::absolute).transpose() {
                Ok(absolute) => Handoff::of(absolute.as_deref()),
                Err(error) => {
                    tracing::warn!(%error, "the file to open has no absolute path; opening none");
                    Handoff::of(None)
                }
            };
            if hand_over(&pipe, &handoff) {
                Claim::HandedOff
            } else {
                Claim::Unreachable
            }
        }
        Err(error) => {
            tracing::debug!(%error, "could not create the instance pipe");
            Claim::Standalone
        }
    }
}

// Only a process that can read this file can prove it is the running Zenkai of this user.
// The old file is removed first and the new one created exclusively, so a planted link is
// never followed.
fn publish_token(pipe: &str) -> Option<(String, PathBuf)> {
    let path = token_path(pipe)?;
    let token = match random_hex(TOKEN_BYTES) {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(%error, "no random token for the instance pipe; starting alone");
            return None;
        }
    };
    if let Err(error) = write_token_file(&path, &token) {
        tracing::warn!(%error, path = %path.display(), "could not write the instance token; starting alone");
        return None;
    }
    Some((token, path))
}

fn write_token_file(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(token.as_bytes())
}

pub fn remove_token_file(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        tracing::debug!(%error, "could not remove the instance token");
    }
}

fn hand_over(pipe: &str, handoff: &Handoff) -> bool {
    let line = match handoff.encode() {
        Ok(line) => line,
        Err(error) => {
            tracing::warn!(%error, "could not encode the launch for the running Zenkai");
            return false;
        }
    };
    let (done, answered) = mpsc::channel();
    let pipe = pipe.to_string();
    std::thread::spawn(move || {
        if done.send(send(&pipe, &line)).is_err() {
            tracing::debug!("the launch stopped waiting for the running Zenkai");
        }
    });
    answered.recv_timeout(HANDOFF_TIMEOUT).unwrap_or(false)
}

// The listener makes its next pipe instance only after accepting, so a connect that lands
// in between is busy for a moment.
fn connect(pipe: &str) -> Option<Stream> {
    let deadline = Instant::now() + CONNECT_WINDOW;
    loop {
        let name = pipe.to_ns_name::<GenericNamespaced>().ok()?;
        match Stream::connect(name) {
            Ok(stream) => return Some(stream),
            Err(_) if Instant::now() < deadline => std::thread::sleep(CONNECT_RETRY),
            Err(error) => {
                tracing::debug!(%error, "could not connect to the instance pipe");
                return None;
            }
        }
    }
}

fn send(pipe: &str, line: &str) -> bool {
    let Some(stream) = connect(pipe) else {
        return false;
    };
    if let Err(error) = stream.set_nonblocking(true) {
        tracing::debug!(%error, "could not poll the instance pipe");
        return false;
    }
    let mut stream = BufReader::new(stream);
    let offered = read_line_by_deadline(&mut stream).unwrap_or_default();
    if !is_ours(pipe, offered.trim()) {
        tracing::warn!("the instance pipe is held by a process that is not this user's Zenkai");
        return false;
    }
    if !write_line(stream.get_mut(), line) {
        return false;
    }
    read_line_by_deadline(&mut stream).is_some_and(|reply| reply.trim() == ACK)
}

// The token file is written just after the pipe exists, so a mismatch is retried briefly.
fn is_ours(pipe: &str, offered: &str) -> bool {
    let Some(path) = token_path(pipe) else {
        return false;
    };
    for attempt in 0..TOKEN_RETRIES {
        if attempt > 0 {
            std::thread::sleep(TOKEN_RETRY_WAIT);
        }
        if std::fs::read_to_string(&path).is_ok_and(|expected| token_matches(&expected, offered)) {
            return true;
        }
    }
    false
}

fn token_matches(expected: &str, given: &str) -> bool {
    let expected = expected.trim().as_bytes();
    !expected.is_empty() && tokens_match(expected, given.as_bytes())
}

struct HandlerSlot(Arc<AtomicUsize>);

impl Drop for HandlerSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn reserve_handler(active: &Arc<AtomicUsize>) -> Option<HandlerSlot> {
    if active.fetch_add(1, Ordering::SeqCst) >= MAX_HANDLERS {
        active.fetch_sub(1, Ordering::SeqCst);
        return None;
    }
    Some(HandlerSlot(active.clone()))
}

pub fn serve(primary: Primary, events: async_channel::Sender<TrayEvent>) {
    let Primary {
        listener, token, ..
    } = primary;
    let token: Arc<str> = token.into();
    let active = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let stream = match connection {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(%error, "the instance pipe refused a connection");
                    continue;
                }
            };
            let Some(slot) = reserve_handler(&active) else {
                tracing::warn!("too many launches at once; one was dropped");
                continue;
            };
            let (events, token) = (events.clone(), token.clone());
            std::thread::spawn(move || {
                answer(stream, &token, &events);
                drop(slot);
            });
        }
    });
}

fn answer(stream: Stream, token: &str, events: &async_channel::Sender<TrayEvent>) {
    let mut stream = BufReader::new(stream);
    if !write_line(stream.get_mut(), token) {
        return;
    }
    let Some(line) = read_line_by_deadline(&mut stream) else {
        tracing::warn!("a launch sent nothing usable in time");
        return;
    };
    let Some(handoff) = Handoff::decode(&line) else {
        tracing::warn!("ignored a malformed message on the instance pipe");
        return;
    };
    let (path, refusal) = match handoff.requested() {
        Some(Ok(path)) => (Some(path), None),
        Some(Err(refusal)) => {
            tracing::warn!(%refusal, "refused the file a launch asked to open");
            (None, Some(refusal))
        }
        None => (None, None),
    };
    if events.try_send(TrayEvent::Open { path }).is_err() {
        return;
    }
    if let Some(refusal) = refusal
        && events.try_send(TrayEvent::Refused(refusal)).is_err()
    {
        return;
    }
    if !write_line(stream.get_mut(), ACK) {
        tracing::debug!("the launching process went away before the answer");
    }
}

// The accepted stream is non-blocking, so a write can ask to be retried.
fn write_line(stream: &mut Stream, text: &str) -> bool {
    let bytes = format!("{text}\n").into_bytes();
    let deadline = Instant::now() + READ_DEADLINE;
    let mut sent = 0;
    while sent < bytes.len() {
        match stream.write(&bytes[sent..]) {
            Ok(0) => return false,
            Ok(written) => sent += written,
            Err(error) if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(READ_POLL);
            }
            Err(_) => return false,
        }
    }
    stream.flush().is_ok()
}

// Accepted streams are non-blocking (pipes have no read timeout), so reading polls until the deadline.
fn read_line_by_deadline(stream: &mut BufReader<Stream>) -> Option<String> {
    let deadline = Instant::now() + READ_DEADLINE;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let line = loop {
        if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.truncate(end);
            break Some(bytes);
        }
        if bytes.len() > MAX_MESSAGE_BYTES || Instant::now() >= deadline {
            break None;
        }
        match stream.read(&mut chunk) {
            Ok(0) => std::thread::sleep(READ_POLL),
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => std::thread::sleep(READ_POLL),
            Err(_) => break None,
        }
    };
    String::from_utf8(line?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::handoff::Refusal;

    #[test]
    fn the_pipe_name_depends_on_user_and_executable() {
        let one = pipe_name("ana", Path::new("C:\\a\\zenkai.exe"));
        assert_eq!(one, pipe_name("ana", Path::new("c:\\A\\Zenkai.exe")));
        assert_ne!(one, pipe_name("bob", Path::new("C:\\a\\zenkai.exe")));
        assert_ne!(one, pipe_name("ana", Path::new("C:\\b\\zenkai.exe")));
        assert!(
            pipe_name("a b\\c", Path::new("x"))
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn tokens_match_only_when_equal_and_not_empty() {
        assert!(token_matches("abc\r\n", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "ab"));
        assert!(!token_matches("", ""));
    }

    #[test]
    fn exactly_four_handlers_run_at_once_and_a_finished_one_frees_its_place() {
        let active = Arc::new(AtomicUsize::new(0));
        let mut slots: Vec<_> = (0..MAX_HANDLERS)
            .map(|_| reserve_handler(&active).expect("a free place"))
            .collect();
        assert!(reserve_handler(&active).is_none());
        slots.pop();
        assert!(reserve_handler(&active).is_some());
    }

    // One test, because the pipe name is per executable: a second claim in this process is
    // what a second launch looks like.
    #[test]
    fn a_second_launch_hands_over_to_a_live_first_and_never_to_an_unproven_one() {
        let Claim::Primary(primary) = claim(None) else {
            panic!("the first claim must own the pipe");
        };
        let token = primary.token.clone();

        let started = Instant::now();
        assert!(matches!(claim(None), Claim::Unreachable));
        assert!(started.elapsed() < HANDOFF_TIMEOUT + Duration::from_secs(3));

        let (events, received) = async_channel::unbounded();
        serve(primary, events);
        let (opened, opened_paths) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(event) = received.recv_blocking() {
                match event {
                    TrayEvent::Open { path } => opened.send(Ok(path)).unwrap(),
                    TrayEvent::Refused(refusal) => opened.send(Err(refusal)).unwrap(),
                    _ => {}
                }
            }
        });
        assert!(matches!(
            claim(Some(Path::new("C:\\planilla.xlsx"))),
            Claim::HandedOff
        ));
        assert!(matches!(
            claim(Some(Path::new("C:\\run.exe"))),
            Claim::HandedOff
        ));
        let mut paths: Vec<Option<PathBuf>> = Vec::new();
        let mut refusals = Vec::new();
        while let Ok(path) = opened_paths.recv_timeout(Duration::from_millis(700)) {
            match path {
                Ok(path) => paths.push(path),
                Err(refusal) => refusals.push(refusal),
            }
        }
        assert_eq!(refusals, [Refusal::UnsupportedExtension]);
        assert!(paths.contains(&Some(PathBuf::from(r"C:\planilla.xlsx"))));
        assert_eq!(
            paths.iter().flatten().count(),
            1,
            "the unsupported file is refused but its launch still raises the window"
        );

        let file = token_path(&own_pipe_name().unwrap()).unwrap();
        std::fs::write(&file, "someone else's token").unwrap();
        assert!(matches!(claim(None), Claim::Unreachable));
        std::fs::write(&file, token).unwrap();
    }
}
