use crate::arguments::Version;
use crate::disk::{self, Presence, Running};
use crate::folders::Folders;
use crate::message::Message;
use crate::notice::Notice;
use crate::start::Timing;

// One row of the state table in docs/SPEC.md, "Reemplazo y reinicio".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    Normal,
    Interrupted,
    Unfinished,
    Starting,
    NotStarted,
    Unknown,
    Missing,
}

// Nothing is ever moved on a state that is not known for sure: the app is opened as it is and the
// next start decides.
pub fn plan(
    app: Presence,
    old: Presence,
    marker: impl FnOnce() -> Presence,
    running: impl FnOnce() -> Running,
) -> Plan {
    use Presence::{Absent, Present};
    match (app, old) {
        (Presence::Unknown, _) | (_, Presence::Unknown) => Plan::Unknown,
        (Present, Absent) => Plan::Normal,
        (Absent, Present) => Plan::Interrupted,
        (Absent, Absent) => Plan::Missing,
        (Present, Present) => match marker() {
            Present => Plan::Unfinished,
            Presence::Unknown => Plan::Unknown,
            Absent => match running() {
                Running::Yes => Plan::Starting,
                Running::No => Plan::NotStarted,
                Running::Unknown => Plan::Unknown,
            },
        },
    }
}

#[derive(Debug)]
pub enum Settled {
    Clean(Option<Notice>),
    AsIs,
    NoApp(Message),
}

pub fn settle(folders: &Folders, timing: &Timing) -> Settled {
    let plan = plan(
        disk::presence(&folders.app()),
        disk::presence(&folders.old()),
        || disk::marker(folders),
        || disk::running(&folders.program()),
    );
    tracing::info!(?plan, "read the install");
    match plan {
        Plan::Normal => Settled::Clean(None),
        Plan::Interrupted => match restore(folders, timing) {
            Ok(()) => {
                disk::forget_applying(folders);
                Settled::Clean(Some(Notice::Interrupted))
            }
            Err(message) => Settled::NoApp(message),
        },
        Plan::Unfinished => match disk::finish_update(folders) {
            Ok(()) => Settled::Clean(None),
            Err(error) => {
                tracing::warn!(%error, "could not finish cleaning up after the update");
                if disk::presence(&folders.old()) == Presence::Absent {
                    Settled::Clean(None)
                } else {
                    Settled::AsIs
                }
            }
        },
        Plan::Starting | Plan::Unknown => Settled::AsIs,
        Plan::NotStarted => reject(folders, disk::applying_version(folders), timing),
        Plan::Missing => Settled::NoApp(Message::Reinstall),
    }
}

// The version in `app` did not start: it moves aside and the previous one comes back. If it
// cannot move, nothing is restored and the next start decides.
pub fn reject(folders: &Folders, version: Option<Version>, timing: &Timing) -> Settled {
    let rejected = folders.rejected();
    if let Err(error) = disk::remove_tree(&rejected) {
        tracing::warn!(%error, "could not clear an earlier rejected folder");
    }
    if let Err(error) =
        disk::rename_retrying(&folders.app(), &rejected, timing.move_app, timing.poll)
    {
        tracing::error!(%error, "could not move aside the version that did not start");
        return Settled::AsIs;
    }
    match restore(folders, timing) {
        Ok(()) => {
            disk::forget_applying(folders);
            Settled::Clean(Some(Notice::Restored(version)))
        }
        Err(message) => Settled::NoApp(message),
    }
}

pub fn restore(folders: &Folders, timing: &Timing) -> Result<(), Message> {
    disk::rename_retrying(&folders.old(), &folders.app(), timing.move_app, timing.poll).map_err(
        |error| {
            tracing::error!(%error, "could not restore the previous version");
            Message::Interrupted
        },
    )
}

#[cfg(test)]
mod tests;
