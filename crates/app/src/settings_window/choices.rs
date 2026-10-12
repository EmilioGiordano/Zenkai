use gpui_kit::*;
use zenkai_agent::preferences::{DarkTheme, LightTheme, ThemeChoice};
use zenkai_agent::settings::Settings;
use zenkai_i18n::t;
use zenkai_types::Language;

use super::rows::RowId;
use crate::agent_settings;
use crate::space_appearance::{NewSpaceColor, SpaceAppearance};
use crate::space_settings;

pub enum Choices {
    Plain(Vec<(&'static str, bool)>),
    Themes {
        themes: Vec<ThemeChoice>,
        current: ThemeChoice,
    },
}

fn language_label(language: Option<Language>) -> &'static str {
    match language {
        None => t!("settings.language.follow_windows"),
        Some(Language::English) => "English",
        Some(Language::Spanish) => "Español",
    }
}

const LANGUAGES: [Option<Language>; 3] = [None, Some(Language::English), Some(Language::Spanish)];
const NEW_SPACE_COLORS: [NewSpaceColor; 2] = [NewSpaceColor::Auto, NewSpaceColor::None];

fn new_space_color_label(choice: NewSpaceColor) -> &'static str {
    match choice {
        NewSpaceColor::Auto => t!("settings.new_space_color.automatic"),
        NewSpaceColor::None => t!("settings.new_space_color.none"),
    }
}

fn themes_of(dark: bool) -> Vec<ThemeChoice> {
    ThemeChoice::ALL
        .into_iter()
        .filter(|choice| choice.is_dark() == dark)
        .collect()
}

pub fn value_label(row: RowId, settings: &Settings, spaces: &SpaceAppearance) -> &'static str {
    match row {
        RowId::Language => language_label(settings.language),
        RowId::DarkTheme => ThemeChoice::Dark(settings.appearance.dark_theme).name(),
        RowId::LightTheme => ThemeChoice::Light(settings.appearance.light_theme).name(),
        RowId::NewSpaceColor => new_space_color_label(spaces.new_space_color),
        _ => "",
    }
}

pub fn choices(row: RowId, settings: &Settings, spaces: &SpaceAppearance) -> Option<Choices> {
    Some(match row {
        RowId::Language => Choices::Plain(
            LANGUAGES
                .into_iter()
                .map(|language| (language_label(language), language == settings.language))
                .collect(),
        ),
        RowId::NewSpaceColor => Choices::Plain(
            NEW_SPACE_COLORS
                .into_iter()
                .map(|choice| {
                    (
                        new_space_color_label(choice),
                        choice == spaces.new_space_color,
                    )
                })
                .collect(),
        ),
        RowId::DarkTheme => Choices::Themes {
            themes: themes_of(true),
            current: ThemeChoice::Dark(settings.appearance.dark_theme),
        },
        RowId::LightTheme => Choices::Themes {
            themes: themes_of(false),
            current: ThemeChoice::Light(settings.appearance.light_theme),
        },
        _ => return None,
    })
}

pub fn pick(row: RowId, index: usize, cx: &mut App) {
    match row {
        RowId::Language => {
            if let Some(language) = LANGUAGES.get(index) {
                let language = *language;
                agent_settings::change(cx, move |settings| settings.language = language);
            }
        }
        RowId::NewSpaceColor => {
            if let Some(choice) = NEW_SPACE_COLORS.get(index) {
                let choice = *choice;
                space_settings::change(cx, move |spaces| spaces.new_space_color = choice);
            }
        }
        RowId::DarkTheme => {
            if let Some(ThemeChoice::Dark(theme)) = themes_of(true).get(index) {
                let theme: DarkTheme = *theme;
                agent_settings::change(cx, move |settings| settings.appearance.dark_theme = theme);
            }
        }
        RowId::LightTheme => {
            if let Some(ThemeChoice::Light(theme)) = themes_of(false).get(index) {
                let theme: LightTheme = *theme;
                agent_settings::change(cx, move |settings| settings.appearance.light_theme = theme);
            }
        }
        _ => {}
    }
}
