use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("could not create a random folder name: {0}")]
    Random(String),
    #[error("could not create the agent working folder {path}: {source}")]
    WorkFolder {
        path: PathBuf,
        source: std::io::Error,
    },
}

// The folder an agent runs in: new, empty and outside every workbook's folder, so an agent
// that obeys text planted in a cell finds no files next to it.
#[derive(Debug)]
pub struct WorkFolder {
    path: PathBuf,
}

impl WorkFolder {
    pub fn create() -> Result<WorkFolder, ProcessError> {
        let mut random = [0u8; 8];
        getrandom::fill(&mut random).map_err(|error| ProcessError::Random(error.to_string()))?;
        let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("zenkai-agent-{name}"));
        // create_dir fails when the name exists, so a planted folder is never reused.
        std::fs::create_dir(&path).map_err(|source| ProcessError::WorkFolder {
            path: path.clone(),
            source,
        })?;
        Ok(WorkFolder { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorkFolder {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            tracing::warn!(%error, path = %self.path.display(), "could not remove the agent working folder");
        }
    }
}

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
    let taskkill = PathBuf::from(root).join("System32").join("taskkill.exe");
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
    std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .output()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_work_folder_is_new_empty_and_removed_on_drop() {
        let first = WorkFolder::create().unwrap();
        let second = WorkFolder::create().unwrap();
        assert_ne!(first.path(), second.path());
        assert_eq!(std::fs::read_dir(first.path()).unwrap().count(), 0);
        let path = first.path().to_path_buf();
        drop(first);
        assert!(!path.exists());
    }

    #[test]
    fn a_pid_registered_after_the_close_is_killed_at_once() {
        let tree = ProcessTree::default();
        tree.close();
        assert!(tree.is_closed());
        tree.register(u32::MAX);
        assert!(tree.pid.lock().unwrap().is_none());
    }
}
