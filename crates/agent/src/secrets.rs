use std::sync::Arc;

use keyring_core::{CredentialStore, Entry};

use crate::settings::SecretName;

const SERVICE: &str = "zenkai";

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("secrets are stored in Windows Credential Manager, which this system does not have")]
    Unsupported,
    #[error("Credential Manager refused the secret \"{name}\": {source}")]
    Store {
        name: SecretName,
        source: keyring_core::Error,
    },
    #[error("could not open Credential Manager: {0}")]
    Open(keyring_core::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretStatus {
    Stored,
    Missing,
}

// Values never leave this type except to the agent process environment, and are never
// logged.
#[derive(Clone)]
pub struct Secrets {
    store: Arc<CredentialStore>,
}

impl Secrets {
    #[cfg(windows)]
    pub fn platform() -> Result<Secrets, SecretError> {
        let store = windows_native_keyring_store::Store::new().map_err(SecretError::Open)?;
        Ok(Secrets { store })
    }

    #[cfg(not(windows))]
    pub fn platform() -> Result<Secrets, SecretError> {
        Err(SecretError::Unsupported)
    }

    pub fn with_store(store: Arc<CredentialStore>) -> Secrets {
        Secrets { store }
    }

    fn entry(&self, name: &SecretName) -> Result<Entry, SecretError> {
        self.store
            .build(SERVICE, name.as_str(), None)
            .map_err(|source| SecretError::Store {
                name: name.clone(),
                source,
            })
    }

    pub fn set(&self, name: &SecretName, value: &str) -> Result<(), SecretError> {
        self.entry(name)?
            .set_password(value)
            .map_err(|source| SecretError::Store {
                name: name.clone(),
                source,
            })
    }

    pub fn read(&self, name: &SecretName) -> Result<Option<String>, SecretError> {
        match self.entry(name)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(source) => Err(SecretError::Store {
                name: name.clone(),
                source,
            }),
        }
    }

    pub fn status(&self, name: &SecretName) -> Result<SecretStatus, SecretError> {
        match self.entry(name)?.get_password() {
            Ok(_) => Ok(SecretStatus::Stored),
            Err(keyring_core::Error::NoEntry) => Ok(SecretStatus::Missing),
            Err(source) => Err(SecretError::Store {
                name: name.clone(),
                source,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> SecretName {
        SecretName::try_from(text.to_string()).unwrap()
    }

    #[test]
    fn a_stored_secret_is_reported_without_reading_it_back_out() {
        let secrets = Secrets::with_store(keyring_core::mock::Store::new().unwrap());
        let key = name("zenkai/anthropic");
        assert_eq!(secrets.status(&key).unwrap(), SecretStatus::Missing);
        secrets.set(&key, "sk-test").unwrap();
        assert_eq!(secrets.status(&key).unwrap(), SecretStatus::Stored);
        assert_eq!(
            secrets.status(&name("other")).unwrap(),
            SecretStatus::Missing
        );
    }
}
