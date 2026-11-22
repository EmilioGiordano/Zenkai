use std::path::Path;

use winsafe::{HKEY, RegistryValue, co};

use super::VERSION;

// The key Inno Setup writes for the AppId in packaging/windows/zenkai.iss, per user.
const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{20C0FE1B-59E0-4985-8377-20D8CBD20083}_is1";

#[derive(Debug, PartialEq, Eq)]
enum Recorded {
    NotInstalled,
    OtherCopy,
    AlreadyCurrent,
    Updated,
}

// The installer records the version it installed; after an update or a restore the app keeps
// the list of programs truthful. A portable copy, or another install, is left alone.
pub fn record_version(root: &Path) {
    match recorded_version(root) {
        Ok(Recorded::Updated) => tracing::info!("updated the version in the list of programs"),
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "could not update the version in the list of programs")
        }
    }
}

fn recorded_version(root: &Path) -> Result<Recorded, co::ERROR> {
    let opened = HKEY::CURRENT_USER.RegOpenKeyEx(
        Some(KEY),
        co::REG_OPTION::default(),
        co::KEY::QUERY_VALUE | co::KEY::SET_VALUE,
    );
    let key = match opened {
        Ok(key) => key,
        Err(error) if error == co::ERROR::FILE_NOT_FOUND => return Ok(Recorded::NotInstalled),
        Err(error) => return Err(error),
    };
    let location = match key.RegQueryValueEx(Some("InstallLocation")) {
        Ok(RegistryValue::Sz(text) | RegistryValue::ExpandSz(text)) => text,
        Ok(_) => return Ok(Recorded::OtherCopy),
        Err(error) if error == co::ERROR::FILE_NOT_FOUND => return Ok(Recorded::OtherCopy),
        Err(error) => return Err(error),
    };
    if !same_folder(Path::new(&location), root) {
        return Ok(Recorded::OtherCopy);
    }
    match key.RegQueryValueEx(Some("DisplayVersion")) {
        Ok(RegistryValue::Sz(current)) if current == VERSION => {
            return Ok(Recorded::AlreadyCurrent);
        }
        Ok(_) => {}
        Err(error) if error == co::ERROR::FILE_NOT_FOUND => {}
        Err(error) => return Err(error),
    }
    key.RegSetValueEx(
        Some("DisplayVersion"),
        RegistryValue::Sz(VERSION.to_owned()),
    )?;
    Ok(Recorded::Updated)
}

// A folder that no longer exists is not this install.
fn same_folder(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::same_folder;

    #[test]
    fn the_install_location_matches_whatever_its_case_or_trailing_separator() {
        let folder = tempfile::tempdir().unwrap();
        let written = format!("{}\\", folder.path().display().to_string().to_uppercase());
        assert!(same_folder(std::path::Path::new(&written), folder.path()));
    }

    #[test]
    fn another_or_a_missing_folder_is_not_this_install() {
        let one = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        assert!(!same_folder(one.path(), other.path()));
        assert!(!same_folder(&one.path().join("gone"), one.path()));
    }
}
