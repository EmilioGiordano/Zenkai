use async_channel::Sender;
use gpui_kit::*;

use super::prepare::{self, PrepareError};
use super::transfer::{Event, Outcome, Phase};
use super::{Updates, Version, download, notes, signature};

// Only an install laid out for the launcher, with a key to check what arrives, downloads an
// update; anywhere else "Update now" opens the Releases page.
pub fn can_download() -> bool {
    crate::installation::update_folder().is_some() && signature::has_trusted_key()
}

pub fn phase(cx: &App) -> Phase {
    cx.global::<Updates>().transfer.phase()
}

// Called from an observer of `Updates`, like `take_report`: it updates the global only when
// there is something to take.
pub fn take_outcome(cx: &mut App) -> Option<Outcome> {
    let outcome = cx.global::<Updates>().transfer.outcome?;
    cx.update_global::<Updates, _>(|updates, _| updates.transfer.outcome = None);
    Some(outcome)
}

pub fn cancel(cx: &mut App) -> bool {
    cx.update_global::<Updates, _>(|updates, _| updates.transfer.cancel())
}

// Starts only from "Update now". The download, the checks and the extraction run on their own
// thread; the UI hears about them through a channel and never waits.
pub fn start(cx: &mut App, chosen: Version) {
    let (Some(update), Some(installed)) =
        (crate::installation::update_folder(), Version::installed())
    else {
        tracing::error!("an update download needs the install folder and the installed version");
        return;
    };
    let older = prepare::fallbacks(
        notes(cx).newer_than(installed).map(|note| note.version),
        chosen,
        cx.global::<Updates>().record.restored,
    );
    let Some(cancel) = cx.update_global::<Updates, _>(|updates, _| updates.transfer.begin(chosen))
    else {
        return;
    };
    let watched = cancel.clone();
    let folder = update.clone();
    let (sender, receiver) = async_channel::unbounded();
    let spawned = std::thread::Builder::new()
        .name("update download".to_owned())
        .spawn(move || {
            let agent = download::agent();
            let result = prepare::prepare(&update, chosen, &older, installed, &cancel, |version| {
                send(&sender, Event::Trying(version));
                prepare::fetch_release(&agent, &update, version, &cancel, |received, total| {
                    send(&sender, Event::Received { received, total })
                })
            });
            send(&sender, Event::Finished(result));
        });
    if let Err(error) = spawned {
        cx.update_global::<Updates, _>(|updates, _| {
            updates
                .transfer
                .receive(Event::Finished(Err(PrepareError::Disk(error))))
        });
        return;
    }
    cx.spawn(async move |cx| {
        while let Ok(event) = receiver.recv().await {
            let late = cx.update(|cx| {
                cx.update_global::<Updates, _>(|updates, _| updates.transfer.receive(event))
            });
            if let Some(version) = late {
                let folder = folder.clone();
                cx.background_spawn(async move { prepare::discard(&folder, version) })
                    .await;
                cx.update(|cx| {
                    cx.update_global::<Updates, _>(|updates, _| updates.transfer.cleaned_up())
                });
            }
        }
        // Closed without `Finished` only if the thread panicked; otherwise this does nothing.
        cx.update(|cx| {
            cx.update_global::<Updates, _>(|updates, _| updates.transfer.lose(&watched))
        });
    })
    .detach();
}

fn send(sender: &Sender<Event>, event: Event) {
    if sender.send_blocking(event).is_err() {
        tracing::debug!("nothing listens to the update download any more");
    }
}

pub fn ready_for(cx: &App, wanted: Version) -> Option<Version> {
    cx.global::<Updates>().transfer.ready_for(wanted)
}
