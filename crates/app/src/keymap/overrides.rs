use std::collections::BTreeMap;

use serde_json::Value;
use zenkai_i18n::t;

use super::chord::Chord;
use super::conflict::{self, Claim};
use super::model::{Model, is_guarded};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Override {
    Removed,
    Keys(Vec<Chord>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides(BTreeMap<&'static str, Override>);

impl Overrides {
    pub fn get(&self, name: &str) -> Option<&Override> {
        self.0.get(name)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&'static str, &Override)> {
        self.0
            .iter()
            .map(|(name, replacement)| (*name, replacement))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn remove(&mut self, name: &str) {
        self.0.remove(name);
    }

    pub fn insert(&mut self, name: &'static str, replacement: Override) {
        self.0.insert(name, replacement);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    UnknownCommand(String),
    FixedCommand(String),
    Held(String),
    BadEntry(String),
    BadKeystroke {
        command: String,
        text: String,
        reason: &'static str,
    },
    Conflict {
        command: String,
        keys: String,
        other: String,
    },
}

impl Problem {
    pub fn text(&self) -> String {
        match self {
            Problem::UnknownCommand(name) => t!("keymap.problem.unknown_command", name = name),
            Problem::FixedCommand(command) => t!("keymap.problem.fixed_command", command = command),
            Problem::Held(command) => t!("keymap.problem.held", command = command),
            Problem::BadEntry(command) => t!("keymap.problem.bad_entry", command = command),
            Problem::BadKeystroke {
                command,
                text,
                reason,
            } => t!(
                "keymap.problem.bad_keystroke",
                command = command,
                text = text,
                reason = reason
            ),
            Problem::Conflict {
                command,
                keys,
                other,
            } => t!(
                "keymap.problem.conflict",
                command = command,
                keys = keys,
                other = other
            ),
        }
    }
}

enum Parsed {
    Ignored(Problem),
    Accepted(Override),
}

fn parse_entry(command: &str, value: &Value) -> Parsed {
    let texts: Vec<&str> = match value {
        Value::Null => return Parsed::Accepted(Override::Removed),
        Value::Array(items) => {
            let strings: Option<Vec<&str>> = items.iter().map(Value::as_str).collect();
            match strings {
                Some(strings) => strings,
                None => return Parsed::Ignored(Problem::BadEntry(command.to_string())),
            }
        }
        _ => return Parsed::Ignored(Problem::BadEntry(command.to_string())),
    };
    let mut keys: Vec<Chord> = Vec::new();
    for text in texts {
        match Chord::parse(text) {
            Ok(chord) if !keys.contains(&chord) => keys.push(chord),
            Ok(_) => {}
            Err(error) => {
                return Parsed::Ignored(Problem::BadKeystroke {
                    command: command.to_string(),
                    text: text.to_string(),
                    reason: error.reason(),
                });
            }
        }
    }
    Parsed::Accepted(if keys.is_empty() {
        Override::Removed
    } else {
        Override::Keys(keys)
    })
}

// Whatever cannot be honoured is reported and left out; the command keeps its default.
pub fn validate(
    model: &Model,
    raw: &BTreeMap<String, Value>,
    foreign: &[Claim],
    approved: &Overrides,
) -> (Overrides, Vec<Problem>) {
    let mut overrides = Overrides::default();
    let mut problems: Vec<Problem> = Vec::new();
    for (name, value) in raw {
        let Some(command) = model.command(name) else {
            problems.push(Problem::UnknownCommand(name.clone()));
            continue;
        };
        if command.home().is_none() {
            problems.push(Problem::FixedCommand(command.label.to_string()));
            continue;
        }
        match parse_entry(command.label, value) {
            Parsed::Accepted(replacement) => {
                if is_guarded(command.name) && approved.get(command.name) != Some(&replacement) {
                    problems.push(Problem::Held(command.label.to_string()));
                } else {
                    overrides.insert(command.name, replacement);
                }
            }
            Parsed::Ignored(problem) => problems.push(problem),
        }
    }
    drop_conflicting(model, &mut overrides, foreign, &mut problems);
    (overrides, problems)
}

// Two commands on one key would be settled by file order, which nobody can see; the
// override that collides is left out instead.
fn drop_conflicting(
    model: &Model,
    overrides: &mut Overrides,
    foreign: &[Claim],
    problems: &mut Vec<Problem>,
) {
    loop {
        let claims = model.claims(overrides, foreign);
        let found = overrides.iter().rev().find_map(|(name, replacement)| {
            let Override::Keys(keys) = replacement else {
                return None;
            };
            let command = model.command(name)?;
            let scopes = command.scopes();
            keys.iter().find_map(|chord| {
                let conflict = conflict::find(&claims, name, &scopes, chord)?;
                Some((name, chord.clone(), conflict))
            })
        });
        let Some((name, chord, conflict)) = found else {
            return;
        };
        let other = match conflict {
            conflict::Conflict::Replaceable(others) => others.first().cloned().unwrap_or_default(),
            conflict::Conflict::Fixed(other) => other,
        };
        problems.push(Problem::Conflict {
            command: model.label_of(name),
            keys: chord.display(),
            other: model.label_of(&other),
        });
        overrides.0.remove(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOLD: &str = "zenkai::ToggleBold";
    const ITALIC: &str = "zenkai::ToggleItalic";
    const SIDEBAR: &str = "zenkai::ToggleSidebar";

    fn raw(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
        entries
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect()
    }

    fn keys(texts: &[&str]) -> Value {
        Value::Array(texts.iter().map(|text| Value::from(*text)).collect())
    }

    fn chords(texts: &[&str]) -> Vec<Chord> {
        texts
            .iter()
            .map(|text| Chord::parse(text).unwrap())
            .collect()
    }

    #[test]
    fn a_valid_entry_becomes_an_override_and_null_removes_the_shortcut() {
        let model = Model::build();
        let (overrides, problems) = validate(
            &model,
            &raw(&[(BOLD, keys(&["ctrl-shift-b"])), (ITALIC, Value::Null)]),
            &[],
            &Overrides::default(),
        );
        assert_eq!(problems, []);
        assert_eq!(
            overrides.get(BOLD),
            Some(&Override::Keys(chords(&["ctrl-shift-b"])))
        );
        assert_eq!(overrides.get(ITALIC), Some(&Override::Removed));
    }

    #[test]
    fn an_unknown_command_is_reported_and_ignored() {
        let model = Model::build();
        let (overrides, problems) = validate(
            &model,
            &raw(&[("zenkai::Nonsense", keys(&["ctrl-alt-j"]))]),
            &[],
            &Overrides::default(),
        );
        assert_eq!(overrides.len(), 0);
        assert_eq!(
            problems,
            [Problem::UnknownCommand("zenkai::Nonsense".into())]
        );
    }

    #[test]
    fn a_keystroke_that_does_not_parse_leaves_the_default() {
        let model = Model::build();
        let (overrides, problems) = validate(
            &model,
            &raw(&[
                (BOLD, keys(&["ctrl-alt-j", "ctrl-bogus"])),
                (ITALIC, keys(&["ctrl-k ctrl-i"])),
            ]),
            &[],
            &Overrides::default(),
        );
        assert_eq!(overrides.len(), 0);
        assert_eq!(problems.len(), 2);
        assert!(matches!(problems[0], Problem::BadKeystroke { .. }));
    }

    #[test]
    fn a_value_of_the_wrong_shape_is_one_bad_entry() {
        let model = Model::build();
        let (overrides, problems) = validate(
            &model,
            &raw(&[
                (BOLD, Value::from("ctrl-alt-j")),
                (ITALIC, keys(&["ctrl-alt-j"])),
            ]),
            &[],
            &Overrides::default(),
        );
        assert_eq!(problems, [Problem::BadEntry(model.label_of(BOLD))]);
        assert_eq!(overrides.len(), 1);
    }

    #[test]
    fn commands_that_raise_agent_permissions_cannot_be_rebound() {
        let model = Model::build();
        let (overrides, problems) = validate(
            &model,
            &raw(&[("zenkai::AllowAgentChange", keys(&["ctrl-s"]))]),
            &[],
            &Overrides::default(),
        );
        assert_eq!(overrides.len(), 0);
        assert!(matches!(problems[0], Problem::FixedCommand(_)));
    }

    #[test]
    fn a_file_may_move_a_shortcut_when_it_frees_the_old_owner() {
        let model = Model::build();
        let (overrides, problems) = validate(
            &model,
            &raw(&[(SIDEBAR, keys(&["ctrl-b"])), (BOLD, Value::Null)]),
            &[],
            &Overrides::default(),
        );
        assert_eq!(problems, []);
        assert_eq!(overrides.len(), 2);
    }

    #[test]
    fn an_override_that_takes_a_key_from_another_command_is_ignored() {
        let model = Model::build();
        let (overrides, problems) = validate(
            &model,
            &raw(&[(SIDEBAR, keys(&["ctrl-b"]))]),
            &[],
            &Overrides::default(),
        );
        assert_eq!(overrides.len(), 0);
        assert!(matches!(problems[0], Problem::Conflict { .. }));
    }

    #[test]
    fn two_overrides_on_one_key_keep_only_the_first_by_name() {
        let model = Model::build();
        let (overrides, problems) = validate(
            &model,
            &raw(&[
                (SIDEBAR, keys(&["ctrl-alt-j"])),
                (ITALIC, keys(&["ctrl-alt-j"])),
            ]),
            &[],
            &Overrides::default(),
        );
        assert_eq!(problems.len(), 1);
        assert_eq!(overrides.len(), 1);
        assert!(overrides.get(ITALIC).is_some());
    }

    #[test]
    fn a_file_cannot_put_a_key_on_a_destructive_command_unless_the_user_set_it() {
        let model = Model::build();
        let entry = raw(&[
            ("zenkai::DeleteSpace", keys(&["ctrl-s"])),
            ("zenkai::Save", Value::Null),
        ]);
        let (overrides, problems) = validate(&model, &entry, &[], &Overrides::default());
        assert_eq!(
            problems,
            [Problem::Held(model.label_of("zenkai::DeleteSpace"))]
        );
        assert_eq!(overrides.get("zenkai::DeleteSpace"), None);
        assert_eq!(overrides.get("zenkai::Save"), Some(&Override::Removed));

        let mut approved = Overrides::default();
        approved.insert("zenkai::DeleteSpace", Override::Keys(chords(&["ctrl-s"])));
        let (overrides, problems) = validate(&model, &entry, &[], &approved);
        assert_eq!(problems, []);
        assert!(overrides.get("zenkai::DeleteSpace").is_some());
    }

    #[test]
    fn an_approval_for_other_keys_does_not_cover_the_file() {
        let model = Model::build();
        let entry = raw(&[("zenkai::DeleteSpace", keys(&["ctrl-alt-j"]))]);
        let mut approved = Overrides::default();
        approved.insert("zenkai::DeleteSpace", Override::Keys(chords(&["ctrl-k"])));
        let (overrides, problems) = validate(&model, &entry, &[], &approved);
        assert_eq!(overrides.len(), 0);
        assert_eq!(problems.len(), 1);
    }
}
