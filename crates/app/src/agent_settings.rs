use std::collections::BTreeMap;

use gpui_kit::*;
use zenkai_agent::detect::{self, Detection};
use zenkai_agent::secrets::{SecretStatus, Secrets};
use zenkai_agent::settings::{SecretName, Settings, SettingsState};
use zenkai_agent::settings_file::{self, SettingsFileError, SettingsPaths, SettingsWatcher};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretState {
    Known(SecretStatus),
    Unavailable(String),
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
                    Ok(settings) => config.state.apply(Ok(settings)),
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
            cx.update_global::<AgentConfig, _>(|config, _| config.state.apply(loaded));
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
            config.state.apply(loaded);
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

// A file that does not parse is the user's (or an agent's) unfinished edit: writing the
// last good settings over it would throw that edit away.
pub fn change(cx: &mut App, edit: impl FnOnce(&mut Settings)) {
    let config = cx.global::<AgentConfig>();
    let (problem, paths) = (config.state.problem.clone(), config.paths.clone());
    let mut settings = config.state.current.clone();
    if let Some(problem) = problem {
        cx.update_global::<AgentConfig, _>(|config, _| {
            config.failure = Some(format!(
                "Fix settings.json before changing settings here: {problem}"
            ));
        });
        return;
    }
    let Some(paths) = paths else {
        return;
    };
    edit(&mut settings);
    cx.spawn(async move |cx| {
        let saved = settings.clone();
        let written = cx
            .background_executor()
            .spawn(async move { settings_file::save(&paths, &saved) })
            .await;
        cx.update_global::<AgentConfig, _>(|config, _| match written {
            Ok(()) => {
                config.state.apply(Ok(settings));
                config.failure = None;
            }
            Err(error) => config.failure = Some(error.to_string()),
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
