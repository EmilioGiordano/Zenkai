use std::io::ErrorKind;

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use zenkai_agent::preferences::{AppearanceSettings, InterfaceScale};
use zenkai_agent::settings_file::SettingsPaths;

use crate::agent_settings;

const REM_BASE_PIXELS: f32 = 16.0;

// What is on screen now and the value the settings file last had. A step changes the first
// at once while the write is in flight, so a reload of the older file never undoes it.
#[derive(Default)]
struct OnScreen {
    shown: InterfaceScale,
    from_settings: Option<InterfaceScale>,
}

impl Global for OnScreen {}

pub fn init(cx: &mut App) {
    let startup = startup();
    cx.set_global(OnScreen {
        shown: startup,
        from_settings: None,
    });
}

pub fn shown(cx: &App) -> InterfaceScale {
    cx.global::<OnScreen>().shown
}

// Every window calls this while rendering; the rem size is what grows the bars, dialogs,
// Settings and chat, while the grid is drawn in pixels and keeps its own zoom.
pub fn apply(window: &mut Window, cx: &App) {
    window.set_rem_size(cx.theme().font_size * shown(cx).factor());
}

// Sizes written in pixels for the interface, kept proportional to the interface scale.
pub fn rem_px(pixels: f32) -> Rems {
    rems(pixels / REM_BASE_PIXELS)
}

pub fn larger(cx: &mut App) {
    set(shown(cx).larger(), cx);
}

pub fn smaller(cx: &mut App) {
    set(shown(cx).smaller(), cx);
}

pub fn reset(cx: &mut App) {
    set(InterfaceScale::default(), cx);
}

fn set(scale: InterfaceScale, cx: &mut App) {
    show(scale, cx);
    agent_settings::change(cx, move |settings| {
        settings.appearance.interface_scale = scale;
    });
}

fn show(scale: InterfaceScale, cx: &mut App) {
    if shown(cx) == scale {
        return;
    }
    cx.update_global::<OnScreen, _>(|on_screen, _| on_screen.shown = scale);
    cx.refresh_windows();
}

pub fn sync_with_settings(cx: &mut App) {
    let scale = agent_settings::settings(cx).appearance.interface_scale;
    if cx.global::<OnScreen>().from_settings == Some(scale) {
        return;
    }
    cx.update_global::<OnScreen, _>(|on_screen, _| on_screen.from_settings = Some(scale));
    show(scale, cx);
}

// A settings write that failed leaves the file as it was, so the screen goes back to it.
pub fn revert_to_settings(cx: &mut App) {
    show(agent_settings::settings(cx).appearance.interface_scale, cx);
}

// Read before the first window opens: the settings watcher loads in the background, too
// late for the first frame.
fn startup() -> InterfaceScale {
    configured().unwrap_or_default()
}

fn configured() -> Option<InterfaceScale> {
    let paths = SettingsPaths::from_environment()
        .inspect_err(
            |error| tracing::debug!(%error, "no settings folder to read the interface size from"),
        )
        .ok()?;
    let text = match std::fs::read_to_string(paths.settings()) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(%error, "could not read the interface size from settings.json");
            return None;
        }
    };
    scale_in(&text)
}

fn scale_in(text: &str) -> Option<InterfaceScale> {
    let value: serde_json::Value = serde_json::from_str(text)
        .inspect_err(|error| tracing::warn!(%error, "settings.json is not valid JSON"))
        .ok()?;
    let appearance = value.get("appearance")?.get("interface_scale")?.clone();
    serde_json::from_value::<AppearanceSettings>(
        serde_json::json!({ "interface_scale": appearance }),
    )
    .inspect_err(|error| tracing::warn!(%error, "unreadable interface size in settings.json"))
    .ok()
    .map(|appearance| appearance.interface_scale)
}

#[cfg(test)]
mod tests {
    use zenkai_agent::preferences::InterfaceScale;

    use super::scale_in;

    #[test]
    fn the_appearance_section_selects_the_startup_scale() {
        let scale = scale_in(r#"{ "appearance": { "interface_scale": 150 } }"#);
        assert_eq!(scale.map(InterfaceScale::percent), Some(150));
    }

    #[test]
    fn a_startup_scale_out_of_range_is_clamped_and_absence_falls_through() {
        let scale = scale_in(r#"{ "appearance": { "interface_scale": 5000 } }"#);
        assert_eq!(scale.map(InterfaceScale::percent), Some(200));
        assert_eq!(scale_in("{}"), None);
        assert_eq!(scale_in(r#"{ "appearance": {} }"#), None);
        assert_eq!(scale_in("not json"), None);
    }
}
