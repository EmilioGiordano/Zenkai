#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod chart;
mod chart_panel;
mod clipboard;
mod document;
mod files;
mod find;
mod jump;
mod stats;
mod toolbar;
mod view;

use std::path::PathBuf;

use gpui_kit::*;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let initial: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            zenkai_grid::bind_keys(cx);
            actions::bind_keys(cx);
            cx.on_action(|_: &actions::Quit, cx| cx.quit());

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1280.0), px(800.0)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Zenkai".into()),
                    ..Default::default()
                }),
                ..Default::default()
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
