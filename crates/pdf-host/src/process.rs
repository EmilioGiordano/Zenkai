use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

const KILL_WAIT: Duration = Duration::from_millis(50);
const KILL_WAITS: u32 = 40;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// Only next to the exe, so a binary planted in the working directory or on the path never runs.
pub fn host_next_to_exe(file: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(file))
}

pub fn spawn_piped(
    path: &Path,
    arguments: &[OsString],
    environment: &[(String, String)],
) -> io::Result<(Child, ChildStdin, ChildStdout)> {
    let mut command = Command::new(path);
    command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, value) in environment {
        command.env(name, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    match (child.stdin.take(), child.stdout.take()) {
        (Some(stdin), Some(stdout)) => Ok((child, stdin, stdout)),
        _ => {
            kill_and_wait(&mut child);
            Err(io::Error::other("the pipes did not open"))
        }
    }
}

pub fn kill_and_wait(child: &mut Child) {
    if let Err(error) = child.kill() {
        tracing::debug!(%error, "the helper process was already gone");
    }
    for _ in 0..KILL_WAITS {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(KILL_WAIT),
            Err(error) => {
                tracing::debug!(%error, "could not wait for the helper process");
                return;
            }
        }
    }
    tracing::warn!("the helper process did not exit after being killed");
}
