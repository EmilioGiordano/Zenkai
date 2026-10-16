use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

// Shared between the task that runs the agent and the code that closes the chat. The pid
// can arrive after a close was asked for; then it is killed on arrival.
#[derive(Debug, Default)]
pub struct ProcessTree {
    pid: Mutex<Option<u32>>,
    closed: AtomicBool,
}

impl ProcessTree {
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn register(&self, pid: u32) {
        if let Ok(mut slot) = self.pid.lock() {
            *slot = Some(pid);
        }
        if self.is_closed() {
            self.kill();
        }
    }

    pub fn forget(&self) {
        if let Ok(mut slot) = self.pid.lock() {
            *slot = None;
        }
    }

    // Blocks while taskkill runs: call it from a background thread.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.kill();
    }

    fn kill(&self) {
        let pid = self.pid.lock().ok().and_then(|mut slot| slot.take());
        if let Some(pid) = pid
            && let Err(error) = kill_tree(pid)
        {
            tracing::warn!(%error, pid, "could not stop the agent process tree");
        }
    }
}

// Windows has no process groups that std can kill, and a job object needs unsafe, so the
// agent, node and anything node started are ended with the system's own taskkill.
#[cfg(windows)]
fn kill_tree(pid: u32) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let taskkill = std::path::PathBuf::from(root)
        .join("System32")
        .join("taskkill.exe");
    let output = std::process::Command::new(taskkill)
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if !output.status.success() {
        // taskkill fails when the process already ended, which is the goal.
        tracing::debug!(
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "taskkill reported a failure"
        );
    }
    Ok(())
}

#[cfg(not(windows))]
fn kill_tree(pid: u32) -> std::io::Result<()> {
    // A pid outside pid_t wraps to -1 or 0, and `kill -KILL -1` signals every process of the user.
    let pid = match i32::try_from(pid) {
        Ok(pid) if pid > 0 => pid,
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{pid} is not a valid process id"),
            ));
        }
    };
    let output = std::process::Command::new("kill")
        .args(["-KILL", "--", &pid.to_string()])
        .output()?;
    if !output.status.success() {
        // kill fails when the process already ended, which is the goal.
        tracing::debug!(
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "kill reported a failure"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    struct KillOnDrop(Child);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            self.0.kill().ok();
            self.0.wait().ok();
        }
    }

    fn spawn_long_lived_child() -> KillOnDrop {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("ping");
            command.args(["-n", "30", "127.0.0.1"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(KillOnDrop)
            .unwrap()
    }

    #[test]
    fn a_pid_registered_after_the_close_is_killed_at_once() {
        let mut child = spawn_long_lived_child();
        let tree = ProcessTree::default();
        tree.close();
        assert!(tree.is_closed());
        tree.register(child.0.id());
        assert!(tree.pid.lock().unwrap().is_none());

        let deadline = Instant::now() + Duration::from_secs(10);
        let exited = loop {
            if child.0.try_wait().unwrap().is_some() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(
            exited,
            "the registered child was still running after 10 seconds"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_pid_that_is_not_a_positive_pid_t_is_rejected_before_running_kill() {
        for pid in [u32::MAX, i32::MAX as u32 + 1, 0] {
            let error = kill_tree(pid).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
    }
}
