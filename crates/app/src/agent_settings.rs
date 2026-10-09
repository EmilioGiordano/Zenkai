use std::collections::BTreeMap;

use gpui_kit::*;
use zenkai_agent::chat::launch::LaunchApprovals;
use zenkai_agent::confirmed;
use zenkai_agent::detect::{self, Detection};
use zenkai_agent::secrets::{SecretStatus, Secrets};
use zenkai_agent::settings::{
    AgentId, AgentServer, Escalation, HeldChange, SecretName, Settings, SettingsState, escalations,
    remembered_confirmations,
};
use zenkai_agent::settings_file::{self, SettingsFileError, SettingsPaths, SettingsWatcher};
use zenkai_i18n::t;

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

impl AgentConfig {
    pub fn settings(&self) -> &Settings {
        &self.state.current
    }
}

// The confirmations on disk, so the file is rewritten only when what is in force changes.
struct Remembered(Vec<Escalation>);

impl Global for Remembered {}

pub fn settings(cx: &App) -> &Settings {
    cx.global::<AgentConfig>().settings()
}

pub fn init(cx: &mut App) {
    cx.set_global(AgentConfig::default());
    cx.set_global(Remembered(Vec::new()));
    cx.observe_global::<AgentConfig>(|cx| {
        crate::theme::sync_with_settings(cx);
        remember_confirmations(cx);
    })
    .detach();
    let (sender, reloads) = async_channel::unbounded();
    cx.spawn(async move |cx| {
        let started = cx
            .background_executor()
            .spawn(async move {
                let paths = SettingsPaths::from_environment()?;
                let loaded = settings_file::prepare(&paths);
                let remembered = match Secrets::platform() {
                    Ok(secrets) => confirmed::load(&secrets, &paths),
                    Err(error) => {
                        tracing::warn!(%error, "no secret store: nothing is confirmed");
                        Vec::new()
                    }
                };
                let watcher = settings_file::watch(&paths, move |loaded| {
                    if sender.send_blocking(loaded).is_err() {
                        tracing::debug!("settings reload arrived after the app closed");
                    }
                });
                Ok::<_, SettingsFileError>((paths, loaded, remembered, watcher))
            })
            .await;
        if let Ok((_, _, remembered, _)) = &started {
            cx.update(|cx| cx.set_global(Remembered(remembered.clone())));
        }
        cx.update_global::<AgentConfig, _>(|config, _| match started {
            Ok((paths, loaded, remembered, watcher)) => {
                config.paths = Some(paths);
                match loaded {
                    Ok(settings) => config.state.apply_startup(Ok(settings), &remembered),
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
        let written_failed = written.is_err();
        cx.update_global::<AgentConfig, _>(|config, _| match written {
            Ok(settings) => {
                config.state.apply_from_page(settings, &approved);
                for (id, server) in &launches {
                    config.approvals.approve(id, server, None);
                }
                config.failure = None;
            }
            Err(error) => config.failure = Some(t!("settings.not_changed", error = error)),
        });
        if written_failed {
            cx.update(crate::theme::revert_to_settings);
        }
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

// Written whenever the elevated values in force change, and derived from them: a settings.json
// edit never reaches it unless the user confirmed it. It lives in Credential Manager so a
// program that can only write files cannot forge it.
fn remember_confirmations(cx: &mut App) {
    let config = cx.global::<AgentConfig>();
    if config.paths.is_none() {
        return;
    }
    let confirmed = remembered_confirmations(&config.state.current);
    if confirmed == cx.global::<Remembered>().0 {
        return;
    }
    cx.set_global(Remembered(confirmed.clone()));
    cx.spawn(async move |cx| {
        let saved = cx
            .background_executor()
            .spawn(async move {
                Secrets::platform()
                    .map_err(|error| error.to_string())
                    .and_then(|secrets| confirmed::save(&secrets, &confirmed))
            })
            .await;
        if let Err(error) = saved {
            tracing::warn!(%error, "could not record the confirmed settings");
            cx.update_global::<AgentConfig, _>(|config, _| {
                config.failure = Some(t!("settings.confirmation_not_stored", error = error))
            });
        }
    })
    .detach();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldDecision {
    Apply,
    Keep,
}

// Applies only the change the user was shown: a newer file change may have replaced it since.
pub fn decide_held(cx: &mut App, shown: Option<HeldChange>, decision: HeldDecision) {
    cx.update_global::<AgentConfig, _>(|config, _| {
        if config.state.held != shown {
            return;
        }
        match decision {
            HeldDecision::Apply => config.state.accept_held(),
            HeldDecision::Keep => config.state.decline_held(),
        }
    });
}

pub fn held_summary(held: &HeldChange) -> String {
    let asks: Vec<&str> = held.escalations.iter().map(|e| e.label()).collect();
    t!(
        "held.summary",
        asks = asks.join(&format!(" {} ", t!("held.and")))
    )
}

// Only the servers a click changed count as approved; a file edit that is still waiting
// stays unconfirmed even though the Settings window writes the whole file.
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
