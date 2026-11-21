use std::ffi::OsString;
use std::io;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::apply::apply;
use crate::arguments::{self, Request};
use crate::folders::Folders;
use crate::lock::{self, Taken};
use crate::message::{self, Message};
use crate::notice::{self, Notice};
use crate::settle::{Settled, settle};

pub struct Timing {
    pub lock: Duration,
    pub previous_exit: Duration,
    pub move_app: Duration,
    pub move_new: Duration,
    pub confirmation: Duration,
    pub poll: Duration,
}

const TIMING: Timing = Timing {
    lock: Duration::from_secs(90),
    previous_exit: Duration::from_secs(60),
    move_app: Duration::from_secs(10),
    move_new: Duration::from_secs(30),
    confirmation: Duration::from_secs(120),
    poll: Duration::from_millis(100),
};

#[derive(Debug)]
pub enum Finish {
    Open(Vec<Notice>),
    OpenAndTell(Message),
    Running,
    Tell(Message),
}

pub fn main() {
    let folders = match Folders::of_this_launcher() {
        Ok(folders) => folders,
        Err(error) => {
            message::show(&Message::CouldNotStart(error));
            return;
        }
    };
    crate::journal::init(&folders);
    let launch = arguments::parse(std::env::args_os().skip(1));
    if let Some(message) = run(&folders, launch.request, &launch.passed, &TIMING) {
        message::show(&message);
    }
}

// Returns what is left to tell the user, shown once the lock is released so that a message box
// left open never holds up another start.
pub fn run(
    folders: &Folders,
    request: Request,
    passed: &[OsString],
    timing: &Timing,
) -> Option<Message> {
    let _lock = match lock::take(&folders.lock(), timing.lock, timing.poll) {
        Taken::Held(lock) => lock,
        Taken::Busy => return Some(Message::Busy),
        Taken::Unavailable(error) => {
            tracing::warn!(%error, "no lock, so the install is left as it is");
            return open(folders, passed, &[]).err();
        }
    };
    let finish = match request {
        Request::Apply(version) => apply(folders, &version, passed, timing),
        Request::Open => open_as_settled(folders, timing),
        Request::Malformed => {
            tracing::warn!("ignored a malformed --apply argument");
            open_as_settled(folders, timing)
        }
    };
    tracing::info!(?finish, "done");
    match finish {
        Finish::Open(notices) => open(folders, passed, &notices).err(),
        Finish::OpenAndTell(message) => Some(open(folders, passed, &[]).err().unwrap_or(message)),
        Finish::Running => None,
        Finish::Tell(message) => Some(message),
    }
}

fn open_as_settled(folders: &Folders, timing: &Timing) -> Finish {
    settle(folders, timing).into()
}

impl From<Settled> for Finish {
    fn from(settled: Settled) -> Finish {
        match settled {
            Settled::Clean(notice) => Finish::Open(notice.into_iter().collect()),
            Settled::AsIs => Finish::Open(Vec::new()),
            Settled::NoApp(message) => Finish::Tell(message),
        }
    }
}

// The app outlives the launcher: dropping the child's handle does not end it.
fn open(folders: &Folders, passed: &[OsString], notices: &[Notice]) -> Result<(), Message> {
    if !notices.is_empty() {
        notice::leave(folders, notices);
    }
    open_app(folders, passed)
        .map(drop)
        .map_err(Message::CouldNotStart)
}

pub fn open_app(folders: &Folders, passed: &[OsString]) -> io::Result<Child> {
    Command::new(folders.program())
        .args(passed)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

#[cfg(test)]
pub const TEST_TIMING: Timing = Timing {
    lock: Duration::from_millis(300),
    previous_exit: Duration::from_millis(500),
    move_app: Duration::from_secs(2),
    move_new: Duration::from_secs(1),
    confirmation: Duration::from_secs(20),
    poll: Duration::from_millis(20),
};

#[cfg(test)]
mod tests;
