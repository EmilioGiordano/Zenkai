use std::collections::BTreeMap;

use gpui_kit::*;
use zenkai_agent::chat::launch::LaunchApprovals;
use zenkai_agent::detect::{self, Detection};
use zenkai_agent::secrets::{SecretStatus, Secrets};
use zenkai_agent::settings::{
    AgentId, AgentServer, SecretName, Settings, SettingsState, escalations,
};
use zenkai_agent::settings_file::{self, SettingsFileError, SettingsPaths, SettingsWatcher};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretState {
    Known(SecretStatus),
    Unavailable(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BridgeStatus {
    #[default]
    Off,
    Listening,
    Failed(String),
}

// The agent settings in force for the whole app, kept up to date by the file watcher.
#[derive(Default)]
pub struct AgentConfig {
    pub paths: Option<SettingsPaths>,
    pub state: SettingsState,
    pub detection: Option<Detection>,
    pub secrets: BTreeMap<SecretName, SecretState>,
    // The last failure of something the user asked for (saving, storing a secret).
    pub failure: Option<String>,
    pub bridge: BridgeStatus,
    pub approvals: LaunchApprovals,
    watcher: Option<SettingsWatcher>,
}

impl Global for AgentConfig {}

pub fn init(cx: &mut App) {
    cx.set_global(AgentConfig::default());
    let (sender, reloads) = async_channel::unbounded();
    cx.spawn(async move |cx| {
        let started = cx
            .background_executor()
            .spawn(async move {
                let paths = SettingsPaths::from_environment()?;
                let loaded = settings_file::prepare(&paths);
                let watcher = settings_file::watch(&paths, move |loaded| {
                    if sender.send_blocking(loaded).is_err() {
                        tracing::debug!("settings reload arrived after the app closed");
                    }
                });
                Ok::<_, SettingsFileError>((paths, loaded, watcher))
            })
            .await;
        cx.update_global::<AgentConfig, _>(|config, _| match started {
            Ok((paths, loaded, watcher)) => {
                config.paths = Some(paths);
                match loaded {
                    Ok(settings) => config.state.apply_file(Ok(settings)),
                    Err(error) => {
                        tracing::warn!(%error, "could not prepare the settings folder");
                        config.failure = Some(error.to_string());
                    }
                }
                match watcher {
                    Ok(watcher) => config.watcher = Some(watcher),
                    Err(error) => tracing::warn!(%error, "settings will not reload by themselves"),
                }
            }
            Err(error) => {
                tracing::warn!(%error, "no settings folder");
                config.failure = Some(error.to_string());
            }
        });
        refresh_secrets(cx);
        while let Ok(loaded) = reloads.recv().await {
            cx.update_global::<AgentConfig, _>(|config, _| config.state.apply_file(loaded));
            refresh_secrets(cx);
        }
    })
    .detach();
}

// Window focus is a second chance to pick up an edit the watcher missed.
pub fn reload(cx: &mut App) {
    let Some(file) = cx
        .global::<AgentConfig>()
        .paths
        .as_ref()
        .map(SettingsPaths::settings)
    else {
        return;
    };
    cx.spawn(async move |cx| {
        let loaded = cx
            .background_executor()
            .spawn(async move { settings_file::load(&file) })
            .await;
        let changed = cx.update_global::<AgentConfig, _>(|config, _| {
            let before = config.state.clone();
            config.state.apply_file(loaded);
            config.state != before
        });
        if changed {
            refresh_secrets(cx);
        }
    })
    .detach();
}

pub fn detect_agents(cx: &mut App) {
    cx.spawn(async move |cx| {
        let detection = cx
            .background_executor()
            .spawn(async move {
                let search_path = detect::search_path();
                detect::detect(
                    |name| detect::find_in(&search_path, name),
                    detect::run_version,
                )
            })
            .await;
        cx.update_global::<AgentConfig, _>(|config, _| config.detection = Some(detection));
    })
    .detach();
}

fn refresh_secrets(cx: &mut AsyncApp) {
    let names = cx.read_global::<AgentConfig, _>(|config, _| config.state.current.secret_names());
    cx.spawn(async move |cx| {
        let states = cx
            .background_executor()
            .spawn(async move { secret_states(&names) })
            .await;
        cx.update_global::<AgentConfig, _>(|config, _| config.secrets = states);
    })
    .detach();
}

fn secret_states(names: &[SecretName]) -> BTreeMap<SecretName, SecretState> {
    let secrets = Secrets::platform();
    names
        .iter()
        .map(|name| {
            let state = match &secrets {
                Ok(secrets) => match secrets.status(name) {
                    Ok(status) => SecretState::Known(status),
                    Err(error) => SecretState::Unavailable(error.to_string()),
                },
                Err(error) => SecretState::Unavailable(error.to_string()),
            };
            (name.clone(), state)
        })
        .collect()
}

// What the user clicked on the Settings page is their own choice, so a change that
// gives agents more power is approved here and never held like a file edit.
pub fn change(cx: &mut App, edit: impl Fn(&mut Settings) + Send + 'static) {
    let config = cx.global::<AgentConfig>();
    let Some(paths) = config.paths.clone() else {
        return;
    };
    let current = config.state.current.clone();
    let mut edited = current.clone();
    edit(&mut edited);
    let approved = escalations(&current, &edited);
    let launches = servers_changed(&current, &edited);
    cx.spawn(async move |cx| {
        let written = cx
            .background_executor()
            .spawn(async move { settings_file::update(&paths, edit) })
            .await;
        cx.update_global::<AgentConfig, _>(|config, _| match written {
            Ok(settings) => {
                config.state.apply_from_page(settings, &approved);
                for (id, server) in &launches {
                    config.approvals.approve(id, server, None);
                }
                config.failure = None;
            }
            Err(error) => config.failure = Some(format!("Settings were not changed: {error}")),
        });
        refresh_secrets(cx);
    })
    .detach();
}

pub fn store_secret(cx: &mut App, name: SecretName, value: String) {
    cx.spawn(async move |cx| {
        let stored = cx
            .background_executor()
            .spawn(async move { Secrets::platform()?.set(&name, &value) })
            .await;
        cx.update_global::<AgentConfig, _>(|config, _| {
            config.failure = stored.err().map(|error| error.to_string());
        });
        refresh_secrets(cx);
    })
    .detach();
}

// Only the servers a click changed count as approved; a file edit that is still waiting
// stays unconfirmed even though the page writes the whole file.
fn servers_changed(current: &Settings, edited: &Settings) -> Vec<(AgentId, AgentServer)> {
    edited
        .agents
        .servers
        .iter()
        .filter(|(id, server)| current.agents.servers.get(*id) != Some(*server))
        .map(|(id, server)| (id.clone(), server.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use zenkai_agent::settings::Settings;

    use super::servers_changed;

    fn settings(json: &str) -> Settings {
        Settings::parse(json).unwrap()
    }

    #[test]
    fn only_servers_the_edit_added_or_changed_are_approved() {
        let current = settings(
            r#"{"agents": {"servers": {"kept": {"name": "K", "command": "a"},
                "edited": {"name": "E", "command": "b"}}}}"#,
        );
        let edited = settings(
            r#"{"agents": {"servers": {"kept": {"name": "K", "command": "a"},
                "edited": {"name": "E", "command": "c"},
                "new": {"name": "N", "command": "d"}}}}"#,
        );
        let ids: Vec<String> = servers_changed(&current, &edited)
            .into_iter()
            .map(|(id, _)| id.to_string())
            .collect();
        assert_eq!(ids, ["edited", "new"]);
    }

    #[test]
    fn a_click_that_changes_no_server_approves_nothing() {
        let current = settings(r#"{"agents": {"servers": {"a": {"name": "A", "command": "x"}}}}"#);
        assert!(servers_changed(&current, &current.clone()).is_empty());
    }
}
