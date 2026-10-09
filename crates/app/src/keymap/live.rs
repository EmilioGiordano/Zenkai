use std::collections::BTreeMap;
use std::rc::Rc;

use gpui_kit::*;
use serde_json::Value;

use super::chord::{Chord, display};
use super::conflict::{self, Claim, Conflict, Scope};
use super::defaults::Binding;
use super::edit::{self, Edit};
use super::model::Model;
use super::overrides::{Overrides, Problem, validate};
use crate::agent_settings;

// What settings.json asks for, as last applied to the app's key bindings.
#[derive(Default)]
pub struct KeymapState {
    raw: BTreeMap<String, Value>,
    pub overrides: Overrides,
    pub problems: Vec<Problem>,
}

impl Global for KeymapState {}

pub fn init(cx: &mut App) {
    cx.set_global(KeymapState::default());
    rebuild(cx, &Model::build(), &Overrides::default());
}

// Runs on every change of the settings in force and does work only when the keymap section
// differs from the one already applied.
pub fn sync_with_settings(cx: &mut App) {
    let raw = agent_settings::settings(cx).keymap.clone();
    if cx.global::<KeymapState>().raw == raw {
        return;
    }
    let model = Model::build();
    let (overrides, problems) = validate(&model, &raw, &foreign_claims(cx, &model));
    rebuild(cx, &model, &overrides);
    cx.set_global(KeymapState {
        raw,
        overrides,
        problems,
    });
}

// The keymap is rebuilt from what the live one holds that is not ours (cell navigation,
// widget keys, possibly added after startup), plus our own bindings with the overrides.
fn rebuild(cx: &mut App, model: &Model, overrides: &Overrides) {
    let owned = model.owned_names();
    let others: Vec<KeyBinding> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter(|binding| !owned.contains(binding.action().name()))
        .cloned()
        .collect();
    let ours = model.effective(overrides).into_iter().filter_map(load);
    let rebuilt: Vec<KeyBinding> = others.into_iter().chain(ours).collect();
    cx.clear_key_bindings();
    cx.bind_keys(rebuilt);
}

fn load(binding: Binding) -> Option<KeyBinding> {
    let predicate = match binding.context.map(KeyBindingContextPredicate::parse) {
        Some(Ok(predicate)) => Some(Rc::new(predicate)),
        Some(Err(error)) => {
            tracing::error!(%error, "a shortcut context does not parse");
            return None;
        }
        None => None,
    };
    match KeyBinding::load(
        binding.chord.as_str(),
        binding.action,
        predicate,
        false,
        None,
        &DummyKeyboardMapper,
    ) {
        Ok(loaded) => Some(loaded),
        Err(error) => {
            tracing::error!(%error, "a shortcut does not load");
            None
        }
    }
}

// Cell navigation applies inside the window whatever the user binds, and widget keys such as
// text editing live deeper still, so only the former and global keys count as taken.
fn foreign_claims(cx: &App, model: &Model) -> Vec<Claim> {
    let owned = model.owned_names();
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    keymap
        .bindings()
        .filter(|binding| !owned.contains(binding.action().name()))
        .filter(|binding| !binding.action().as_any().is::<NoAction>())
        .filter(|binding| {
            binding
                .predicate()
                .is_none_or(|predicate| predicate.to_string().contains("Grid"))
        })
        .filter_map(|binding| {
            let [stroke] = binding.keystrokes() else {
                return None;
            };
            Some(Claim {
                chord: Chord::from_keystroke(stroke.inner()).ok()?,
                owner: binding.action().name().to_string(),
                scope: Scope::Both,
                editable: false,
            })
        })
        .collect()
}

// The label with the shortcut the command has right now, for tooltips and buttons.
pub fn labeled(cx: &App, label: &str, action: &dyn Action) -> String {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let keys = keymap
        .bindings_for_action(action)
        .next()
        .and_then(|binding| binding.keystrokes().first())
        .map(|stroke| display(stroke.inner()));
    match keys {
        Some(keys) => format!("{label} ({keys})"),
        None => label.to_string(),
    }
}

pub struct Proposal {
    pub command: &'static str,
    pub chord: Chord,
    pub conflict: Option<Conflict>,
}

pub fn propose(cx: &App, command: &'static str, chord: Chord) -> Proposal {
    let model = Model::build();
    let overrides = &cx.global::<KeymapState>().overrides;
    let claims = model.claims(overrides, &foreign_claims(cx, &model));
    let scopes = model
        .command(command)
        .map(|known| known.scopes())
        .unwrap_or_default();
    Proposal {
        command,
        conflict: conflict::find(&claims, command, &scopes, &chord),
        chord,
    }
}

pub fn commit(cx: &mut App, proposal: &Proposal) {
    let model = Model::build();
    let overrides = &cx.global::<KeymapState>().overrides;
    let mut edits: Vec<Edit> = Vec::new();
    if let Some(Conflict::Replaceable(others)) = &proposal.conflict {
        for name in others {
            let Some(other) = model.command(name) else {
                continue;
            };
            let mut keys = model.keys(other, overrides);
            keys.retain(|chord| *chord != proposal.chord);
            edits.push(Edit::Set {
                command: other.name,
                keys,
            });
        }
    }
    edits.push(Edit::Set {
        command: proposal.command,
        keys: vec![proposal.chord.clone()],
    });
    edit(cx, edits);
}

pub fn remove(cx: &mut App, command: &'static str) {
    edit(
        cx,
        vec![Edit::Set {
            command,
            keys: Vec::new(),
        }],
    );
}

pub fn reset(cx: &mut App, command: &'static str) {
    edit(cx, vec![Edit::Reset { command }]);
}

pub fn reset_all(cx: &mut App) {
    edit(cx, vec![Edit::ResetAll]);
}

fn edit(cx: &mut App, edits: Vec<Edit>) {
    agent_settings::change(cx, move |settings| {
        edit::apply(&Model::build(), &mut settings.keymap, &edits)
    });
}
