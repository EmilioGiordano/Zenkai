// Structured fuzzing of settings.json, which any program running as the user can write:
// damaged files are refused with an error, never a panic, and what parses is safe to use.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../test-support/json_mutation.rs"]
mod json_mutation;

use proptest::prelude::*;
use zenkai_agent::settings::{Settings, SettingsError};
use zenkai_agent::settings_file;

fn baseline() -> serde_json::Value {
    serde_json::json!({
        "$schema": "./settings.schema.json",
        "agents": {
            "default": "claude",
            "servers": {
                "claude": {
                    "name": "Claude",
                    "command": "npx",
                    "args": ["-y", "agent@1"],
                    "env": {
                        "ANTHROPIC_API_KEY": { "secret": "zenkai/anthropic" },
                        "OPENAI_API_KEY": "sk-plain",
                        "MODE": "fast"
                    }
                },
                "local": { "name": "Mine", "command": "C:/tools/agent.exe" }
            },
            "permission": "ask_before_write",
            "external_agents": "blocked"
        }
    })
}

fn check_parsed(settings: &Settings) {
    let json = settings.to_json().unwrap();
    assert_eq!(&Settings::parse(&json).unwrap(), settings);
    settings.plain_secret_warnings();
    for name in settings.secret_names() {
        assert!(!name.as_str().is_empty() && name.as_str().len() <= 128);
        assert!(
            name.as_str()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
        );
    }
    if let Some(default) = &settings.agents.default {
        assert!(settings.agents.servers.contains_key(default));
    }
}

#[test]
fn the_baseline_settings_parse() {
    let text = baseline().to_string();
    check_parsed(&Settings::parse(&text).unwrap());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

    #[test]
    fn damaged_settings_are_refused_or_safe(bytes in json_mutation::mutated(baseline())) {
        let text = String::from_utf8_lossy(&bytes);
        if let Ok(settings) = Settings::parse(&text) {
            check_parsed(&settings);
        }
    }

    #[test]
    fn arbitrary_text_never_panics_the_settings_parser(text in prop::collection::vec(any::<char>(), 0..100).prop_map(String::from_iter)) {
        if let Ok(settings) = Settings::parse(&text) {
            check_parsed(&settings);
        }
    }

    #[test]
    fn a_settings_file_with_any_bytes_loads_or_reports(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("settings.json");
        std::fs::write(&file, &bytes).unwrap();
        match settings_file::load(&file) {
            Ok(settings) => check_parsed(&settings),
            Err(SettingsError::Parse { .. } | SettingsError::Read(_) | SettingsError::UnknownDefault(_)) => {}
            Err(other) => panic!("unexpected error kind: {other}"),
        }
    }
}
