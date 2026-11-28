use gpui_kit::*;

// Tab wraps around inside a dialog: focus never lands on the window behind it. `stops` is
// how many tab stops the dialog holds.
pub fn cycle(dialog: &FocusHandle, stops: usize, forward: bool, window: &mut Window, cx: &mut App) {
    if forward {
        window.focus_next(cx);
        if !dialog.contains_focused(window, cx) {
            window.focus(dialog, cx);
            window.focus_next(cx);
        }
    } else {
        window.focus_prev(cx);
        if !dialog.contains_focused(window, cx) {
            window.focus(dialog, cx);
            for _ in 0..stops {
                window.focus_next(cx);
            }
        }
    }
}
