use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winsafe::{EnumThreadWindows, GetCurrentThreadId, co, prelude::*};

pub fn handle_of(window: &Window) -> Option<isize> {
    // `Window` has its own `window_handle` for GPUI handles, so the trait is named in full.
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

// Must run on the thread that owns the window, and outside any GPUI update: the window
// procedure answers synchronously and draws the window when it is shown.
pub fn show(handle: isize) -> bool {
    apply(handle, co::SW::SHOW)
}

pub fn hide(handle: isize) -> bool {
    apply(handle, co::SW::HIDE)
}

// Matched by the exact handle among this thread's windows: the settings window and the
// tray's own message window live here too and must not be touched.
fn apply(handle: isize, command: co::SW) -> bool {
    let mut found = false;
    let walked = EnumThreadWindows(GetCurrentThreadId(), |candidate| {
        if candidate.ptr() as isize == handle {
            candidate.ShowWindow(command);
            found = true;
        }
        true
    });
    if let Err(error) = walked {
        tracing::warn!(%error, "could not list the windows of the interface thread");
    }
    found
}
