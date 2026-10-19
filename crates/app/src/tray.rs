use std::path::PathBuf;

use gpui_kit::*;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use zenkai_i18n::t;

use crate::actions::OpenZenkai;
use crate::close_prompt::CloseGoal;
use crate::view::Workspace;
use presence::Presence;
use window::NativeWindow;

mod handoff;
#[cfg(test)]
mod handoff_fuzz;
mod icon;
mod instance;
mod labels;
mod presence;
mod window;

pub use instance::{Claim, claim};

const OPEN_ID: &str = "zenkai-open";
const QUIT_ID: &str = "zenkai-quit";
const MAX_OPENS_AT_ONCE: usize = 8;

#[derive(Debug, thiserror::Error)]
enum TrayError {
    #[error("tray icon image: {0}")]
    Icon(String),
    #[error("tray icon: {0}")]
    Create(String),
    #[error("the main window has no native handle")]
    NoWindowHandle,
}

pub enum TrayEvent {
    Show,
    Quit,
    Refresh,
    Open { path: Option<PathBuf> },
}

struct Tray {
    icon: TrayIcon,
    shown: Icon,
    hidden: Icon,
    workbooks: MenuItem,
    memory: MenuItem,
    presence: Presence,
    hide_requested: bool,
    window: NativeWindow,
    events: async_channel::Sender<TrayEvent>,
}

impl Global for Tray {}

impl Tray {
    fn show_presence(&mut self, presence: Presence) {
        if presence == self.presence {
            return;
        }
        self.presence = presence;
        let (image, tooltip) = match presence {
            Presence::Shown => (self.shown.clone(), t!("tray.tooltip_shown")),
            Presence::HiddenToTray => (self.hidden.clone(), t!("tray.tooltip_hidden")),
        };
        if let Err(error) = self.icon.set_icon(Some(image)) {
            tracing::warn!(%error, "could not change the tray icon");
        }
        if let Err(error) = self.icon.set_tooltip(Some(tooltip)) {
            tracing::warn!(%error, "could not change the tray tooltip");
        }
    }

    fn show_summary(&self, workbooks: usize, committed_mb: u64) {
        self.workbooks.set_text(labels::workbooks_label(workbooks));
        self.memory.set_text(labels::memory_label(committed_mb));
    }
}

pub fn installed(cx: &App) -> bool {
    cx.has_global::<Tray>()
}

// Called from inside an update, so the hide itself runs in a task of its own.
pub fn hide_main_window(cx: &mut App) {
    if !cx.has_global::<Tray>() {
        return;
    }
    let tray = cx.global_mut::<Tray>();
    tray.hide_requested = true;
    let window = tray.window;
    cx.spawn(async move |_| {
        if !window.hide() {
            tracing::warn!("the main window was not found to hide it");
        }
    })
    .detach();
}

pub fn request_show(cx: &App) {
    if let Some(tray) = cx.try_global::<Tray>() {
        send(&tray.events, TrayEvent::Show);
    }
}

pub fn note_visibility(visibility: WindowVisibility, cx: &mut App) {
    if !cx.has_global::<Tray>() {
        return;
    }
    let tray = cx.global_mut::<Tray>();
    let presence = tray
        .presence
        .after_visibility(visibility.is_visible(), tray.hide_requested);
    tray.show_presence(presence);
}

pub fn warn_another_zenkai_unreachable() {
    window::warn_blocking(t!("tray.unreachable.title"), t!("tray.unreachable.text"));
}

// The window's root is gpui-kit's, so the workspace is reached through its entity.
#[derive(Clone)]
struct MainWindow {
    window: AnyWindowHandle,
    workspace: WeakEntity<Workspace>,
}

impl MainWindow {
    fn update<R>(
        &self,
        cx: &mut impl AppContext,
        update: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) -> R,
    ) -> Option<R> {
        let updated = self.window.update(cx, |_, window, cx| {
            self.workspace
                .update(cx, |workspace, cx| update(workspace, window, cx))
        });
        match updated {
            Ok(Ok(result)) => Some(result),
            Ok(Err(error)) | Err(error) => {
                tracing::debug!(%error, "the main window is gone");
                None
            }
        }
    }
}

pub fn install(window: AnyWindowHandle, workspace: &Entity<Workspace>, claim: Claim, cx: &mut App) {
    let main = MainWindow {
        window,
        workspace: workspace.downgrade(),
    };
    let (events, received) = async_channel::unbounded();
    if let Claim::Primary(primary) = claim {
        instance::serve(primary, events.clone());
    }
    let native = main
        .update(cx, |_, window, _| NativeWindow::of(window))
        .flatten();
    let opener = events.clone();
    cx.on_action(move |_: &OpenZenkai, _| send(&opener, TrayEvent::Show));
    let built = native
        .ok_or(TrayError::NoWindowHandle)
        .and_then(|native| build(native, &events));
    match built {
        Ok(tray) => {
            cx.set_global(tray);
            cx.set_quit_mode(QuitMode::Explicit);
            cx.on_app_quit(|cx| {
                if cx.has_global::<Tray>() {
                    cx.remove_global::<Tray>();
                }
                async {}
            })
            .detach();
            refresh_summary(&main, cx);
        }
        Err(error) => tracing::warn!(%error, "no tray icon; closing the window will quit"),
    }
    cx.spawn(async move |cx| {
        while let Ok(first) = received.recv().await {
            let mut batch = vec![first];
            while let Ok(next) = received.try_recv() {
                batch.push(next);
            }
            handle(batch, &main, native, cx);
        }
    })
    .detach();
}

fn send(events: &async_channel::Sender<TrayEvent>, event: TrayEvent) {
    if events.try_send(event).is_err() {
        tracing::debug!("the tray event loop is gone");
    }
}

fn build(
    window: NativeWindow,
    events: &async_channel::Sender<TrayEvent>,
) -> Result<Tray, TrayError> {
    let create = |error: &dyn std::fmt::Display| TrayError::Create(error.to_string());
    let open = MenuItem::with_id(MenuId::new(OPEN_ID), t!("tray.open"), true, None);
    let workbooks = MenuItem::new(labels::workbooks_label(0), false, None);
    let memory = MenuItem::new(labels::memory_label(0), false, None);
    let quit = MenuItem::with_id(MenuId::new(QUIT_ID), t!("tray.quit"), true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &open,
        &PredefinedMenuItem::separator(),
        &workbooks,
        &memory,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|error| create(&error))?;

    let shown = icon::build(Presence::Shown)?;
    let hidden = icon::build(Presence::HiddenToTray)?;
    let icon = TrayIconBuilder::new()
        .with_icon(shown.clone())
        .with_tooltip(t!("tray.tooltip_shown"))
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|error| create(&error))?;

    let pointer_events = events.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let Some(event) = pointer_event(&event) {
            send(&pointer_events, event);
        }
    }));
    let menu_events = events.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some(event) = menu_event(event.id.as_ref()) {
            send(&menu_events, event);
        }
    }));
    Ok(Tray {
        icon,
        shown,
        hidden,
        workbooks,
        memory,
        presence: Presence::Shown,
        hide_requested: false,
        window,
        events: events.clone(),
    })
}

fn pointer_event(event: &TrayIconEvent) -> Option<TrayEvent> {
    match event {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
        | TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        } => Some(TrayEvent::Show),
        TrayIconEvent::Enter { .. } => Some(TrayEvent::Refresh),
        _ => None,
    }
}

fn menu_event(id: &str) -> Option<TrayEvent> {
    match id {
        OPEN_ID => Some(TrayEvent::Show),
        QUIT_ID => Some(TrayEvent::Quit),
        _ => None,
    }
}

fn refresh_summary(main: &MainWindow, cx: &mut impl AppContext) {
    main.update(cx, |workspace, _, cx| {
        let (workbooks, committed_mb) = workspace.tray_summary();
        if cx.has_global::<Tray>() {
            cx.global::<Tray>().show_summary(workbooks, committed_mb);
        }
    });
}

// Runs in a foreground task, outside any update. Events that queued up together raise the
// window once, so a burst of launches cannot take the focus over and over.
fn handle(
    batch: Vec<TrayEvent>,
    main: &MainWindow,
    native: Option<NativeWindow>,
    cx: &mut AsyncApp,
) {
    let (mut reveal_wanted, mut refresh_wanted, mut quit_wanted) = (false, false, false);
    let mut paths: Vec<PathBuf> = Vec::new();
    for event in batch {
        match event {
            TrayEvent::Show => reveal_wanted = true,
            TrayEvent::Refresh => refresh_wanted = true,
            TrayEvent::Quit => quit_wanted = true,
            TrayEvent::Open { path } => {
                reveal_wanted = true;
                if let Some(path) = path
                    && paths.len() < MAX_OPENS_AT_ONCE
                    && !paths.contains(&path)
                {
                    paths.push(path);
                }
            }
        }
    }
    if reveal_wanted {
        reveal(main, native, cx);
    } else if refresh_wanted {
        refresh_summary(main, cx);
    }
    for path in paths {
        main.update(cx, |workspace, window, cx| {
            workspace.open_path(path, window, cx)
        });
    }
    if quit_wanted {
        let needs_window = main.update(cx, |workspace, _, _| workspace.close_needs_window());
        if needs_window.unwrap_or(false) {
            reveal(main, native, cx);
        }
        main.update(cx, |workspace, window, cx| {
            workspace.request_close(CloseGoal::Quit, window, cx)
        });
    }
}

fn reveal(main: &MainWindow, native: Option<NativeWindow>, cx: &mut AsyncApp) {
    if let Some(native) = native
        && !native.show()
    {
        tracing::warn!("the main window was not found to show it");
    }
    main.update(cx, |_, window, cx| {
        if cx.has_global::<Tray>() {
            cx.global_mut::<Tray>().hide_requested = false;
        }
        window.activate_window();
    });
    refresh_summary(main, cx);
}

#[cfg(test)]
mod tests {
    use super::{OPEN_ID, QUIT_ID, TrayEvent, menu_event};

    #[test]
    fn menu_ids_map_to_commands() {
        assert!(matches!(menu_event(OPEN_ID), Some(TrayEvent::Show)));
        assert!(matches!(menu_event(QUIT_ID), Some(TrayEvent::Quit)));
        assert!(menu_event("other").is_none());
    }
}
