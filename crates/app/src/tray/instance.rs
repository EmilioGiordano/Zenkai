use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, Listener, ListenerOptions, Stream};
use serde::{Deserialize, Serialize};

use super::TrayEvent;

const MAX_MESSAGE_BYTES: u64 = 64 * 1024;
const ACK: &str = "ok";
// A running Zenkai that does not answer within this is stuck, so the new launch goes on alone.
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(4);
const ACK_TIMEOUT: Duration = Duration::from_secs(10);

pub enum Claim {
    Primary(Listener),
    HandedOff,
    Standalone,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Handoff {
    open: Option<String>,
}

impl Handoff {
    fn of(open: Option<&Path>) -> Handoff {
        Handoff {
            open: open.and_then(|path| path.to_str()).map(str::to_string),
        }
    }

    fn encode(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }

    fn decode(line: &str) -> Option<Handoff> {
        serde_json::from_str(line.trim()).ok()
    }
}

// Keyed by user and by executable, so another account or a debug build next to an installed
// one never hands its launch to the wrong process.
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

// The first process to create the pipe is the running Zenkai; anyone else hands over.
pub fn claim(open: Option<&Path>) -> Claim {
    let Some(pipe) = own_pipe_name() else {
        tracing::debug!("could not name the single instance pipe");
        return Claim::Standalone;
    };
    let Ok(name) = pipe.as_str().to_ns_name::<GenericNamespaced>() else {
        return Claim::Standalone;
    };
    match ListenerOptions::new().name(name).create_sync() {
        Ok(listener) => Claim::Primary(listener),
        Err(error) => {
            tracing::debug!(%error, "the instance pipe exists; handing the launch over");
            let absolute = open.map(|path| std::path::absolute(path).unwrap_or(path.to_path_buf()));
            if hand_over(&pipe, &Handoff::of(absolute.as_deref())) {
                Claim::HandedOff
            } else {
                Claim::Standalone
            }
        }
    }
}

fn hand_over(pipe: &str, handoff: &Handoff) -> bool {
    let (done, answered) = mpsc::channel();
    let pipe = pipe.to_string();
    let Some(line) = handoff.encode() else {
        return false;
    };
    std::thread::spawn(move || {
        let sent = send(&pipe, &line);
        if done.send(sent).is_err() {
            tracing::debug!("the launch stopped waiting for the running Zenkai");
        }
    });
    answered.recv_timeout(HANDOFF_TIMEOUT).unwrap_or(false)
}

fn send(pipe: &str, line: &str) -> bool {
    let Ok(name) = pipe.to_ns_name::<GenericNamespaced>() else {
        return false;
    };
    let Ok(stream) = Stream::connect(name) else {
        return false;
    };
    let mut stream = BufReader::new(stream);
    if writeln!(stream.get_mut(), "{line}").is_err() || stream.get_mut().flush().is_err() {
        return false;
    }
    let mut reply = String::new();
    stream.read_line(&mut reply).is_ok() && reply.trim() == ACK
}

// Accepting happens on a thread of its own; every request is answered only after the
// foreground has handled it, so a stuck window never looks like a healthy one.
pub fn serve(listener: Listener, events: async_channel::Sender<TrayEvent>) {
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            match connection {
                Ok(stream) => {
                    let events = events.clone();
                    std::thread::spawn(move || answer(stream, &events));
                }
                Err(error) => tracing::warn!(%error, "the instance pipe refused a connection"),
            }
        }
    });
}

fn answer(stream: Stream, events: &async_channel::Sender<TrayEvent>) {
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    if (&mut stream)
        .take(MAX_MESSAGE_BYTES)
        .read_line(&mut line)
        .is_err()
    {
        return;
    }
    let Some(handoff) = Handoff::decode(&line) else {
        tracing::warn!("ignored a malformed message on the instance pipe");
        return;
    };
    let (handled, wait) = mpsc::channel();
    let open = handoff.open.map(PathBuf::from);
    if events
        .send_blocking(TrayEvent::Open {
            path: open,
            handled,
        })
        .is_err()
    {
        return;
    }
    if wait.recv_timeout(ACK_TIMEOUT).is_ok() {
        let reply = stream.get_mut();
        if let Err(error) = writeln!(reply, "{ACK}").and_then(|()| reply.flush()) {
            tracing::debug!(%error, "the launching process went away before the answer");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handoff_round_trips_with_and_without_a_file() {
        let none = Handoff::of(None);
        assert_eq!(Handoff::decode(&none.encode().unwrap()), Some(none));
        let some = Handoff::of(Some(Path::new("C:\\Datos\\Ñandú planilla.xlsx")));
        assert_eq!(Handoff::decode(&some.encode().unwrap()), Some(some));
    }

    #[test]
    fn a_malformed_message_is_not_a_handoff() {
        assert_eq!(Handoff::decode(""), None);
        assert_eq!(Handoff::decode("not json"), None);
        assert_eq!(Handoff::decode("{\"open\":5}"), None);
    }

    #[test]
    fn a_message_line_ends_with_a_newline_that_decode_ignores() {
        assert_eq!(
            Handoff::decode("{\"open\":null}\r\n"),
            Some(Handoff { open: None })
        );
    }

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

    // One test, because the pipe name is per executable: a second claim in this process is
    // what a second launch looks like.
    #[test]
    fn a_second_launch_gives_up_on_a_stuck_first_one_and_hands_over_to_a_live_one() {
        let Claim::Primary(listener) = claim(None) else {
            panic!("the first claim must own the pipe");
        };
        let started = std::time::Instant::now();
        assert!(matches!(claim(None), Claim::Standalone));
        assert!(started.elapsed() < HANDOFF_TIMEOUT + Duration::from_secs(3));

        let (events, received) = async_channel::unbounded();
        serve(listener, events);
        let (opened, opened_paths) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(event) = received.recv_blocking() {
                if let TrayEvent::Open { path, handled } = event {
                    opened.send(path).unwrap();
                    handled.send(()).unwrap();
                }
            }
        });
        assert!(matches!(
            claim(Some(Path::new("planilla.xlsx"))),
            Claim::HandedOff
        ));
        let paths: Vec<Option<PathBuf>> = opened_paths.try_iter().collect();
        let last = paths.last().unwrap().as_ref().unwrap();
        assert!(last.is_absolute() && last.ends_with("planilla.xlsx"));
    }
}
