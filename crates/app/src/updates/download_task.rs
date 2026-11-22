use async_channel::Sender;
use gpui_kit::*;

use super::handoff::{self, StageError};
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
    let (Some(root), Some(update), Some(installed)) = (
        crate::installation::root(),
        crate::installation::update_folder(),
        Version::installed(),
    ) else {
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
            // Nothing is downloaded that this install could never put in place.
            let result = match handoff::blocked(root) {
                Ok(Some(blocked)) => Err(PrepareError::Blocked(chosen, blocked)),
                Err(error) => Err(PrepareError::Disk(error)),
                Ok(None) => {
                    let agent = download::agent();
                    prepare::prepare(&update, chosen, &older, installed, &cancel, |version| {
                        send(&sender, Event::Trying(version));
                        prepare::fetch_release(
                            &agent,
                            &update,
                            version,
                            &cancel,
                            |received, total| send(&sender, Event::Received { received, total }),
                        )
                    })
                }
            };
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

// "Restart now": the ready update is checked again in the background. None when no update is
// ready, or its checks already run.
pub fn stage(cx: &mut App) -> Option<(Version, Task<Result<(), StageError>>)> {
    let (Some(root), Some(installed)) = (crate::installation::root(), Version::installed()) else {
        tracing::error!("an update handoff needs the install folder and the installed version");
        return None;
    };
    let version = cx.update_global::<Updates, _>(|updates, _| updates.transfer.begin_staging())?;
    let staging = cx
        .background_executor()
        .spawn(async move { handoff::stage(root, version, installed) });
    Some((version, staging))
}

// A disk error leaves the update ready to try again; a refusal ends it.
pub fn end_stage(cx: &mut App, version: Version, staged: &Result<(), StageError>) {
    cx.update_global::<Updates, _>(|updates, _| match staged {
        Ok(()) | Err(StageError::Disk(_)) => updates.transfer.keep_staged(version),
        Err(StageError::Blocked(_) | StageError::NotVerified(_)) => {
            updates.transfer.drop_staged(version)
        }
    });
}

// Runs once the session is saved, right before the app quits. When the staged update is no
// longer the ready one, or the launcher does not start, the app restarts as it is, so the user
// is never left without it.
pub fn restart_into(version: Version, cx: &App) {
    let staged = cx.global::<Updates>().transfer.phase() == Phase::Ready(version);
    match crate::installation::root() {
        Some(root) if staged => match handoff::launch(root, version) {
            Ok(()) => return,
            Err(error) => tracing::error!(%error, "could not start the launcher; restarting as is"),
        },
        _ => tracing::warn!(%version, "the staged update is no longer ready; restarting as is"),
    }
    crate::restart::relaunch();
}
