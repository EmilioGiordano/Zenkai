// The client must turn every way a child can end into an error and leave no child behind:
// aborts, early exits, garbage, a protocol breach, a flood, a hang and a cancel.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use zenkai_values_host::client::{ReadError, ValuesClient};

fn fake(mode: &str) -> ValuesClient {
    ValuesClient::new(Some(PathBuf::from(env!("CARGO_BIN_EXE_fake-values-host"))))
        .with_environment("ZENKAI_FAKE_VALUES_HOST", mode)
        .with_deadline(Duration::from_secs(2))
}

fn read(client: &ValuesClient) -> Result<usize, ReadError> {
    client
        .read(Path::new("ignored.xls"), &AtomicBool::new(false))
        .map(|sheets| sheets.len())
}

#[test]
fn a_child_that_finishes_gives_its_result() {
    assert_eq!(read(&fake("done")).unwrap(), 0);
}

#[test]
fn a_child_that_dies_or_misbehaves_is_a_broken_host_error() {
    for mode in [
        "abort",
        "exit",
        "garbage",
        "block_without_sheet",
        "empty_block",
        "flood",
    ] {
        let outcome = read(&fake(mode));
        assert!(
            matches!(outcome, Err(ReadError::HostBroken(_))),
            "{mode}: {outcome:?}"
        );
    }
}

#[test]
fn a_host_that_aborts_fails_only_its_own_read() {
    assert!(matches!(
        read(&fake("abort")),
        Err(ReadError::HostBroken(_))
    ));
    assert_eq!(read(&fake("done")).unwrap(), 0);
}

#[test]
fn a_host_that_cannot_be_located_is_a_missing_host() {
    assert!(matches!(
        read(&ValuesClient::new(None)),
        Err(ReadError::HostMissing)
    ));
}

#[test]
fn a_child_that_sends_more_bytes_than_the_budget_is_refused() {
    let client = fake("big_block").with_payload_budget(100);
    assert!(matches!(read(&client), Err(ReadError::TooLarge)));
    assert_eq!(read(&fake("big_block")).unwrap(), 1);
}

#[test]
fn a_missing_host_is_its_own_error() {
    let client = ValuesClient::new(Some(PathBuf::from("no-such-values-host.exe")));
    assert!(matches!(read(&client), Err(ReadError::HostMissing)));
}

#[test]
fn a_child_that_hangs_is_stopped_at_the_deadline_and_killed() {
    for mode in ["hang", "hang_after_sheet"] {
        let pid_file = tempfile::NamedTempFile::new().unwrap();
        let client = fake(mode).with_environment(
            "ZENKAI_FAKE_VALUES_HOST_PID_FILE",
            &pid_file.path().to_string_lossy(),
        );
        let started = Instant::now();
        let outcome = read(&client);
        assert!(matches!(outcome, Err(ReadError::TimedOut)), "{mode}");
        assert!(started.elapsed() < Duration::from_secs(20), "{mode}");
        let pid = std::fs::read_to_string(pid_file.path()).unwrap();
        assert!(
            !is_running(pid.trim()),
            "{mode}: the child is still running"
        );
    }
}

#[test]
fn a_cancel_stops_a_hanging_child() {
    let cancel = AtomicBool::new(false);
    let client = fake("hang").with_deadline(Duration::from_secs(60));
    let outcome = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(400));
            cancel.store(true, Ordering::Relaxed);
        });
        client.read(Path::new("ignored.xls"), &cancel)
    });
    assert!(matches!(outcome, Err(ReadError::Cancelled)));
}

fn is_running(pid: &str) -> bool {
    #[cfg(windows)]
    {
        let listing = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&listing.stdout).contains(pid)
    }
    #[cfg(not(windows))]
    {
        Command::new("kill")
            .args(["-0", pid])
            .status()
            .unwrap()
            .success()
    }
}
