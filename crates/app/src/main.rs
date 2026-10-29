#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod agent_folder;
mod agent_mark;
mod agent_review;
mod agent_routing;
mod agent_settings;
mod assets;
mod auto_sum;
mod autosave;
mod chart;
mod chart_panel;
mod chat;
mod chat_layout;
mod clipboard;
mod close_prompt;
mod csv_export_size;
mod csv_preview;
mod decimals;
mod document;
mod documents;
mod entry;
mod failure;
mod file_search;
mod files;
mod find;
mod format_category;
mod format_dialog;
mod generate_dialog;
mod go_to;
mod interface_scale;
mod job_pill;
mod jump;
mod keymap;
mod language;
mod logging;
mod markdown_depth;
mod markdown_highlight;
mod markdown_images;
mod markdown_links;
mod markdown_pane;
mod memory;
mod open_loads;
mod palette;
mod panel_edge;
mod panel_width;
mod pdf;
mod pdf_places;
mod pdf_tab;
mod pending_save;
mod previews;
mod prompt_gate;
mod recent;
mod recovery;
mod refresh;
mod region;
mod restart;
mod session;
mod settings_window;
mod sidebar_item;
mod sidebar_rows;
mod space_appearance;
mod space_controls;
mod space_parts;
mod space_settings;
mod spaces;
mod stale_view;
mod start_view;
mod stats;
mod text_disk;
mod text_document;
mod text_file;
mod text_zoom;
mod theme;
mod theme_list;
mod theme_palettes;
mod toolbar;
#[cfg(windows)]
mod tray;
mod view;

use std::path::PathBuf;

use gpui_kit::*;

fn main() {
    restart::wait_for_previous_instance();
    let initial: Option<PathBuf> = restart::file_argument();
    // Before the log starts: a launch that only hands over must not truncate the log of the
    // Zenkai that keeps running.
    #[cfg(windows)]
    let claim = match tray::claim(initial.as_deref()) {
        tray::Claim::HandedOff => return,
        tray::Claim::Unreachable => {
            zenkai_i18n::init(language::startup());
            tray::warn_another_zenkai_unreachable();
            return;
        }
        claim => claim,
    };
    logging::init();
    logging::log_panics();
    #[cfg(windows)]
    if matches!(claim, tray::Claim::Standalone) {
        tracing::warn!(
            "the single instance pipe or its token could not be set up; running without single instance protection"
        );
    }
    zenkai_i18n::init(language::startup());

    gpui_kit::application()
        .with_assets(assets::ZenkaiAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            keymap::init(cx);
            theme::init(cx);
            interface_scale::init(cx);
            agent_settings::init(cx);
            settings_window::register_agent_actions(cx);
            cx.on_action(|_: &actions::OpenSettings, cx| settings_window::open(cx));
            cx.on_action(|_: &actions::InterfaceLarger, cx| interface_scale::larger(cx));
            cx.on_action(|_: &actions::InterfaceSmaller, cx| interface_scale::smaller(cx));
            cx.on_action(|_: &actions::InterfaceReset, cx| interface_scale::reset(cx));
            cx.on_action(|_: &actions::RecordShortcutKeys, cx| {
                settings_window::open_to_find_shortcut(cx)
            });
            cx.on_action(|_: &actions::RestartToApplyLanguage, cx| restart::request(cx));
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
                #[cfg(windows)]
                Ok((window, workspace)) => {
                    restart::register(window, &workspace, cx);
                    tray::install(window, &workspace, claim, cx);
                    cx.activate(true)
                }
                #[cfg(not(windows))]
                Ok((window, workspace)) => {
                    restart::register(window, &workspace, cx);
                    cx.activate(true)
                }
                Err(error) => {
                    tracing::error!(%error, "could not open the main window");
                    cx.quit();
                }
            }
        });
}
