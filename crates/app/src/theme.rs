// Light, dark and high-contrast themes. High contrast is the dark theme with pure black
// and white surfaces, white borders and a yellow accent (as in Windows' contrast themes).

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use zenkai_grid::HighContrast;

use zenkai_agent::preferences::{AppearanceSettings, DarkTheme, ThemeChoice};

use crate::agent_settings;

// What is on screen now and the appearance settings it was last derived from. A preview
// changes the first and leaves the second, so a settings reload never ends a preview.
#[derive(Default)]
struct OnScreen {
    shown: Option<ThemeChoice>,
    from_settings: Option<AppearanceSettings>,
}

impl Global for OnScreen {}

fn on_screen(cx: &App) -> &OnScreen {
    cx.global::<OnScreen>()
}

pub fn init(cx: &mut App) {
    cx.set_global(OnScreen::default());
}

pub fn shown(cx: &App) -> ThemeChoice {
    on_screen(cx)
        .shown
        .unwrap_or_else(|| AppearanceSettings::default().effective(system_is_dark(cx)))
}

fn system_is_dark(cx: &App) -> bool {
    matches!(
        cx.window_appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

pub fn show(choice: ThemeChoice, cx: &mut App) {
    if on_screen(cx).shown == Some(choice) {
        return;
    }
    match choice {
        ThemeChoice::Dark(DarkTheme::ZenkaiDark) => {
            cx.set_global(HighContrast(false));
            Theme::change(ThemeMode::Dark, None, cx);
        }
        ThemeChoice::Dark(DarkTheme::HighContrast) => {
            Theme::change(ThemeMode::Dark, None, cx);
            apply_high_contrast(cx);
            cx.set_global(HighContrast(true));
        }
        ThemeChoice::Light(_) => {
            cx.set_global(HighContrast(false));
            Theme::change(ThemeMode::Light, None, cx);
        }
    }
    cx.update_global::<OnScreen, _>(|screen, _| screen.shown = Some(choice));
}

pub fn sync_with_settings(cx: &mut App) {
    let appearance = agent_settings::settings(cx).appearance;
    if on_screen(cx).from_settings == Some(appearance) {
        return;
    }
    cx.update_global::<OnScreen, _>(|screen, _| screen.from_settings = Some(appearance));
    show(appearance.effective(system_is_dark(cx)), cx);
}

// In System mode the theme follows the window setting; a fixed mode ignores it.
pub fn follow_system(window: &mut Window, cx: &mut App) {
    let appearance = agent_settings::settings(cx).appearance;
    let system_is_dark = matches!(
        window.appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    show(appearance.effective(system_is_dark), cx);
    window.refresh();
}

// A settings write that failed leaves the file as it was, so what is on screen goes back to it.
pub fn revert_to_settings(cx: &mut App) {
    let appearance = agent_settings::settings(cx).appearance;
    show(appearance.effective(system_is_dark(cx)), cx);
}

pub fn choose(choice: ThemeChoice, cx: &mut App) {
    show(choice, cx);
    agent_settings::change(cx, move |settings| {
        settings.appearance = settings.appearance.with_theme(choice);
    });
}

pub fn cycle(cx: &mut App) {
    let all = ThemeChoice::ALL;
    let current = shown(cx);
    let position = all.iter().position(|choice| *choice == current);
    choose(all[position.map_or(0, |at| (at + 1) % all.len())], cx);
}

// Moving through the theme list applies each theme at once; confirming keeps the last one
// and cancelling puts the original back.
#[derive(Clone, Copy, Debug)]
pub struct ThemePreview {
    original: AppearanceSettings,
    original_shown: ThemeChoice,
    shown: ThemeChoice,
}

impl ThemePreview {
    pub fn begin(original: AppearanceSettings, original_shown: ThemeChoice) -> ThemePreview {
        ThemePreview {
            original,
            original_shown,
            shown: original_shown,
        }
    }

    pub fn original_shown(&self) -> ThemeChoice {
        self.original_shown
    }

    pub fn highlight(&mut self, choice: ThemeChoice) -> Option<ThemeChoice> {
        (choice != self.shown).then(|| {
            self.shown = choice;
            choice
        })
    }

    pub fn cancel(&mut self) -> Option<ThemeChoice> {
        self.highlight(self.original_shown)
    }

    pub fn keep(self, choice: ThemeChoice) -> AppearanceSettings {
        self.original.with_theme(choice)
    }
}

fn apply_high_contrast(cx: &mut App) {
    let black: Hsla = rgb(0x00_00_00).into();
    let white: Hsla = rgb(0xFF_FF_FF).into();
    let yellow: Hsla = rgb(0xFF_FF_00).into();
    let gray: Hsla = rgb(0x33_33_33).into();
    // Theme::update keeps the colors, the tokens and the base layer in step.
    Theme::update(cx, |theme| {
        let colors = &mut theme.colors;
        for surface in [
            &mut colors.background,
            &mut colors.muted,
            &mut colors.popover,
            &mut colors.table_head,
            &mut colors.tab_bar,
            &mut colors.tab,
            &mut colors.tab_active,
            &mut colors.title_bar,
            &mut colors.status_bar,
            &mut colors.list,
            &mut colors.sidebar,
            &mut colors.secondary,
            &mut colors.button,
            &mut colors.primary_foreground,
            &mut colors.button_primary_foreground,
        ] {
            *surface = black;
        }
        for ink in [
            &mut colors.foreground,
            &mut colors.muted_foreground,
            &mut colors.popover_foreground,
            &mut colors.border,
            &mut colors.sidebar_border,
            &mut colors.sidebar_foreground,
            &mut colors.sidebar_accent_foreground,
            &mut colors.input,
            &mut colors.tab_foreground,
            &mut colors.secondary_foreground,
            &mut colors.button_foreground,
            &mut colors.accent_foreground,
            &mut colors.table_head_foreground,
        ] {
            *ink = white;
        }
        for mark in [
            &mut colors.primary,
            &mut colors.primary_hover,
            &mut colors.primary_active,
            &mut colors.button_primary,
            &mut colors.button_primary_hover,
            &mut colors.button_primary_active,
            &mut colors.ring,
            &mut colors.caret,
            &mut colors.link,
            &mut colors.tab_active_foreground,
        ] {
            *mark = yellow;
        }
        colors.accent = gray;
        colors.secondary_hover = gray;
        colors.button_hover = gray;
        colors.list_hover = gray;
        colors.sidebar_accent = gray;
        colors.table_even = rgb(0x14_14_14).into();
        colors.selection = yellow.opacity(0.4);
        colors.list_active = yellow.opacity(0.3);
        colors.danger = rgb(0xFF_6B_6B).into();
    });
}

#[cfg(test)]
mod tests {
    use zenkai_agent::preferences::{
        AppearanceSettings, ColorMode, DarkTheme, LightTheme, ThemeChoice,
    };

    use super::ThemePreview;

    const DARK: ThemeChoice = ThemeChoice::Dark(DarkTheme::ZenkaiDark);
    const CONTRAST: ThemeChoice = ThemeChoice::Dark(DarkTheme::HighContrast);
    const LIGHT: ThemeChoice = ThemeChoice::Light(LightTheme::ZenkaiLight);

    fn preview_from_dark() -> ThemePreview {
        let original = AppearanceSettings {
            mode: ColorMode::Dark,
            ..Default::default()
        };
        ThemePreview::begin(original, DARK)
    }

    #[test]
    fn each_highlighted_theme_applies_once_and_a_repeat_is_ignored() {
        let mut preview = preview_from_dark();
        assert_eq!(preview.highlight(DARK), None);
        assert_eq!(preview.highlight(LIGHT), Some(LIGHT));
        assert_eq!(preview.highlight(LIGHT), None);
        assert_eq!(preview.highlight(CONTRAST), Some(CONTRAST));
    }

    #[test]
    fn cancelling_restores_the_theme_that_was_showing() {
        let mut preview = preview_from_dark();
        preview.highlight(LIGHT);
        assert_eq!(preview.cancel(), Some(DARK));
        assert_eq!(preview.cancel(), None);
    }

    #[test]
    fn cancelling_without_moving_changes_nothing() {
        assert_eq!(preview_from_dark().cancel(), None);
    }

    #[test]
    fn keeping_the_highlighted_theme_selects_it_and_its_appearance() {
        let mut preview = preview_from_dark();
        preview.highlight(LIGHT);
        let kept = preview.keep(LIGHT);
        assert_eq!(kept.mode, ColorMode::Light);
        assert_eq!(kept.effective(true), LIGHT);
    }

    #[test]
    fn the_original_settings_are_untouched_until_the_choice_is_kept() {
        let mut preview = preview_from_dark();
        preview.highlight(CONTRAST);
        assert_eq!(preview.original_shown(), DARK);
        assert_eq!(preview.cancel(), Some(DARK));
    }
}
