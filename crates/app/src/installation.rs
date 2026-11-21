#[cfg(windows)]
mod uninstall_entry;

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use gpui_kit::*;

use crate::view::Workspace;

const APP_FOLDER: &str = "app";
const LAUNCHER: &str = "zenkai.exe";
const OLD_APP: &str = "app.old";
const UPDATE: &str = ".update";
const CONFIRMED: &str = "confirmed";
const MAX_MARKER_BYTES: u64 = 32;
// Ten tries two seconds apart end well inside the launcher's two minutes of waiting.
const CONFIRM_ATTEMPTS: u32 = 10;
const CONFIRM_RETRY: Duration = Duration::from_secs(2);
const VERSION: &str = env!("CARGO_PKG_VERSION");
#[cfg(windows)]
const APP_USER_MODEL_ID: &str = "EmilioGiordano.Zenkai";

// The Windows install keeps the app in `<root>\app\` and the launcher at `<root>\zenkai.exe`.
// Anywhere else, such as a development checkout, there is no root and the updater stays off.
pub fn root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let exe = std::env::current_exe()
            .inspect_err(|error| tracing::warn!(%error, "could not find this executable"))
            .ok()?;
        root_of(&exe)
    })
    .as_deref()
}

fn root_of(exe: &Path) -> Option<PathBuf> {
    let folder = exe.parent()?;
    let root = folder.parent()?;
    let in_app_folder = folder.file_name()?.eq_ignore_ascii_case(APP_FOLDER);
    (in_app_folder && root.join(LAUNCHER).is_file()).then(|| root.to_path_buf())
}

pub fn update_folder() -> Option<PathBuf> {
    Some(root()?.join(UPDATE))
}

// The taskbar groups the app with the shortcuts, which point at the launcher, only when both
// carry the same explicit identity. Must run before the first window.
#[cfg(windows)]
pub fn identify_process() {
    if root().is_none() {
        return;
    }
    if let Err(error) = winsafe::SetCurrentProcessExplicitAppUserModelID(APP_USER_MODEL_ID) {
        tracing::warn!(%error, "could not set the taskbar identity");
    }
}

// A version that cannot confirm its start closes itself, through the normal close with its save
// prompt, so the launcher restores the previous one without ever ending a window the user sees.
pub fn after_first_window(window: AnyWindowHandle, workspace: &Entity<Workspace>, cx: &mut App) {
    let Some(root) = root() else {
        return;
    };
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        if !confirm_with_retries(root, cx).await {
            let closed = window.update(cx, |_, window, cx| {
                workspace.update(cx, |workspace, cx| workspace.close_unconfirmed(window, cx))
            });
            if let Ok(Err(error)) | Err(error) = closed {
                tracing::error!(%error, "the main window is gone; quitting");
                cx.update(|cx| cx.quit());
            }
            return;
        }
        #[cfg(windows)]
        cx.background_executor()
            .spawn(async move { uninstall_entry::record_version(root) })
            .await;
    })
    .detach();
}

async fn confirm_with_retries(root: &'static Path, cx: &mut AsyncApp) -> bool {
    for attempt in 1..=CONFIRM_ATTEMPTS {
        let confirmed = cx
            .background_executor()
            .spawn(async move { confirm_start(root, VERSION) })
            .await;
        match confirmed {
            Ok(Confirmation::Written) => {
                tracing::info!("confirmed that this version starts");
                return true;
            }
            Ok(Confirmation::NotUpdating | Confirmation::AlreadyWritten) => return true,
            Err(error) => {
                tracing::warn!(%error, attempt, "could not confirm that this version starts");
            }
        }
        cx.background_executor().timer(CONFIRM_RETRY).await;
    }
    false
}

#[derive(Debug, PartialEq, Eq)]
enum Confirmation {
    NotUpdating,
    AlreadyWritten,
    Written,
}

// While `app.old` exists the launcher waits for this marker before it drops the previous
// version. It must hold this exact version and be on disk before the launcher reads it.
fn confirm_start(root: &Path, version: &str) -> io::Result<Confirmation> {
    if !root.join(OLD_APP).try_exists()? {
        return Ok(Confirmation::NotUpdating);
    }
    let marker = root.join(UPDATE).join(CONFIRMED);
    match File::open(&marker) {
        Ok(file) => {
            let mut written = Vec::new();
            file.take(MAX_MARKER_BYTES + 1).read_to_end(&mut written)?;
            if written == version.as_bytes() {
                return Ok(Confirmation::AlreadyWritten);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = File::create(&marker)?;
    file.write_all(version.as_bytes())?;
    file.sync_all()?;
    Ok(Confirmation::Written)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{Confirmation, confirm_start, root_of};

    fn install(root: &Path) {
        fs::create_dir_all(root.join("app")).unwrap();
        fs::create_dir_all(root.join(".update")).unwrap();
        fs::write(root.join("zenkai.exe"), b"launcher").unwrap();
        fs::write(root.join("app").join("zenkai.exe"), b"app").unwrap();
    }

    #[test]
    fn the_app_folder_beside_the_launcher_is_the_install() {
        let folder = tempfile::tempdir().unwrap();
        install(folder.path());
        let exe = folder.path().join("app").join("zenkai.exe");
        assert_eq!(root_of(&exe).as_deref(), Some(folder.path()));
        let shouting = folder.path().join("APP").join("zenkai.exe");
        assert_eq!(root_of(&shouting).as_deref(), Some(folder.path()));
    }

    #[test]
    fn a_development_build_is_not_an_install() {
        let folder = tempfile::tempdir().unwrap();
        let release = folder.path().join("target").join("release");
        fs::create_dir_all(&release).unwrap();
        fs::write(release.join("zenkai.exe"), b"app").unwrap();
        fs::write(folder.path().join("target").join("zenkai.exe"), b"x").unwrap();
        assert_eq!(root_of(&release.join("zenkai.exe")), None);
    }

    #[test]
    fn an_app_folder_without_the_launcher_is_not_an_install() {
        let folder = tempfile::tempdir().unwrap();
        install(folder.path());
        fs::remove_file(folder.path().join("zenkai.exe")).unwrap();
        let exe = folder.path().join("app").join("zenkai.exe");
        assert_eq!(root_of(&exe), None);
        fs::create_dir(folder.path().join("zenkai.exe")).unwrap();
        assert_eq!(root_of(&exe), None);
    }

    #[test]
    fn without_an_update_in_progress_nothing_is_written() {
        let folder = tempfile::tempdir().unwrap();
        install(folder.path());
        assert_eq!(
            confirm_start(folder.path(), "0.2.0").unwrap(),
            Confirmation::NotUpdating
        );
        assert!(!folder.path().join(".update").join("confirmed").exists());
    }

    #[test]
    fn a_start_during_an_update_writes_exactly_its_version() {
        let folder = tempfile::tempdir().unwrap();
        install(folder.path());
        fs::create_dir(folder.path().join("app.old")).unwrap();
        let marker = folder.path().join(".update").join("confirmed");
        fs::write(&marker, b"0.1.9").unwrap();
        assert_eq!(
            confirm_start(folder.path(), "0.2.0").unwrap(),
            Confirmation::Written
        );
        assert_eq!(fs::read(&marker).unwrap(), b"0.2.0");
        assert_eq!(
            confirm_start(folder.path(), "0.2.0").unwrap(),
            Confirmation::AlreadyWritten
        );
    }

    #[test]
    fn a_marker_that_cannot_be_written_is_an_error() {
        let folder = tempfile::tempdir().unwrap();
        install(folder.path());
        fs::create_dir(folder.path().join("app.old")).unwrap();
        fs::remove_dir(folder.path().join(".update")).unwrap();
        assert!(confirm_start(folder.path(), "0.2.0").is_err());
    }
}
