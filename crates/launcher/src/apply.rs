use std::ffi::OsString;
use std::process::Child;
use std::thread::sleep;
use std::time::Instant;

use crate::arguments::Version;
use crate::disk::{self, Running};
use crate::folders::Folders;
use crate::message::Message;
use crate::notice::Notice;
use crate::settle::{self, Settled};
use crate::start::{self, Finish, Timing};

// The steps of "Aplicar una actualización" in docs/SPEC.md, run while the lock is held.
pub fn apply(folders: &Folders, version: &Version, passed: &[OsString], timing: &Timing) -> Finish {
    tracing::info!(%version, "applying an update");
    if !previous_app_closed(folders, timing) {
        tracing::warn!("the previous version did not close; nothing was touched");
        return Finish::OpenAndTell(Message::NotClosed);
    }
    let not_applied = || Finish::Open(Some(Notice::NotApplied(version.clone())));
    match settle::settle(folders, timing) {
        Settled::Clean(_) => {}
        Settled::AsIs => return not_applied(),
        Settled::NoApp(message) => return Finish::Tell(message),
    }
    if let Err(problem) = disk::check_new_app(folders, version) {
        tracing::error!(%problem, "the new version cannot be swapped in");
        return not_applied();
    }
    if let Err(error) = disk::remove_marker(folders) {
        tracing::error!(%error, "could not remove an earlier confirmation marker");
        return not_applied();
    }
    if let Err(error) = disk::record_applying(folders, version) {
        tracing::error!(%error, "could not record the version being applied");
        return not_applied();
    }
    let new_app = folders.new_app(version);
    if let Err(error) =
        disk::rename_retrying(&folders.app(), &folders.old(), timing.move_app, timing.poll)
    {
        tracing::error!(%error, "something still runs from the app folder");
        disk::forget_applying(folders);
        return not_applied();
    }
    if let Err(error) =
        disk::rename_retrying(&new_app, &folders.app(), timing.move_new, timing.poll)
    {
        tracing::error!(%error, "could not move the new version into place");
        return match settle::restore(folders, timing) {
            Ok(()) => {
                disk::forget_applying(folders);
                not_applied()
            }
            Err(message) => Finish::Tell(message),
        };
    }
    let started = match start::open_app(folders, passed) {
        Ok(child) => child,
        Err(error) => {
            tracing::error!(%error, "the new version could not start");
            return finish_rejected(folders, version, timing);
        }
    };
    if confirmed(folders, version, started, timing) {
        if let Err(error) = disk::finish_update(folders) {
            tracing::warn!(%error, "could not finish cleaning up; the next start retries");
        }
        tracing::info!(%version, "the update is in place");
        return Finish::Running;
    }
    finish_rejected(folders, version, timing)
}

fn previous_app_closed(folders: &Folders, timing: &Timing) -> bool {
    let deadline = Instant::now() + timing.previous_exit;
    while disk::running(&folders.program()) == Running::Yes {
        if Instant::now() >= deadline {
            return false;
        }
        sleep(timing.poll);
    }
    true
}

// The marker must name this very version. Without it, the new version is ended if it still runs:
// it is the launcher's own child, and it never showed a window.
fn confirmed(folders: &Folders, version: &Version, mut started: Child, timing: &Timing) -> bool {
    let deadline = Instant::now() + timing.confirmation;
    loop {
        if disk::confirms(folders, version) {
            return true;
        }
        match started.try_wait() {
            Ok(Some(status)) => {
                tracing::warn!(%status, "the new version ended before it confirmed");
                return disk::confirms(folders, version);
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "could not tell whether the new version runs"),
        }
        if Instant::now() >= deadline {
            tracing::warn!("the new version did not confirm in time; ending it");
            if let Err(error) = started.kill() {
                tracing::warn!(%error, "could not end the new version");
            }
            if let Err(error) = started.wait() {
                tracing::warn!(%error, "could not wait for the new version to end");
            }
            return false;
        }
        sleep(timing.poll);
    }
}

fn finish_rejected(folders: &Folders, version: &Version, timing: &Timing) -> Finish {
    settle::reject(folders, Some(version.clone()), timing).into()
}

#[cfg(test)]
mod tests;
