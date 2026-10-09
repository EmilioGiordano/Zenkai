use std::io::ErrorKind;

use zenkai_agent::settings_file::SettingsPaths;
use zenkai_types::Language;

// Read before the first window opens: the settings watcher loads in the background, too
// late for the first frame.
pub fn startup() -> Language {
    configured().unwrap_or_else(system)
}

fn system() -> Language {
    sys_locale::get_locale().map_or(Language::English, |tag| Language::from_locale_tag(&tag))
}

fn configured() -> Option<Language> {
    let paths = SettingsPaths::from_environment()
        .inspect_err(
            |error| tracing::debug!(%error, "no settings folder to read the language from"),
        )
        .ok()?;
    let text = match std::fs::read_to_string(paths.settings()) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(%error, "could not read the language from settings.json");
            return None;
        }
    };
    language_in(&text)
}

fn language_in(text: &str) -> Option<Language> {
    let value: serde_json::Value = serde_json::from_str(text)
        .inspect_err(|error| tracing::warn!(%error, "settings.json is not valid JSON"))
        .ok()?;
    let language = value.get("language")?.clone();
    serde_json::from_value(language)
        .inspect_err(|error| tracing::warn!(%error, "unknown language in settings.json"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_key_selects_the_language() {
        assert_eq!(
            language_in(r#"{ "language": "es" }"#),
            Some(Language::Spanish)
        );
        assert_eq!(
            language_in(r#"{ "language": "en" }"#),
            Some(Language::English)
        );
    }

    #[test]
    fn an_absent_or_unknown_language_falls_through() {
        assert_eq!(language_in("{}"), None);
        assert_eq!(language_in(r#"{ "language": "fr" }"#), None);
        assert_eq!(language_in("not json"), None);
    }
}
