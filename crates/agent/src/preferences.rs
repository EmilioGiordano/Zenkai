use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zenkai_i18n::t;

pub const MIN_AUTOSAVE_SECONDS: u32 = 10;
pub const MAX_AUTOSAVE_SECONDS: u32 = 600;
pub const AUTOSAVE_STEP_SECONDS: u32 = 10;
const DEFAULT_AUTOSAVE_SECONDS: u32 = 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GeneralSettings {
    #[serde(default = "restore_by_default")]
    #[schemars(
        description = "Reopen the spaces and workbooks of the last session, including unsaved work, when Zenkai starts."
    )]
    pub restore_session: bool,
    #[serde(default = "default_autosave", deserialize_with = "clamped_autosave")]
    #[schemars(
        range(min = 10, max = 600),
        description = "Seconds between recovery copies of work that is not saved."
    )]
    pub autosave_seconds: u32,
}

fn restore_by_default() -> bool {
    true
}

fn default_autosave() -> u32 {
    DEFAULT_AUTOSAVE_SECONDS
}

// A value out of range never fails the whole file; it lands on the nearest allowed one.
fn clamped_autosave<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let seconds = u64::deserialize(deserializer)?;
    Ok(seconds.clamp(
        u64::from(MIN_AUTOSAVE_SECONDS),
        u64::from(MAX_AUTOSAVE_SECONDS),
    ) as u32)
}

impl Default for GeneralSettings {
    fn default() -> GeneralSettings {
        GeneralSettings {
            restore_session: restore_by_default(),
            autosave_seconds: DEFAULT_AUTOSAVE_SECONDS,
        }
    }
}

impl GeneralSettings {
    pub fn autosave_stepped(&self, up: bool) -> u32 {
        let seconds = if up {
            self.autosave_seconds.saturating_add(AUTOSAVE_STEP_SECONDS)
        } else {
            self.autosave_seconds.saturating_sub(AUTOSAVE_STEP_SECONDS)
        };
        seconds.clamp(MIN_AUTOSAVE_SECONDS, MAX_AUTOSAVE_SECONDS)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    Light,
    Dark,
    #[default]
    System,
}

impl ColorMode {
    pub const ALL: [ColorMode; 3] = [ColorMode::Light, ColorMode::Dark, ColorMode::System];

    pub fn label(self) -> &'static str {
        match self {
            ColorMode::Light => t!("settings.color_mode.light"),
            ColorMode::Dark => t!("settings.color_mode.dark"),
            ColorMode::System => t!("settings.color_mode.system"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DarkTheme {
    #[default]
    ZenkaiDark,
    HighContrast,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LightTheme {
    #[default]
    ZenkaiLight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    Dark(DarkTheme),
    Light(LightTheme),
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [
        ThemeChoice::Dark(DarkTheme::ZenkaiDark),
        ThemeChoice::Dark(DarkTheme::HighContrast),
        ThemeChoice::Light(LightTheme::ZenkaiLight),
    ];

    pub fn is_dark(self) -> bool {
        matches!(self, ThemeChoice::Dark(_))
    }

    pub fn name(self) -> &'static str {
        match self {
            ThemeChoice::Dark(DarkTheme::ZenkaiDark) => t!("theme.zenkai_dark"),
            ThemeChoice::Dark(DarkTheme::HighContrast) => t!("theme.high_contrast"),
            ThemeChoice::Light(LightTheme::ZenkaiLight) => t!("theme.zenkai_light"),
        }
    }

    pub fn tag(self) -> &'static str {
        if self.is_dark() {
            t!("settings.color_mode.dark")
        } else {
            t!("settings.color_mode.light")
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppearanceSettings {
    #[serde(default)]
    #[schemars(description = "Light, dark, or follow the system setting.")]
    pub mode: ColorMode,
    #[serde(default)]
    #[schemars(description = "Theme used when the appearance is dark.")]
    pub dark_theme: DarkTheme,
    #[serde(default)]
    #[schemars(description = "Theme used when the appearance is light.")]
    pub light_theme: LightTheme,
}

impl AppearanceSettings {
    pub fn effective(&self, system_is_dark: bool) -> ThemeChoice {
        let dark = match self.mode {
            ColorMode::Light => false,
            ColorMode::Dark => true,
            ColorMode::System => system_is_dark,
        };
        if dark {
            ThemeChoice::Dark(self.dark_theme)
        } else {
            ThemeChoice::Light(self.light_theme)
        }
    }

    // Picking a theme also picks the appearance it belongs to; otherwise choosing a light
    // theme while the system is dark would change nothing on screen.
    pub fn with_theme(&self, choice: ThemeChoice) -> AppearanceSettings {
        match choice {
            ThemeChoice::Dark(theme) => AppearanceSettings {
                mode: ColorMode::Dark,
                dark_theme: theme,
                ..*self
            },
            ThemeChoice::Light(theme) => AppearanceSettings {
                mode: ColorMode::Light,
                light_theme: theme,
                ..*self
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_system_and_restore_the_session() {
        let appearance = AppearanceSettings::default();
        assert_eq!(appearance.mode, ColorMode::System);
        assert_eq!(appearance.effective(true), ThemeChoice::ALL[0]);
        assert_eq!(appearance.effective(false), ThemeChoice::ALL[2]);
        let general = GeneralSettings::default();
        assert!(general.restore_session);
        assert_eq!(general.autosave_seconds, 60);
    }

    #[test]
    fn a_fixed_mode_ignores_the_system() {
        let dark = AppearanceSettings {
            mode: ColorMode::Dark,
            dark_theme: DarkTheme::HighContrast,
            ..Default::default()
        };
        assert_eq!(dark.effective(false), ThemeChoice::ALL[1]);
        let light = AppearanceSettings {
            mode: ColorMode::Light,
            ..dark
        };
        assert_eq!(light.effective(true), ThemeChoice::ALL[2]);
    }

    #[test]
    fn picking_a_theme_switches_to_its_appearance_and_keeps_the_other_slot() {
        let appearance = AppearanceSettings {
            mode: ColorMode::System,
            dark_theme: DarkTheme::HighContrast,
            ..Default::default()
        };
        let light = appearance.with_theme(ThemeChoice::ALL[2]);
        assert_eq!(light.mode, ColorMode::Light);
        assert_eq!(light.dark_theme, DarkTheme::HighContrast);
        let dark = appearance.with_theme(ThemeChoice::ALL[0]);
        assert_eq!(dark.mode, ColorMode::Dark);
        assert_eq!(dark.dark_theme, DarkTheme::ZenkaiDark);
    }

    #[test]
    fn the_autosave_interval_steps_inside_its_limits() {
        let mut general = GeneralSettings::default();
        assert_eq!(general.autosave_stepped(true), 70);
        general.autosave_seconds = MAX_AUTOSAVE_SECONDS;
        assert_eq!(general.autosave_stepped(true), MAX_AUTOSAVE_SECONDS);
        general.autosave_seconds = MIN_AUTOSAVE_SECONDS;
        assert_eq!(general.autosave_stepped(false), MIN_AUTOSAVE_SECONDS);
    }

    #[test]
    fn an_autosave_interval_out_of_range_is_clamped_not_rejected() {
        let low: GeneralSettings = serde_json::from_str(r#"{ "autosave_seconds": 1 }"#).unwrap();
        assert_eq!(low.autosave_seconds, MIN_AUTOSAVE_SECONDS);
        let high: GeneralSettings =
            serde_json::from_str(r#"{ "autosave_seconds": 99999 }"#).unwrap();
        assert_eq!(high.autosave_seconds, MAX_AUTOSAVE_SECONDS);
        assert!(serde_json::from_str::<GeneralSettings>(r#"{ "autosave_seconds": -5 }"#).is_err());
    }

    #[test]
    fn the_choices_serialize_as_snake_case_words() {
        let appearance = AppearanceSettings {
            mode: ColorMode::Dark,
            dark_theme: DarkTheme::HighContrast,
            light_theme: LightTheme::ZenkaiLight,
        };
        let json = serde_json::to_string(&appearance).unwrap();
        assert_eq!(
            json,
            r#"{"mode":"dark","dark_theme":"high_contrast","light_theme":"zenkai_light"}"#
        );
        assert_eq!(
            serde_json::from_str::<AppearanceSettings>(&json).unwrap(),
            appearance
        );
    }
}
