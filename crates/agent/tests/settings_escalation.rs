// Spec: a change to settings.json that gives agents more power never takes effect until the
// user confirms it in Zenkai, whatever order the file changes and the user's answers come in.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use proptest::prelude::*;
use zenkai_agent::settings::{
    Escalation, ExternalAgents, PermissionMode, Settings, SettingsError, SettingsState,
};

#[derive(Clone, Debug)]
enum Step {
    File(usize, usize, bool),
    BrokenFile,
    Page(usize, usize, Vec<Escalation>),
    Accept,
    Decline,
}

fn escalation() -> impl Strategy<Value = Escalation> {
    prop_oneof![
        Just(Escalation::WriteWithoutAsking),
        Just(Escalation::ExternalAgents)
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        4 => (0..3usize, 0..2usize, any::<bool>()).prop_map(|(p, e, s)| Step::File(p, e, s)),
        1 => Just(Step::BrokenFile),
        2 => (0..3usize, 0..2usize, prop::collection::vec(escalation(), 0..3))
            .prop_map(|(p, e, a)| Step::Page(p, e, a)),
        2 => Just(Step::Accept),
        2 => Just(Step::Decline),
    ]
}

fn settings(permission: usize, external: usize, extra_server: bool) -> Settings {
    let permission = ["read_only", "ask_before_write", "automatic"][permission];
    let external = ["blocked", "allowed"][external];
    let servers = if extra_server {
        r#"{ "a": { "name": "A", "command": "a" } }"#
    } else {
        "{}"
    };
    Settings::parse(&format!(
        r#"{{ "agents": {{ "servers": {servers}, "permission": "{permission}", "external_agents": "{external}" }} }}"#
    ))
    .unwrap()
}

fn raises_write(before: &SettingsState, after: &SettingsState) -> bool {
    after.current.agents.permission == PermissionMode::Automatic
        && before.current.agents.permission != PermissionMode::Automatic
}

fn raises_external(before: &SettingsState, after: &SettingsState) -> bool {
    after.current.agents.external_agents == ExternalAgents::Allowed
        && before.current.agents.external_agents != ExternalAgents::Allowed
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 500, ..ProptestConfig::default() })]

    #[test]
    fn more_power_only_arrives_through_the_user(steps in prop::collection::vec(step(), 1..30)) {
        let mut state = SettingsState::default();
        for step in steps {
            let before = state.clone();
            match &step {
                Step::File(p, e, s) => state.apply_file(Ok(settings(*p, *e, *s))),
                Step::BrokenFile => state.apply_file(Err(SettingsError::Read("broken".into()))),
                Step::Page(p, e, approved) => state.apply_from_page(settings(*p, *e, false), approved),
                Step::Accept => state.accept_held(),
                Step::Decline => state.decline_held(),
            }
            let write = raises_write(&before, &state);
            let external = raises_external(&before, &state);
            match &step {
                Step::File(..) | Step::BrokenFile | Step::Decline => {
                    prop_assert!(!write && !external, "{step:?} raised power from {before:?} to {state:?}");
                }
                Step::Page(_, _, approved) => {
                    prop_assert!(!write || approved.contains(&Escalation::WriteWithoutAsking));
                    prop_assert!(!external || approved.contains(&Escalation::ExternalAgents));
                }
                Step::Accept => match before.held.as_ref() {
                    Some(held) => {
                        prop_assert!(!write || held.escalations.contains(&Escalation::WriteWithoutAsking));
                        prop_assert!(!external || held.escalations.contains(&Escalation::ExternalAgents));
                    }
                    None => prop_assert!(!write && !external),
                },
            }
        }
    }

    #[test]
    fn a_file_change_applies_everything_that_is_not_more_power(p in 0..3usize, e in 0..2usize, server in any::<bool>()) {
        let mut state = SettingsState::default();
        let wanted = settings(p, e, server);
        state.apply_file(Ok(wanted.clone()));
        prop_assert_eq!(&state.current.agents.servers, &wanted.agents.servers);
        let write_held = wanted.agents.permission == PermissionMode::Automatic;
        let external_held = wanted.agents.external_agents == ExternalAgents::Allowed;
        prop_assert_eq!(state.held.is_some(), write_held || external_held);
        if !write_held {
            prop_assert_eq!(state.current.agents.permission, wanted.agents.permission);
        }
        if !external_held {
            prop_assert_eq!(state.current.agents.external_agents, wanted.agents.external_agents);
        }
    }

    #[test]
    fn a_declined_change_is_not_asked_again_for_the_same_file(p in 0..3usize, e in 0..2usize) {
        let wanted = settings(p, e, false);
        let mut state = SettingsState::default();
        state.apply_file(Ok(wanted.clone()));
        state.decline_held();
        state.apply_file(Ok(wanted));
        prop_assert!(state.held.is_none());
    }
}
