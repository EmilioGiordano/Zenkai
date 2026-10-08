#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod chart;
mod chart_panel;
mod clipboard;
mod csv_preview;
mod decimals;
mod document;
mod documents;
mod entry;
mod files;
mod find;
mod format_dialog;
mod jump;
mod logging;
mod palette;
mod previews;
mod recent;
mod recovery;
mod region;
mod session;
mod sidebar_item;
mod sidebar_rows;
mod spaces;
mod stats;
mod theme;
mod toolbar;
mod view;

use std::path::PathBuf;

use gpui_kit::*;

fn main() {
    logging::init();
    let initial: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            zenkai_grid::bind_keys(cx);
            actions::bind_keys(cx);

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
