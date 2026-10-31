use std::io;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

const KILL_WAIT: Duration = Duration::from_millis(50);
const KILL_WAITS: u32 = 40;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// Starts a helper process with stdin and stdout piped and no console window. Shared by every
// helper the app talks to over frames, so they start and stop the same way.
pub fn spawn_piped(
    path: &Path,
    arguments: &[String],
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

// Kills the process and waits a little for it to go; one that will not go is left to the system.
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
