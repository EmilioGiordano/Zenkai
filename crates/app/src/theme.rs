// Light, dark and high-contrast themes. High contrast is the dark theme with pure black
// and white surfaces, white borders and a yellow accent (as in Windows' contrast themes).

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use zenkai_grid::HighContrast;

/// Set once the user picks a theme; from then on the system setting is not followed.
struct ChosenTheme;

impl Global for ChosenTheme {}

/// Light or dark as the system is, unless the user chose a theme in this session.
pub fn follow_system(window: &mut Window, cx: &mut App) {
    if cx.has_global::<ChosenTheme>() {
        return;
    }
    Theme::sync_system_appearance(Some(window), cx);
    window.refresh();
}

pub fn cycle(window: &mut Window, cx: &mut App) {
    cx.set_global(ChosenTheme);
    let high_contrast = cx.try_global::<HighContrast>().is_some_and(|h| h.0);
    if high_contrast {
        cx.set_global(HighContrast(false));
        Theme::change(ThemeMode::Light, Some(window), cx);
    } else if Theme::global(cx).mode.is_dark() {
        Theme::change(ThemeMode::Dark, Some(window), cx);
        apply_high_contrast(cx);
        cx.set_global(HighContrast(true));
    } else {
        Theme::change(ThemeMode::Dark, Some(window), cx);
    }
    window.refresh();
}

fn apply_high_contrast(cx: &mut App) {
    let black: Hsla = rgb(0x00_00_00).into();
    let white: Hsla = rgb(0xFF_FF_FF).into();
    let yellow: Hsla = rgb(0xFF_FF_00).into();
    let gray: Hsla = rgb(0x33_33_33).into();
    let colors = &mut Theme::global_mut(cx).colors;
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
    colors.table_even = rgb(0x14_14_14).into();
    colors.selection = yellow.opacity(0.4);
    colors.list_active = yellow.opacity(0.3);
    colors.danger = rgb(0xFF_6B_6B).into();
}
