use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use gpui_kit::*;
use zenkai_types::Language;

use crate::close_prompt::CloseGoal;
use crate::view::Workspace;
use crate::{agent_settings, language};

const RESTARTED_FLAG: &str = "--restarted";
const PARENT_EXIT_LIMIT: Duration = Duration::from_secs(15);
// The parent's handles close in one teardown, but the lock files may lag it by a moment.
const AFTER_PARENT_EXIT: Duration = Duration::from_millis(300);
#[cfg(windows)]
const DETACHED_NEW_GROUP: u32 = 0x0000_0008 | 0x0000_0200;

struct MainWorkspace {
    window: AnyWindowHandle,
    workspace: WeakEntity<Workspace>,
}

impl Global for MainWorkspace {}

pub fn register(window: AnyWindowHandle, workspace: &Entity<Workspace>, cx: &mut App) {
    cx.set_global(MainWorkspace {
        window,
        workspace: workspace.downgrade(),
    });
}

pub fn language_change_pending(cx: &App) -> bool {
    differs(
        zenkai_i18n::active().language(),
        agent_settings::settings(cx).language,
        language::system,
    )
}

fn differs(running: Language, chosen: Option<Language>, system: impl FnOnce() -> Language) -> bool {
    chosen.unwrap_or_else(system) != running
}

pub fn was_restarted() -> bool {
    std::env::args_os().any(|argument| argument == RESTARTED_FLAG)
}

pub fn file_argument() -> Option<PathBuf> {
    first_file(std::env::args_os().skip(1))
}

fn first_file(arguments: impl Iterator<Item = OsString>) -> Option<PathBuf> {
    arguments
        .into_iter()
        .find(|argument| argument != RESTARTED_FLAG)
        .map(PathBuf::from)
}

pub fn request(cx: &mut App) {
    if !language_change_pending(cx) {
        return;
    }
    let Some(main) = cx.try_global::<MainWorkspace>() else {
        return;
    };
    let (window, workspace) = (main.window, main.workspace.clone());
    #[cfg(windows)]
    crate::tray::request_show(cx);
    let updated = window.update(cx, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.request_close(CloseGoal::Restart, window, cx)
        })
    });
    if let Ok(Err(error)) | Err(error) = updated {
        tracing::debug!(%error, "the main window is gone; not restarting");
    }
}

// Runs once the session is saved. The new process is told by its flag to restore the whole
// session even when "restore last session" is off. It gets the read end of a pipe whose write
// end only this process holds, so it learns the old one has exited (and released the session
// lock and the instance pipe) when that pipe reaches end of file.
pub fn relaunch() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            tracing::error!(%error, "could not find this executable; not restarting");
            return;
        }
    };
    let mut command = Command::new(exe);
    command
        .arg(RESTARTED_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, DETACHED_NEW_GROUP);
    match command.spawn() {
        // Dropping the handle would close the pipe now instead of when this process ends.
        Ok(mut child) => std::mem::forget(child.stdin.take()),
        Err(error) => tracing::error!(%error, "could not start the new instance"),
    }
}

pub fn wait_for_previous_instance() {
    if !was_restarted() {
        return;
    }
    if wait_for_end(std::io::stdin(), PARENT_EXIT_LIMIT) == Wait::Timeout {
        tracing::warn!("the previous instance did not exit in time; starting anyway");
        return;
    }
    std::thread::sleep(AFTER_PARENT_EXIT);
}

#[derive(Debug, PartialEq, Eq)]
enum Wait {
    Ended,
    Timeout,
}

fn wait_for_end(mut source: impl Read + Send + 'static, limit: Duration) -> Wait {
    let (ended, wait) = mpsc::channel();
    std::thread::spawn(move || {
        let mut sink = [0u8; 64];
        while matches!(source.read(&mut sink), Ok(read) if read > 0) {}
        if ended.send(()).is_err() {
            tracing::debug!("the restart wait gave up before the pipe ended");
        }
    });
    match wait.recv_timeout(limit) {
        Ok(()) => Wait::Ended,
        Err(_) => Wait::Timeout,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::io::Read;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;

    use zenkai_types::Language;

    use super::{Wait, differs, first_file, wait_for_end};

    #[test]
    fn a_chosen_language_other_than_the_running_one_needs_a_restart() {
        let (en, es) = (Language::English, Language::Spanish);
        assert!(differs(en, Some(es), || en));
        assert!(!differs(en, Some(en), || es));
    }

    #[test]
    fn following_windows_compares_the_system_language() {
        let (en, es) = (Language::English, Language::Spanish);
        assert!(!differs(es, None, || es));
        assert!(differs(en, None, || es));
    }

    #[test]
    fn the_system_language_is_not_looked_up_when_one_is_chosen() {
        assert!(!differs(Language::English, Some(Language::English), || {
            unreachable!("the system language was read")
        }));
    }

    #[test]
    fn the_restart_flag_is_never_taken_for_a_file() {
        let arguments = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(first_file(arguments(&["--restarted"]).into_iter()), None);
        assert_eq!(
            first_file(arguments(&["--restarted", "a.xlsx"]).into_iter()),
            Some(PathBuf::from("a.xlsx"))
        );
        assert_eq!(
            first_file(arguments(&["a.xlsx", "--restarted"]).into_iter()),
            Some(PathBuf::from("a.xlsx"))
        );
    }

    #[test]
    fn the_wait_ends_when_the_pipe_closes() {
        assert_eq!(
            wait_for_end(std::io::empty(), Duration::from_secs(5)),
            Wait::Ended
        );
    }

    #[test]
    fn the_wait_gives_up_while_the_pipe_stays_open() {
        struct Never(mpsc::Receiver<()>);
        impl Read for Never {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                self.0.recv().ok();
                Ok(0)
            }
        }
        let (keep_open, never) = mpsc::channel();
        let waited = wait_for_end(Never(never), Duration::from_millis(50));
        assert_eq!(waited, Wait::Timeout);
        drop(keep_open);
    }
}
