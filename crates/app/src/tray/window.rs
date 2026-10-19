use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winsafe::{EnumThreadWindows, GetCurrentThreadId, HWND, co, prelude::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeWindow(isize);

impl NativeWindow {
    pub fn of(window: &Window) -> Option<NativeWindow> {
        // `Window` has its own `window_handle` for GPUI handles, so the trait is named in full.
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(NativeWindow(handle.hwnd.get())),
            _ => None,
        }
    }

    // Must run outside any GPUI update: Windows calls into GPUI synchronously and draws the
    // window when it is shown.
    pub fn show(self) -> bool {
        self.apply(co::SW::SHOW)
    }

    pub fn hide(self) -> bool {
        self.apply(co::SW::HIDE)
    }

    fn apply(self, command: co::SW) -> bool {
        let mut found = false;
        let walked = EnumThreadWindows(GetCurrentThreadId(), |candidate| {
            if candidate.ptr() as isize == self.0 {
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
}

pub fn warn_blocking(title: &str, text: &str) {
    if let Err(error) = HWND::NULL.MessageBox(text, title, co::MB::OK | co::MB::ICONWARNING) {
        tracing::warn!(%error, "could not show the message box");
    }
}
