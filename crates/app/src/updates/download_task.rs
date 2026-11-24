use async_channel::{Receiver, Sender};
use gpui_kit::*;

use super::handoff::{self, StageError};
use super::prepare::{self, PrepareError};
use super::transfer::{Event, NextStart, Outcome, Phase, StageEnd};
use super::{Updates, Version, download, notes};

pub fn phase(cx: &App) -> Phase {
    cx.global::<Updates>().transfer.phase()
}

// Called from an observer of `Updates`, like `take_report`: it updates the global only when
// there is something to take.
pub fn take_outcome(cx: &mut App) -> Option<Outcome> {
    cx.global::<Updates>().transfer.outcome.as_ref()?;
    cx.update_global::<Updates, _>(|updates, _| updates.transfer.outcome.take())
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
                Err(error) => Err(PrepareError::InstallCheck(error)),
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

// "Restart now": the ready update is checked again on its own thread. None when no update is
// ready, or its checks already run. The receiver gets the result once the transfer has heard it,
// so the check always ends, even with no window left to show it.
pub fn stage(cx: &mut App) -> Option<(Version, Receiver<Result<(), StageError>>)> {
    let (Some(root), Some(installed)) = (crate::installation::root(), Version::installed()) else {
        tracing::error!("an update handoff needs the install folder and the installed version");
        return None;
    };
    let version = cx.update_global::<Updates, _>(|updates, _| updates.transfer.begin_staging())?;
    let (done, checked) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("update staging".to_owned())
        .spawn(move || {
            if done
                .send_blocking(handoff::stage(root, version, installed))
                .is_err()
            {
                tracing::debug!("nothing listens to the update checks any more");
            }
        });
    if let Err(error) = spawned {
        tracing::error!(%error, "could not start the update checks");
    }
    let (sender, receiver) = async_channel::bounded(1);
    cx.spawn(async move |cx| {
        // Closed without an answer only if the thread panicked or never started.
        let staged = checked.recv().await.unwrap_or(Err(StageError::Lost));
        cx.update(|cx| end_stage(cx, version, &staged));
        if sender.send(staged).await.is_err() {
            tracing::debug!("no window waits for the update checks any more");
        }
    })
    .detach();
    Some((version, receiver))
}

fn end_stage(cx: &mut App, version: Version, staged: &Result<(), StageError>) {
    let end = match staged {
        Ok(()) => StageEnd::Staged,
        Err(StageError::Disk(_) | StageError::Lost) => StageEnd::Retry,
        Err(StageError::Gone(_) | StageError::Blocked(_) | StageError::NotVerified(_)) => {
            StageEnd::Gone
        }
    };
    cx.update_global::<Updates, _>(|updates, _| updates.transfer.end_staging(version, end));
}

// Called whenever a close of the app is called off: the update waits and is checked again.
pub fn unstage(cx: &mut App) {
    if matches!(cx.global::<Updates>().transfer.phase(), Phase::Staged(_)) {
        cx.update_global::<Updates, _>(|updates, _| updates.transfer.unstage());
    }
}

// Runs once the session is saved, right before the app quits. Only the update staged for this
// close goes to the launcher; otherwise, or when the launcher does not start, the app restarts as
// it is, so the user is never left without it.
pub fn restart_into(version: Version, cx: &App) {
    let next = cx.global::<Updates>().transfer.next_start(version);
    match (next, crate::installation::root()) {
        (NextStart::Launcher, Some(root)) => match handoff::launch(root, version) {
            Ok(()) => return,
            Err(error) => tracing::error!(%error, "could not start the launcher; restarting as is"),
        },
        _ => tracing::warn!(%version, "the update is not staged; restarting as is"),
    }
    crate::restart::relaunch();
}
