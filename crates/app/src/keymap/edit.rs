use std::collections::BTreeMap;

use serde_json::Value;

use super::chord::Chord;
use super::model::Model;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Set {
        command: &'static str,
        keys: Vec<Chord>,
    },
    Reset {
        command: &'static str,
    },
    ResetAll,
}

// Settings only hold what differs from the defaults: keys equal to the defaults are dropped
// from the file, and an empty list is written as null.
pub fn apply(model: &Model, raw: &mut BTreeMap<String, Value>, edits: &[Edit]) {
    for edit in edits {
        match edit {
            Edit::ResetAll => raw.clear(),
            Edit::Reset { command } => {
                raw.remove(*command);
            }
            Edit::Set { command, keys } => {
                let Some(known) = model.command(command) else {
                    continue;
                };
                let mut wanted = keys.clone();
                let mut defaults = model.default_keys(known);
                wanted.sort();
                defaults.sort();
                if wanted == defaults {
                    raw.remove(*command);
                } else if keys.is_empty() {
                    raw.insert(command.to_string(), Value::Null);
                } else {
                    let texts = keys.iter().map(|chord| Value::from(chord.as_str()));
                    raw.insert(command.to_string(), Value::Array(texts.collect()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOLD: &str = "zenkai::ToggleBold";

    fn chord(text: &str) -> Chord {
        Chord::parse(text).unwrap()
    }

    #[test]
    fn a_new_key_is_stored_and_the_default_key_deletes_the_entry() {
        let model = Model::build();
        let mut raw = BTreeMap::new();
        apply(
            &model,
            &mut raw,
            &[Edit::Set {
                command: BOLD,
                keys: vec![chord("ctrl-shift-b")],
            }],
        );
        assert_eq!(raw[BOLD], serde_json::json!(["ctrl-shift-b"]));
        apply(
            &model,
            &mut raw,
            &[Edit::Set {
                command: BOLD,
                keys: vec![chord("ctrl-b")],
            }],
        );
        assert!(raw.is_empty());
    }

    #[test]
    fn removing_the_keys_writes_null() {
        let model = Model::build();
        let mut raw = BTreeMap::new();
        apply(
            &model,
            &mut raw,
            &[Edit::Set {
                command: BOLD,
                keys: Vec::new(),
            }],
        );
        assert!(raw[BOLD].is_null());
    }

    #[test]
    fn a_command_without_a_default_stays_default_when_given_no_keys() {
        let model = Model::build();
        let mut raw = BTreeMap::new();
        apply(
            &model,
            &mut raw,
            &[Edit::Set {
                command: "zenkai::AlignLeft",
                keys: Vec::new(),
            }],
        );
        assert!(raw.is_empty());
    }

    #[test]
    fn reset_clears_one_entry_and_reset_all_clears_them_all() {
        let model = Model::build();
        let mut raw = BTreeMap::from([
            (BOLD.to_string(), Value::Null),
            ("zenkai::Open".to_string(), Value::Null),
            ("zenkai::Typo".to_string(), Value::Null),
        ]);
        apply(&model, &mut raw, &[Edit::Reset { command: BOLD }]);
        assert_eq!(raw.len(), 2);
        apply(&model, &mut raw, &[Edit::ResetAll]);
        assert!(raw.is_empty());
    }
}
