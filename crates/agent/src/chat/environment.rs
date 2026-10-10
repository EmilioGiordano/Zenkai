use crate::secrets::{SecretError, Secrets};
use crate::settings::{AgentServer, EnvValue, SecretName};

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("the secret \"{0}\" is not stored. Open Settings (Ctrl+,) and enter its value")]
    MissingSecret(SecretName),
    #[error(transparent)]
    Secrets(#[from] SecretError),
}

// Secret values go into the agent's environment and nowhere else: not the log, not the UI.
pub fn resolve(
    server: &AgentServer,
    open_secrets: impl FnOnce() -> Result<Secrets, SecretError>,
) -> Result<Vec<(String, String)>, EnvironmentError> {
    let mut open_secrets = Some(open_secrets);
    let mut secrets = None;
    let mut resolved = Vec::with_capacity(server.env.len());
    for (name, value) in &server.env {
        let text = match value {
            EnvValue::Text(text) => text.clone(),
            EnvValue::Secret { secret } => {
                if let Some(open) = open_secrets.take() {
                    secrets = Some(open()?);
                }
                let store = secrets.as_ref().ok_or(SecretError::Unsupported)?;
                store
                    .read(secret)?
                    .ok_or_else(|| EnvironmentError::MissingSecret(secret.clone()))?
            }
        };
        resolved.push((name.clone(), text));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn secret(text: &str) -> SecretName {
        SecretName::try_from(text.to_string()).unwrap()
    }

    fn server(env: Vec<(&str, EnvValue)>) -> AgentServer {
        AgentServer {
            name: "Test".to_string(),
            command: "node".to_string(),
            args: Vec::new(),
            env: env
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn store() -> Secrets {
        Secrets::with_store(keyring_core::mock::Store::new().unwrap())
    }

    #[test]
    fn plain_values_pass_through_without_opening_the_store() {
        let server = server(vec![("MODE", EnvValue::Text("fast".to_string()))]);
        let resolved = resolve(&server, || panic!("no secret is needed")).unwrap();
        assert_eq!(resolved, [("MODE".to_string(), "fast".to_string())]);
    }

    #[test]
    fn a_secret_reference_becomes_its_stored_value() {
        let secrets = store();
        secrets.set(&secret("zenkai/key"), "sk-live").unwrap();
        let server = server(vec![(
            "API_KEY",
            EnvValue::Secret {
                secret: secret("zenkai/key"),
            },
        )]);
        let resolved = resolve(&server, || Ok(secrets)).unwrap();
        assert_eq!(resolved, [("API_KEY".to_string(), "sk-live".to_string())]);
    }

    #[test]
    fn a_secret_that_was_never_stored_is_a_clear_error() {
        let server = server(vec![(
            "API_KEY",
            EnvValue::Secret {
                secret: secret("zenkai/missing"),
            },
        )]);
        let error = resolve(&server, || Ok(store())).unwrap_err();
        assert!(matches!(error, EnvironmentError::MissingSecret(_)));
        assert!(error.to_string().contains("zenkai/missing"));
        assert!(!error.to_string().contains("sk-"));
    }
}
