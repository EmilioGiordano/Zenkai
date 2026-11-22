use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon, Sizable, Theme};
use gpui_kit::*;
use zenkai_i18n::t;

use super::notices::NoticeKind;
use crate::actions::{
    CancelUpdate, DismissNotice, OpenReleases, RestartToUpdate, ShowWhatsNew, UpdateNow,
};
use crate::keymap;
use crate::updates::{self, DownloadPhase};

const BYTES_PER_TENTH_OF_MB: u64 = 100_000;

// Decimal megabytes with at most one decimal, as in the approved mockup: "12.4 MB of 31 MB".
fn megabytes(bytes: u64) -> String {
    let tenths = bytes.saturating_add(BYTES_PER_TENTH_OF_MB / 2) / BYTES_PER_TENTH_OF_MB;
    if tenths.is_multiple_of(10) {
        (tenths / 10).to_string()
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    }
}

pub(super) fn download_heading(
    kind: NoticeKind,
    theme: &Theme,
    cx: &App,
) -> Option<(SharedString, Icon)> {
    match kind {
        NoticeKind::UpdateDownloading(started) => {
            let version = match updates::download_phase(cx) {
                DownloadPhase::Downloading { version, .. } => version,
                DownloadPhase::Idle
                | DownloadPhase::Stopping
                | DownloadPhase::Ready(_)
                | DownloadPhase::Staging(_) => started,
            };
            Some((
                t!("update.downloading_title", version = version).into(),
                Icon::new(IconName::Download).text_color(theme.muted_foreground),
            ))
        }
        NoticeKind::UpdateReady(_) => Some((
            t!("update.ready_title").into(),
            Icon::new(IconName::CircleCheck).text_color(theme.success),
        )),
        NoticeKind::UpdateNotVerified(_) => Some((
            t!("update.not_verified_title").into(),
            Icon::new(IconName::TriangleAlert).text_color(theme.warning),
        )),
        NoticeKind::UpdateBlocked(_) => Some((
            t!("update.blocked_title").into(),
            Icon::new(IconName::TriangleAlert).text_color(theme.warning),
        )),
        NoticeKind::Updated(version) => Some((
            t!("update.updated_title", version = version).into(),
            Icon::new(IconName::CircleCheck).text_color(theme.success),
        )),
        NoticeKind::Plain | NoticeKind::UpdateAvailable(_) => None,
    }
}

fn later_button(cx: &App) -> Button {
    Button::new("update-later")
        .ghost()
        .small()
        .label(t!("update.later"))
        .tooltip(keymap::labeled(cx, t!("update.later"), &DismissNotice))
        .on_click(|_, window, cx| window.dispatch_action(Box::new(DismissNotice), cx))
}

fn button_row(buttons: Vec<Button>) -> AnyElement {
    h_flex()
        .justify_end()
        .gap_2()
        .children(buttons)
        .into_any_element()
}

pub(super) fn download_parts(kind: NoticeKind, cx: &App) -> Vec<AnyElement> {
    match kind {
        NoticeKind::UpdateDownloading(_) => {
            let cancel = Button::new("update-cancel")
                .small()
                .label(t!("update.cancel"))
                .tooltip(keymap::labeled(cx, t!("update.cancel"), &CancelUpdate))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(CancelUpdate), cx));
            let mut parts: Vec<AnyElement> = render_progress(updates::download_phase(cx), cx)
                .into_iter()
                .collect();
            parts.push(button_row(vec![cancel]));
            parts
        }
        NoticeKind::UpdateReady(_) => {
            let restart = Button::new("update-restart-now")
                .primary()
                .small()
                .label(t!("update.restart_now"))
                .tooltip(keymap::labeled(
                    cx,
                    t!("update.restart_now"),
                    &RestartToUpdate,
                ))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(RestartToUpdate), cx));
            vec![button_row(vec![later_button(cx), restart])]
        }
        NoticeKind::UpdateBlocked(_) => {
            let releases = Button::new("update-open-releases")
                .primary()
                .small()
                .label(t!("update.open_releases"))
                .tooltip(keymap::labeled(
                    cx,
                    t!("update.open_releases"),
                    &OpenReleases,
                ))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenReleases), cx));
            vec![button_row(vec![later_button(cx), releases])]
        }
        NoticeKind::Updated(_) => {
            let notes = Button::new("update-whats-new")
                .ghost()
                .small()
                .label(t!("whats_new.name"))
                .tooltip(keymap::labeled(cx, t!("whats_new.name"), &ShowWhatsNew))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowWhatsNew), cx));
            vec![button_row(vec![notes])]
        }
        NoticeKind::UpdateNotVerified(_) => {
            let again = Button::new("update-try-again")
                .primary()
                .small()
                .label(t!("update.try_again"))
                .tooltip(keymap::labeled(cx, t!("update.try_again"), &UpdateNow))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(UpdateNow), cx));
            vec![button_row(vec![later_button(cx), again])]
        }
        NoticeKind::Plain | NoticeKind::UpdateAvailable(_) => Vec::new(),
    }
}

pub(super) fn download_status(cx: &App) -> Option<AnyElement> {
    let phase = updates::download_phase(cx);
    let text: SharedString = match phase {
        DownloadPhase::Downloading { .. } => match phase.percent() {
            Some(percent) => t!("update.status_downloading", percent = percent).into(),
            None => t!("update.status_downloading_unknown").into(),
        },
        DownloadPhase::Staging(_) => t!("update.status_preparing").into(),
        DownloadPhase::Ready(_) => return Some(restart_status(cx)),
        DownloadPhase::Idle | DownloadPhase::Stopping => return None,
    };
    Some(
        h_flex()
            .gap_1()
            .items_center()
            .child(Icon::new(IconName::Download).size_3())
            .child(text)
            .into_any_element(),
    )
}

fn restart_status(cx: &App) -> AnyElement {
    let label = t!("update.status_restart");
    h_flex()
        .id("status-restart-to-update")
        .role(Role::Button)
        .aria_label(label)
        .gap_1()
        .items_center()
        .cursor_pointer()
        .text_color(cx.theme().primary)
        .tooltip(|window, cx| {
            Tooltip::new(keymap::labeled(
                cx,
                t!("update.restart_now"),
                &RestartToUpdate,
            ))
            .build(window, cx)
        })
        .on_click(|_, window, cx| window.dispatch_action(Box::new(RestartToUpdate), cx))
        .child(Icon::new(IconName::CircleCheck).size_3())
        .child(label)
        .into_any_element()
}

fn render_progress(phase: DownloadPhase, cx: &App) -> Option<AnyElement> {
    let DownloadPhase::Downloading {
        received, total, ..
    } = phase
    else {
        return None;
    };
    let percent = phase.percent();
    let size: SharedString = match total {
        Some(total) => t!(
            "update.download_size",
            received = megabytes(received),
            total = megabytes(total)
        )
        .into(),
        None => t!("update.download_received", received = megabytes(received)).into(),
    };
    let shown_percent: Option<SharedString> =
        percent.map(|percent| t!("update.download_percent", percent = percent).into());
    Some(
        v_flex()
            .gap_1()
            .child(
                Progress::new("update-download-progress")
                    .small()
                    .value(f32::from(percent.unwrap_or(0)))
                    .loading(percent.is_none())
                    .accessibility_label(t!("update.download_progress")),
            )
            .child(
                h_flex()
                    .justify_between()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(size)
                    .children(shown_percent),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::megabytes;

    #[test]
    fn sizes_show_in_megabytes_with_at_most_one_decimal() {
        assert_eq!(megabytes(0), "0");
        assert_eq!(megabytes(12_400_000), "12.4");
        assert_eq!(megabytes(31_000_000), "31");
        assert_eq!(megabytes(30_980_000), "31");
        assert_eq!(megabytes(49_999), "0");
        assert_eq!(megabytes(50_000), "0.1");
        assert_eq!(megabytes(u64::MAX), "18446744073709.5");
    }
}
