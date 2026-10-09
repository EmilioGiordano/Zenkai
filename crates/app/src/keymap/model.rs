use std::collections::HashSet;

use gpui_kit::Action;

use super::chord::Chord;
use super::conflict::{Claim, Scope};
use super::defaults::{Binding, defaults};
use super::overrides::{Override, Overrides};
use crate::actions::*;
use crate::palette;

const WORKSPACE: Option<&str> = Some("Workspace");
const SETTINGS_WINDOW: Option<&str> = Some("SettingsWindow");

// Commands that give agents more power. A settings.json edit must not be able to put one
// under a key the user presses for something else.
fn is_locked(name: &str) -> bool {
    [
        <AllowAgentChange as Action>::name_for_type(),
        <LetAgentsEdit as Action>::name_for_type(),
        <ApplyHeldSettings as Action>::name_for_type(),
        <PermissionAutomatic as Action>::name_for_type(),
        <ToggleExternalAgents as Action>::name_for_type(),
    ]
    .contains(&name)
}

// Commands that destroy data or end the session. The user may bind them in Settings, but a
// settings.json entry for them is held until the user sets it there, so a file edit cannot
// move a key people press often (Ctrl+S) onto one of them.
pub fn is_guarded(name: &str) -> bool {
    [
        <DeleteFile as Action>::name_for_type(),
        <DeleteSpace as Action>::name_for_type(),
        <Quit as Action>::name_for_type(),
    ]
    .contains(&name)
}

pub enum Editability {
    Editable { home: Vec<Option<&'static str>> },
    Fixed,
}

pub struct Command {
    pub name: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub action: Box<dyn Action>,
    pub editability: Editability,
}

impl Command {
    pub fn home(&self) -> Option<&[Option<&'static str>]> {
        match &self.editability {
            Editability::Editable { home } => Some(home),
            Editability::Fixed => None,
        }
    }

    pub fn scopes(&self) -> Vec<Scope> {
        self.home()
            .unwrap_or_default()
            .iter()
            .map(|context| Scope::of(*context))
            .collect()
    }
}

pub struct Model {
    pub defaults: Vec<Binding>,
    pub commands: Vec<Command>,
}

fn distinct<T: PartialEq + std::marker::Copy>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut seen: Vec<T> = Vec::new();
    for item in items {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen
}

impl Model {
    pub fn build() -> Model {
        let defaults = defaults();
        let mut commands: Vec<Command> = Vec::new();
        for group in palette::table() {
            for entry in group.entries {
                let name = entry.action.name();
                if commands.iter().any(|command| command.name == name) {
                    continue;
                }
                let editability = editability_of(name, &defaults);
                commands.push(Command {
                    name,
                    group: group.label,
                    label: entry.label,
                    action: entry.action,
                    editability,
                });
            }
        }
        Model { defaults, commands }
    }

    pub fn command(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|command| command.name == name)
    }

    pub fn owned_names(&self) -> HashSet<&'static str> {
        self.defaults
            .iter()
            .map(|binding| binding.action.name())
            .chain(self.commands.iter().map(|command| command.name))
            .collect()
    }

    pub fn default_keys(&self, command: &Command) -> Vec<Chord> {
        let own = self
            .defaults
            .iter()
            .filter(|binding| binding.action.name() == command.name)
            .filter(|binding| {
                command
                    .home()
                    .is_none_or(|home| home.contains(&binding.context))
            })
            .map(|binding| &binding.chord);
        let mut keys: Vec<Chord> = Vec::new();
        for chord in own {
            if !keys.contains(chord) {
                keys.push(chord.clone());
            }
        }
        keys
    }

    pub fn keys(&self, command: &Command, overrides: &Overrides) -> Vec<Chord> {
        match (&command.editability, overrides.get(command.name)) {
            (Editability::Editable { .. }, Some(Override::Keys(keys))) => keys.clone(),
            (Editability::Editable { .. }, Some(Override::Removed)) => Vec::new(),
            _ => self.default_keys(command),
        }
    }

    pub fn effective(&self, overrides: &Overrides) -> Vec<Binding> {
        let mut effective: Vec<Binding> = Vec::new();
        let mut placed: HashSet<&str> = HashSet::new();
        for binding in &self.defaults {
            let name = binding.action.name();
            let replaced = self
                .command(name)
                .zip(overrides.get(name))
                .filter(|(command, _)| command.home().is_some());
            let Some((command, replacement)) = replaced else {
                effective.push(duplicate(binding));
                continue;
            };
            let home = command.home().unwrap_or_default();
            if !home.contains(&binding.context) {
                effective.push(duplicate(binding));
            } else if placed.insert(name) {
                effective.extend(replacement_bindings(command, replacement));
            }
        }
        for (name, replacement) in overrides.iter() {
            if let Some(command) = self.command(name)
                && command.home().is_some()
                && placed.insert(name)
            {
                effective.extend(replacement_bindings(command, replacement));
            }
        }
        effective
    }

    pub fn claims(&self, overrides: &Overrides, foreign: &[Claim]) -> Vec<Claim> {
        self.effective(overrides)
            .iter()
            .map(|binding| {
                let name = binding.action.name();
                let editable = self
                    .command(name)
                    .and_then(Command::home)
                    .is_some_and(|home| home.contains(&binding.context));
                Claim {
                    chord: binding.chord.clone(),
                    owner: name.to_string(),
                    scope: Scope::of(binding.context),
                    editable,
                }
            })
            .chain(foreign.iter().cloned())
            .collect()
    }

    pub fn label_of(&self, name: &str) -> String {
        self.command(name)
            .map_or_else(|| humanize(name), |command| command.label.to_string())
    }
}

fn editability_of(name: &str, defaults: &[Binding]) -> Editability {
    if is_locked(name) {
        return Editability::Fixed;
    }
    let own: Vec<Option<&'static str>> = defaults
        .iter()
        .filter(|binding| binding.action.name() == name)
        .map(|binding| binding.context)
        .collect();
    let roots = distinct(
        own.iter()
            .copied()
            .filter(|context| *context == WORKSPACE || *context == SETTINGS_WINDOW),
    );
    if !roots.is_empty() {
        Editability::Editable { home: roots }
    } else if own.is_empty() {
        Editability::Editable {
            home: vec![WORKSPACE],
        }
    } else {
        Editability::Fixed
    }
}

fn duplicate(binding: &Binding) -> Binding {
    Binding {
        chord: binding.chord.clone(),
        action: binding.action.boxed_clone(),
        context: binding.context,
    }
}

fn replacement_bindings(command: &Command, replacement: &Override) -> Vec<Binding> {
    let Override::Keys(keys) = replacement else {
        return Vec::new();
    };
    command
        .home()
        .unwrap_or_default()
        .iter()
        .flat_map(|context| {
            keys.iter().map(|chord| Binding {
                chord: chord.clone(),
                action: command.action.boxed_clone(),
                context: *context,
            })
        })
        .collect()
}

// "zenkai::JumpUp" becomes "Jump up", for commands that have no label of their own.
pub fn humanize(name: &str) -> String {
    let short = name.rsplit("::").next().unwrap_or(name);
    let mut text = String::new();
    for (index, letter) in short.chars().enumerate() {
        if index == 0 {
            text.extend(letter.to_uppercase());
        } else if letter.is_uppercase() {
            text.push(' ');
            text.extend(letter.to_lowercase());
        } else {
            text.push(letter);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};

    use super::*;
    use crate::keymap::conflict::{Conflict, find};
    use crate::keymap::overrides::validate;

    fn overrides_from(raw: Value) -> Overrides {
        let raw: BTreeMap<String, Value> = serde_json::from_value(raw).unwrap();
        let (overrides, problems) = validate(&Model::build(), &raw, &[], &Overrides::default());
        assert_eq!(problems, []);
        overrides
    }

    fn bound(effective: &[Binding], command: &str) -> Vec<(String, Option<&'static str>)> {
        effective
            .iter()
            .filter(|binding| binding.action.name() == command)
            .map(|binding| (binding.chord.as_str().to_string(), binding.context))
            .collect()
    }

    #[test]
    fn without_overrides_the_effective_keymap_is_the_defaults() {
        let model = Model::build();
        let effective = model.effective(&Overrides::default());
        assert_eq!(effective.len(), model.defaults.len());
        assert_eq!(
            bound(&effective, "zenkai::ToggleBold"),
            [("ctrl-b".to_string(), WORKSPACE)]
        );
    }

    #[test]
    fn loading_overrides_rebinds_and_removes_in_the_home_context() {
        let model = Model::build();
        let overrides = overrides_from(json!({
            "zenkai::ToggleBold": ["ctrl-shift-b"],
            "zenkai::ToggleItalic": null,
        }));
        let effective = model.effective(&overrides);
        assert_eq!(
            bound(&effective, "zenkai::ToggleBold"),
            [("ctrl-shift-b".to_string(), WORKSPACE)]
        );
        assert_eq!(bound(&effective, "zenkai::ToggleItalic"), []);
        assert_eq!(bound(&effective, "zenkai::ToggleUnderline").len(), 1);
    }

    #[test]
    fn a_rebound_command_keeps_the_keys_of_its_dialogs() {
        let model = Model::build();
        let effective = model.effective(&overrides_from(
            json!({ "zenkai::DenyAgentChange": ["ctrl-j"] }),
        ));
        let bound = bound(&effective, "zenkai::DenyAgentChange");
        assert!(bound.contains(&("ctrl-j".to_string(), WORKSPACE)));
        assert!(!bound.contains(&("alt-n".to_string(), WORKSPACE)));
        assert!(bound.contains(&("enter".to_string(), Some("AgentApproval"))));
        assert!(bound.contains(&("escape".to_string(), Some("AgentApproval"))));
    }

    #[test]
    fn commands_with_no_default_bind_in_the_workspace_and_settings_ones_in_settings() {
        let model = Model::build();
        let effective = model.effective(&overrides_from(json!({
            "zenkai::AlignLeft": ["ctrl-alt-l"],
            "zenkai::DetectAgents": ["alt-j"],
        })));
        assert_eq!(
            bound(&effective, "zenkai::AlignLeft"),
            [("ctrl-alt-l".to_string(), WORKSPACE)]
        );
        assert_eq!(
            bound(&effective, "zenkai::DetectAgents"),
            [("alt-j".to_string(), SETTINGS_WINDOW)]
        );
    }

    #[test]
    fn dialog_only_and_permission_raising_commands_are_fixed() {
        let model = Model::build();
        for name in [
            "zenkai::AllowAgentChange",
            "zenkai::LetAgentsEdit",
            "zenkai::ApplyHeldSettings",
            "zenkai::PermissionAutomatic",
            "zenkai::ToggleExternalAgents",
            "zenkai::KeepCurrentSettings",
            "zenkai::DeleteFile",
        ] {
            assert!(model.command(name).unwrap().home().is_none(), "{name}");
        }
        assert!(
            model
                .command("zenkai::ToggleBold")
                .unwrap()
                .home()
                .is_some()
        );
    }

    #[test]
    fn a_fixed_command_shows_all_its_keys() {
        let model = Model::build();
        let keep = model.command("zenkai::KeepCurrentSettings").unwrap();
        assert!(model.default_keys(keep).len() >= 2);
    }

    #[test]
    fn conflicts_depend_on_the_window() {
        let model = Model::build();
        let claims = model.claims(&Overrides::default(), &[]);
        let chord = Chord::parse("ctrl-b").unwrap();
        let sidebar = model.command("zenkai::ToggleSidebar").unwrap().scopes();
        assert_eq!(
            find(&claims, "zenkai::ToggleSidebar", &sidebar, &chord),
            Some(Conflict::Replaceable(vec!["zenkai::ToggleBold".into()]))
        );
        let detect = model.command("zenkai::DetectAgents").unwrap().scopes();
        assert_eq!(find(&claims, "zenkai::DetectAgents", &detect, &chord), None);
    }

    #[test]
    fn a_key_freed_by_an_override_is_available() {
        let model = Model::build();
        let overrides = overrides_from(json!({ "zenkai::ToggleBold": null }));
        let claims = model.claims(&overrides, &[]);
        let chord = Chord::parse("ctrl-b").unwrap();
        let sidebar = model.command("zenkai::ToggleSidebar").unwrap().scopes();
        assert_eq!(
            find(&claims, "zenkai::ToggleSidebar", &sidebar, &chord),
            None
        );
    }

    #[test]
    fn dialog_keys_are_never_given_up() {
        let model = Model::build();
        let claims = model.claims(&Overrides::default(), &[]);
        let chord = Chord::parse("enter").unwrap();
        let bold = model.command("zenkai::ToggleBold").unwrap().scopes();
        assert!(matches!(
            find(&claims, "zenkai::ToggleBold", &bold, &chord),
            Some(Conflict::Fixed(_))
        ));
    }

    #[test]
    fn humanized_names_read_as_words() {
        assert_eq!(humanize("zenkai::JumpUp"), "Jump up");
        assert_eq!(humanize("grid::SelectAll"), "Select all");
    }
}
