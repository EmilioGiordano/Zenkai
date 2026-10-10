use std::collections::BTreeMap;
use std::rc::Rc;

use gpui_kit::*;
use serde_json::Value;

use super::chord::{Chord, display};
use super::conflict::{self, Claim, Conflict, Scope};
use super::defaults::Binding;
use super::edit::{self, Edit};
use super::model::{Command, Model, is_guarded};
use super::overrides::{Override, Overrides, Problem, validate};
use crate::agent_settings;

// What settings.json asks for, as last applied to the app's key bindings.
#[derive(Default)]
pub struct KeymapState {
    raw: BTreeMap<String, Value>,
    pub overrides: Overrides,
    pub problems: Vec<Problem>,
    // What the user set for guarded commands in this session; the file alone cannot.
    approved: Overrides,
    pub reset_armed: bool,
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
    let approved = cx.global::<KeymapState>().approved.clone();
    let (overrides, problems) = validate(&model, &raw, &foreign_claims(cx, &model), &approved);
    rebuild(cx, &model, &overrides);
    cx.set_global(KeymapState {
        raw,
        overrides,
        problems,
        approved,
        reset_armed: false,
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
                .is_none_or(|predicate| mentions(&predicate, "Grid"))
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

fn mentions(predicate: &KeyBindingContextPredicate, name: &str) -> bool {
    use KeyBindingContextPredicate::*;
    match predicate {
        Identifier(identifier) => identifier == name,
        Descendant(parent, child) => mentions(parent, name) || mentions(child, name),
        And(left, right) | Or(left, right) => mentions(left, name) || mentions(right, name),
        Equal(..) | NotEqual(..) | Not(_) => false,
    }
}

pub struct Proposal {
    pub command: &'static str,
    pub chord: Chord,
    pub conflict: Option<Conflict>,
}

pub fn propose(cx: &App, model: &Model, command: &Command, chord: Chord) -> Proposal {
    let overrides = &cx.global::<KeymapState>().overrides;
    let claims = model.claims(overrides, &foreign_claims(cx, model));
    Proposal {
        command: command.name,
        conflict: conflict::find(&claims, command.name, &command.scopes(), &chord),
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

// The first request arms a confirmation and the second carries it out. Only the entries that
// are in force are removed; what the file holds and was reported as a problem stays.
// Returns whether anything was armed, so the caller can show the Settings window.
pub fn request_reset_all(cx: &mut App) -> bool {
    let state = cx.global::<KeymapState>();
    if state.overrides.len() == 0 {
        return false;
    }
    if state.reset_armed {
        let edits = state
            .overrides
            .iter()
            .map(|(command, _)| Edit::Reset { command })
            .collect();
        edit(cx, edits);
    } else {
        cx.update_global::<KeymapState, _>(|state, _| state.reset_armed = true);
    }
    true
}

pub fn disarm_reset(cx: &mut App) -> bool {
    let armed = cx.global::<KeymapState>().reset_armed;
    if armed {
        cx.update_global::<KeymapState, _>(|state, _| state.reset_armed = false);
    }
    armed
}

fn edit(cx: &mut App, edits: Vec<Edit>) {
    cx.update_global::<KeymapState, _>(|state, _| {
        for edit in &edits {
            match edit {
                Edit::Set { command, keys } if is_guarded(command) => {
                    let replacement = if keys.is_empty() {
                        Override::Removed
                    } else {
                        Override::Keys(keys.clone())
                    };
                    state.approved.insert(command, replacement);
                }
                Edit::Reset { command } => state.approved.remove(command),
                Edit::Set { .. } => {}
            }
        }
        state.reset_armed = false;
    });
    agent_settings::change(cx, move |settings| {
        edit::apply(&Model::build(), &mut settings.keymap, &edits)
    });
}
