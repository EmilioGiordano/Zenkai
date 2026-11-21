use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const APP_FOLDER: &str = "app";
const LAUNCHER: &str = "zenkai.exe";

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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::root_of;

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
}
