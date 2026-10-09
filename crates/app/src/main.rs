#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod agent_review;
mod agent_routing;
mod agent_settings;
mod assets;
mod chart;
mod chart_panel;
mod chat;
mod clipboard;
mod csv_preview;
mod decimals;
mod document;
mod documents;
mod entry;
mod file_search;
mod files;
mod find;
mod format_dialog;
mod generate_dialog;
mod jump;
mod keymap;
mod language;
mod logging;
mod memory;
mod palette;
mod previews;
mod recent;
mod recovery;
mod region;
mod session;
mod settings_window;
mod sidebar_item;
mod sidebar_rows;
mod space_appearance;
mod space_controls;
mod space_parts;
mod space_settings;
mod spaces;
mod start_view;
mod stats;
mod theme;
mod theme_list;
mod toolbar;
mod view;

use std::path::PathBuf;

use gpui_kit::*;

fn main() {
    logging::init();
    zenkai_i18n::init(language::startup());
    let initial: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    gpui_kit::application()
        .with_assets(assets::ZenkaiAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            zenkai_grid::bind_keys(cx);
            keymap::init(cx);
            theme::init(cx);
            agent_settings::init(cx);
            settings_window::register_agent_actions(cx);
            cx.on_action(|_: &actions::OpenSettings, cx| settings_window::open(cx));
            cx.on_action(|_: &actions::RecordShortcutKeys, cx| {
                settings_window::open_to_find_shortcut(cx)
            });
            cx.on_action(|_: &actions::ResetAllShortcuts, cx| {
                if keymap::request_reset_all(cx) {
                    settings_window::open_to_keyboard(cx);
                }
            });

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1280.0), px(800.0)), cx)),
                ..gpui_kit::component::TitleBar::window_options()
            };
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| view::Workspace::new(initial, window, cx))
            });
            match opened {
                Ok(_) => cx.activate(true),
                Err(error) => {
                    tracing::error!(%error, "could not open the main window");
                    cx.quit();
                }
            }
        });
}
