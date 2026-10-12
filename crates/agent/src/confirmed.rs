use crate::secrets::Secrets;
use crate::settings::{Escalation, SecretName};
use crate::settings_file::SettingsPaths;

const RECORD_NAME: &str = "zenkai-internal/confirmed-at-start";

fn record_name() -> Result<SecretName, String> {
    SecretName::try_from(RECORD_NAME.to_string())
}

// Kept in Credential Manager, not in the settings folder: a file there can be forged by any
// program that can write to the user's profile. Anything unreadable means nothing was
// confirmed, which is the safe side: elevated settings are held for the user as usual.
pub fn load(secrets: &Secrets, paths: &SettingsPaths) -> Vec<Escalation> {
    let legacy = paths.legacy_remembered();
    if legacy.exists()
        && let Err(error) = std::fs::remove_file(&legacy)
    {
        tracing::warn!(%error, "could not delete the old record of confirmed settings");
    }
    let read =
        record_name().and_then(|name| secrets.read(&name).map_err(|error| error.to_string()));
    match read {
        Ok(Some(text)) => serde_json::from_str(&text).unwrap_or_else(|error| {
            tracing::warn!(%error, "ignoring the record of confirmed settings");
            Vec::new()
        }),
        Ok(None) => Vec::new(),
        Err(error) => {
            tracing::warn!(%error, "could not read the record of confirmed settings");
            Vec::new()
        }
    }
}

pub fn save(secrets: &Secrets, confirmed: &[Escalation]) -> Result<(), String> {
    let text = serde_json::to_string(confirmed).map_err(|error| error.to_string())?;
    secrets
        .set(&record_name()?, &text)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets() -> Secrets {
        Secrets::with_store(keyring_core::mock::Store::new().unwrap())
    }

    fn paths(dir: &tempfile::TempDir) -> SettingsPaths {
        SettingsPaths {
            folder: dir.path().to_path_buf(),
        }
    }

    #[test]
    fn confirmations_are_stored_in_the_secret_store_and_read_back() {
        let (dir, secrets) = (tempfile::tempdir().unwrap(), secrets());
        let paths = paths(&dir);
        assert_eq!(load(&secrets, &paths), []);
        let confirmed = [Escalation::WriteWithoutAsking];
        save(&secrets, &confirmed).unwrap();
        assert_eq!(load(&secrets, &paths), confirmed);
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn a_broken_record_confirms_nothing() {
        let (dir, secrets) = (tempfile::tempdir().unwrap(), secrets());
        secrets
            .set(&record_name().unwrap(), "[\"something else\"]")
            .unwrap();
        assert_eq!(load(&secrets, &paths(&dir)), []);
    }

    #[test]
    fn a_forged_file_in_the_settings_folder_confirms_nothing_and_is_deleted() {
        let (dir, secrets) = (tempfile::tempdir().unwrap(), secrets());
        let paths = paths(&dir);
        std::fs::write(paths.legacy_remembered(), "[\"write_without_asking\"]").unwrap();
        assert_eq!(load(&secrets, &paths), []);
        assert!(!paths.legacy_remembered().exists());
    }
}
