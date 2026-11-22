use std::sync::Arc;

use super::launcher_notice::LauncherNotice;
use super::notes::Notes;
use super::record::Record;
use super::releases::Releases;
#[cfg(windows)]
use super::transfer::Transfer;
use super::version::{Version, newer_than};

// Ordered by how much the user asked to hear: a stronger request joins a running check.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Source {
    Startup,
    Settings,
    Palette,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Unchecked,
    Checking,
    UpToDate { at: u64 },
    Available(Version),
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Report {
    Available(Version),
    UpToDate,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsReading {
    Loading,
    Broken,
    Loaded { check_for_updates: bool },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StartupCheck {
    Wait,
    Skip,
    Run,
}

// The newest version above the installed one that is known to exist: what the last check found,
// or, while a check runs or after one failed, the newest in the notes kept from before.
pub fn known_update(status: Status, notes: &Notes, installed: Version) -> Option<Version> {
    match status {
        Status::Available(version) => Some(version),
        Status::Checking | Status::Failed => {
            notes.newer_than(installed).next().map(|note| note.version)
        }
        Status::Unchecked | Status::UpToDate { .. } => None,
    }
}

// What one save writes. `notes` is there only when a check replaced them, so announcing a
// version does not rewrite a file of up to a megabyte or more.
pub struct Snapshot {
    pub record: Record,
    pub notes: Option<Arc<Notes>>,
    pub order: u64,
}

pub struct Updates {
    pub record: Record,
    pub report: Option<Report>,
    pub launcher_notices: Vec<LauncherNotice>,
    #[cfg(windows)]
    pub transfer: Transfer,
    notes: Arc<Notes>,
    notes_from_check: bool,
    notes_unsaved: bool,
    saves_started: u64,
    record_loaded: bool,
    startup_decided: bool,
    status: Status,
    status_before_check: Status,
    source: Source,
}

impl Default for Updates {
    fn default() -> Updates {
        Updates {
            record: Record::default(),
            report: None,
            launcher_notices: Vec::new(),
            #[cfg(windows)]
            transfer: Transfer::default(),
            notes: Arc::default(),
            notes_from_check: false,
            notes_unsaved: false,
            saves_started: 0,
            record_loaded: false,
            startup_decided: false,
            status: Status::Unchecked,
            status_before_check: Status::Unchecked,
            source: Source::Startup,
        }
    }
}

impl Updates {
    pub fn startup_pending(&self) -> bool {
        self.record_loaded && !self.startup_decided
    }

    pub fn status(&self) -> Status {
        self.status
    }

    // What the last answer on disk says, so the status line is right after a restart.
    pub fn load_record(&mut self, record: Record, installed: Version) {
        self.record = record;
        self.record_loaded = true;
        if self.status == Status::Unchecked {
            self.status = self.status_from_record(installed);
        }
    }

    pub fn notes(&self) -> Arc<Notes> {
        self.notes.clone()
    }

    // The cache is read in the background and can land after a check that already found newer
    // notes: those stay.
    pub fn load_notes(&mut self, cached: Notes) {
        if !self.notes_from_check {
            self.notes = Arc::new(cached);
        }
    }

    fn status_from_record(&self, installed: Version) -> Status {
        match self.record.last_success {
            None => Status::Unchecked,
            Some(at) => match newer_than(self.record.newest, installed) {
                Some(found) => Status::Available(found),
                None => Status::UpToDate { at },
            },
        }
    }

    // The settings file loads on its own schedule, and a broken one says nothing about the
    // user's choice: neither case may start a request.
    pub fn startup_check(&mut self, settings: SettingsReading, now: Option<u64>) -> StartupCheck {
        if self.startup_decided || !self.record_loaded {
            return StartupCheck::Wait;
        }
        let check_for_updates = match settings {
            SettingsReading::Loading => return StartupCheck::Wait,
            SettingsReading::Broken => false,
            SettingsReading::Loaded { check_for_updates } => check_for_updates,
        };
        self.startup_decided = true;
        let due = now.is_some_and(|now| self.record.is_due(now));
        if check_for_updates && due {
            StartupCheck::Run
        } else {
            StartupCheck::Skip
        }
    }

    // False when a check is already running; a stronger request then joins it, so the user
    // who asked gets the answer.
    pub fn begin_check(&mut self, source: Source) -> bool {
        if self.status == Status::Checking {
            self.source = self.source.max(source);
            return false;
        }
        self.source = source;
        self.status_before_check = self.status;
        self.status = Status::Checking;
        true
    }

    pub fn finish_ok(&mut self, releases: Releases, installed: Version, now: u64) {
        self.record.last_check = Some(now);
        self.record.last_success = Some(now);
        self.record.newest = releases.newest;
        self.notes = Arc::new(releases.notes);
        self.notes_from_check = true;
        self.notes_unsaved = true;
        match newer_than(releases.newest, installed) {
            Some(found) => {
                self.status = Status::Available(found);
                if self.source != Source::Startup || self.record.should_announce(found) {
                    self.report = Some(Report::Available(found));
                }
            }
            None => {
                self.status = Status::UpToDate { at: now };
                if self.source == Source::Palette {
                    self.report = Some(Report::UpToDate);
                }
            }
        }
    }

    pub fn finish_failed(&mut self, now: u64) {
        match self.source {
            // An automatic check that fails waits for tomorrow and says nothing.
            Source::Startup => {
                self.record.last_check = Some(now);
                self.status = self.status_before_check;
            }
            Source::Settings => self.status = Status::Failed,
            Source::Palette => {
                self.status = Status::Failed;
                self.report = Some(Report::Failed);
            }
        }
    }

    // A check that cannot start is a failure only for the user who asked for it.
    pub fn fail_to_start(&mut self, source: Source) {
        if self.status == Status::Checking {
            return;
        }
        match source {
            Source::Startup => {}
            Source::Settings => self.status = Status::Failed,
            Source::Palette => {
                self.status = Status::Failed;
                self.report = Some(Report::Failed);
            }
        }
    }

    // Saves run on background threads in any order: the number lets the writer drop a stale snapshot.
    pub fn snapshot_for_save(&mut self) -> Snapshot {
        self.saves_started += 1;
        let notes = self.notes_unsaved.then(|| self.notes.clone());
        Snapshot {
            record: self.record.clone(),
            notes,
            order: self.saves_started,
        }
    }

    // The notes stay owed until a write of this very list lands, so a failed write is tried
    // again with the next snapshot; a newer list that arrived meanwhile still owes its own.
    pub fn notes_saved(&mut self, saved: &Arc<Notes>) {
        if Arc::ptr_eq(&self.notes, saved) {
            self.notes_unsaved = false;
        }
    }

    pub fn mark_announced(&mut self, version: Version) {
        self.record.announced = Some(version);
    }

    // True when the record changed and needs saving: a version the launcher had to restore from
    // is not offered again on its own.
    pub fn receive_launcher_notice(&mut self, notice: LauncherNotice) -> bool {
        self.launcher_notices.push(notice);
        match notice {
            LauncherNotice::Restored(Some(version)) => {
                self.record.restored = Some(version);
                true
            }
            LauncherNotice::Restored(None)
            | LauncherNotice::Interrupted
            | LauncherNotice::NotApplied(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::notes::Note;
    use super::{
        LauncherNotice, Notes, Record, Releases, Report, SettingsReading, Source, StartupCheck,
        Status, Updates, Version, known_update,
    };

    fn version(text: &str) -> Version {
        Version::parse_bare(text).unwrap()
    }

    fn installed() -> Version {
        version("0.1.0")
    }

    fn found(newest: Option<Version>) -> Releases {
        Releases {
            newest,
            notes: Notes::default(),
        }
    }

    fn loaded(record: Record) -> Updates {
        let mut updates = Updates::default();
        updates.load_record(record, installed());
        updates
    }

    fn checking(updates: &mut Updates, source: Source) {
        assert!(updates.begin_check(source));
    }

    const ON: SettingsReading = SettingsReading::Loaded {
        check_for_updates: true,
    };

    #[test]
    fn a_broken_settings_file_never_starts_a_check() {
        let mut updates = loaded(Record::default());
        assert_eq!(
            updates.startup_check(SettingsReading::Broken, Some(100)),
            StartupCheck::Skip
        );
        assert_eq!(updates.startup_check(ON, Some(100)), StartupCheck::Wait);
    }

    #[test]
    fn the_startup_check_waits_for_the_settings_and_the_record() {
        let mut updates = Updates::default();
        assert_eq!(updates.startup_check(ON, Some(100)), StartupCheck::Wait);
        updates.load_record(Record::default(), installed());
        assert_eq!(
            updates.startup_check(SettingsReading::Loading, Some(100)),
            StartupCheck::Wait
        );
        assert_eq!(updates.startup_check(ON, Some(100)), StartupCheck::Run);
    }

    #[test]
    fn the_startup_check_respects_the_switch_and_the_day() {
        let mut off = loaded(Record::default());
        let off_reading = SettingsReading::Loaded {
            check_for_updates: false,
        };
        assert_eq!(
            off.startup_check(off_reading, Some(100)),
            StartupCheck::Skip
        );

        let mut recent = loaded(Record {
            last_check: Some(100),
            ..Record::default()
        });
        assert_eq!(recent.startup_check(ON, Some(200)), StartupCheck::Skip);

        let mut no_clock = loaded(Record::default());
        assert_eq!(no_clock.startup_check(ON, None), StartupCheck::Skip);
    }

    #[test]
    fn the_startup_check_is_decided_once() {
        let mut updates = loaded(Record::default());
        assert_eq!(updates.startup_check(ON, Some(100)), StartupCheck::Run);
        assert_eq!(updates.startup_check(ON, Some(100)), StartupCheck::Wait);
    }

    #[test]
    fn the_status_after_a_restart_comes_from_the_record() {
        let up_to_date = loaded(Record {
            last_success: Some(50),
            newest: Some(installed()),
            ..Record::default()
        });
        assert_eq!(up_to_date.status(), Status::UpToDate { at: 50 });

        let behind = loaded(Record {
            last_success: Some(50),
            newest: Some(version("0.2.0")),
            ..Record::default()
        });
        assert_eq!(behind.status(), Status::Available(version("0.2.0")));

        assert_eq!(loaded(Record::default()).status(), Status::Unchecked);
    }

    #[test]
    fn a_failed_automatic_check_keeps_the_earlier_answer() {
        let mut updates = loaded(Record {
            last_success: Some(50),
            newest: Some(installed()),
            ..Record::default()
        });
        checking(&mut updates, Source::Startup);
        assert_eq!(updates.status(), Status::Checking);
        updates.finish_failed(900);
        assert_eq!(updates.status(), Status::UpToDate { at: 50 });
        assert_eq!(updates.record.last_check, Some(900));
        assert_eq!(updates.report, None);
    }

    #[test]
    fn a_failed_manual_check_says_so() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Settings);
        updates.finish_failed(900);
        assert_eq!(updates.status(), Status::Failed);
        assert_eq!(updates.record.last_check, None);
        assert_eq!(updates.report, None);

        checking(&mut updates, Source::Palette);
        updates.finish_failed(900);
        assert_eq!(updates.report, Some(Report::Failed));
    }

    #[test]
    fn a_manual_check_that_cannot_start_says_so() {
        let mut settings = loaded(Record::default());
        settings.fail_to_start(Source::Settings);
        assert_eq!(settings.status(), Status::Failed);
        assert_eq!(settings.report, None);

        let mut palette = loaded(Record::default());
        palette.fail_to_start(Source::Palette);
        assert_eq!(palette.status(), Status::Failed);
        assert_eq!(palette.report, Some(Report::Failed));
    }

    #[test]
    fn an_automatic_check_that_cannot_start_stays_quiet() {
        let mut updates = loaded(Record::default());
        updates.fail_to_start(Source::Startup);
        assert_eq!(updates.status(), Status::Unchecked);
        assert_eq!(updates.report, None);
    }

    #[test]
    fn a_request_that_cannot_start_leaves_a_running_check_alone() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Startup);
        updates.fail_to_start(Source::Palette);
        assert_eq!(updates.status(), Status::Checking);
        assert_eq!(updates.report, None);
    }

    #[test]
    fn a_second_request_does_not_start_another_check() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Startup);
        assert!(!updates.begin_check(Source::Settings));
    }

    #[test]
    fn a_manual_request_during_the_automatic_check_gets_its_answer() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Startup);
        assert!(!updates.begin_check(Source::Palette));
        updates.finish_ok(found(Some(installed())), installed(), 900);
        assert_eq!(updates.report, Some(Report::UpToDate));
    }

    #[test]
    fn a_weaker_request_does_not_downgrade_a_running_check() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Palette);
        assert!(!updates.begin_check(Source::Startup));
        updates.finish_failed(900);
        assert_eq!(updates.report, Some(Report::Failed));
    }

    #[test]
    fn a_found_version_is_reported_but_not_yet_announced() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Startup);
        updates.finish_ok(found(Some(version("0.1.1"))), installed(), 900);
        assert_eq!(updates.report, Some(Report::Available(version("0.1.1"))));
        assert_eq!(updates.record.announced, None);
        assert_eq!(updates.status(), Status::Available(version("0.1.1")));
    }

    #[test]
    fn the_automatic_check_stays_quiet_about_an_announced_version() {
        let mut updates = loaded(Record {
            announced: Some(version("0.1.1")),
            ..Record::default()
        });
        checking(&mut updates, Source::Startup);
        updates.finish_ok(found(Some(version("0.1.1"))), installed(), 900);
        assert_eq!(updates.report, None);
        assert_eq!(updates.status(), Status::Available(version("0.1.1")));
    }

    #[test]
    fn a_check_the_user_asked_for_always_reports() {
        let mut updates = loaded(Record {
            announced: Some(version("0.1.1")),
            ..Record::default()
        });
        checking(&mut updates, Source::Palette);
        updates.finish_ok(found(Some(version("0.1.1"))), installed(), 900);
        assert_eq!(updates.report, Some(Report::Available(version("0.1.1"))));
    }

    #[test]
    fn a_version_the_launcher_restored_from_is_offered_only_when_asked() {
        let mut updates = loaded(Record::default());
        assert!(updates.receive_launcher_notice(LauncherNotice::Restored(Some(version("0.1.1")))));
        assert_eq!(
            updates.launcher_notices,
            [LauncherNotice::Restored(Some(version("0.1.1")))]
        );
        checking(&mut updates, Source::Startup);
        updates.finish_ok(found(Some(version("0.1.1"))), installed(), 900);
        assert_eq!(updates.report, None);
        checking(&mut updates, Source::Palette);
        updates.finish_ok(found(Some(version("0.1.1"))), installed(), 901);
        assert_eq!(updates.report, Some(Report::Available(version("0.1.1"))));
    }

    #[test]
    fn other_launcher_notices_leave_the_record_alone() {
        for notice in [
            LauncherNotice::Restored(None),
            LauncherNotice::Interrupted,
            LauncherNotice::NotApplied(version("0.1.1")),
        ] {
            let mut updates = loaded(Record::default());
            assert!(!updates.receive_launcher_notice(notice));
            assert_eq!(updates.record, Record::default());
            assert_eq!(updates.launcher_notices, [notice]);
        }
    }

    #[test]
    fn a_bogus_high_tag_that_goes_away_does_not_mute_the_real_update() {
        let mut updates = loaded(Record {
            announced: Some(version("9.9.9")),
            ..Record::default()
        });
        checking(&mut updates, Source::Startup);
        updates.finish_ok(found(Some(version("0.1.1"))), installed(), 900);
        assert_eq!(updates.report, Some(Report::Available(version("0.1.1"))));
    }

    #[test]
    fn up_to_date_is_reported_only_to_the_palette() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Settings);
        updates.finish_ok(found(Some(installed())), installed(), 900);
        assert_eq!(updates.report, None);
        assert_eq!(updates.status(), Status::UpToDate { at: 900 });
    }

    #[test]
    fn an_update_stays_known_while_a_check_runs_or_after_it_failed() {
        let notes = Notes::from_releases(vec![note("0.1.1"), note("0.1.2")]);
        let known = |status| known_update(status, &notes, installed());
        assert_eq!(
            known(Status::Available(version("0.1.3"))),
            Some(version("0.1.3"))
        );
        assert_eq!(known(Status::Checking), Some(version("0.1.2")));
        assert_eq!(known(Status::Failed), Some(version("0.1.2")));
        assert_eq!(known(Status::Unchecked), None);
        assert_eq!(known(Status::UpToDate { at: 5 }), None);
        let none = Notes::default();
        assert_eq!(known_update(Status::Failed, &none, installed()), None);
    }

    #[test]
    fn each_snapshot_for_saving_is_numbered_after_the_last() {
        let mut updates = loaded(Record::default());
        let first = updates.snapshot_for_save();
        updates.mark_announced(version("0.1.1"));
        let second = updates.snapshot_for_save();
        assert!(second.order > first.order);
        assert_eq!(second.record.announced, Some(version("0.1.1")));
    }

    fn note(text: &str) -> Note {
        Note {
            version: version(text),
            published: None,
            body: Some("text".to_owned()),
        }
    }

    fn found_with_notes(newest: &str, notes: Vec<Note>) -> Releases {
        Releases {
            newest: Some(version(newest)),
            notes: Notes::from_releases(notes),
        }
    }

    #[test]
    fn the_notes_of_a_check_are_owed_until_a_write_of_them_lands() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Startup);
        updates.finish_ok(
            found_with_notes("0.1.1", vec![note("0.1.1")]),
            installed(),
            900,
        );
        assert_eq!(updates.notes().newer_than(installed()).count(), 1);
        let after_check = updates.snapshot_for_save();
        let written = after_check.notes.unwrap();
        updates.mark_announced(version("0.1.1"));
        let retried = updates.snapshot_for_save();
        assert!(retried.notes.is_some());
        updates.notes_saved(&written);
        assert!(updates.snapshot_for_save().notes.is_none());
    }

    #[test]
    fn a_write_of_older_notes_does_not_clear_what_a_newer_check_owes() {
        let mut updates = loaded(Record::default());
        updates.finish_ok(
            found_with_notes("0.1.1", vec![note("0.1.1")]),
            installed(),
            900,
        );
        let older = updates.snapshot_for_save().notes.unwrap();
        updates.finish_ok(
            found_with_notes("0.1.2", vec![note("0.1.2")]),
            installed(),
            901,
        );
        updates.notes_saved(&older);
        assert!(updates.snapshot_for_save().notes.is_some());
    }

    #[test]
    fn a_cache_read_late_does_not_replace_the_notes_of_a_check() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Palette);
        updates.finish_ok(
            found_with_notes("0.1.2", vec![note("0.1.2")]),
            installed(),
            900,
        );
        updates.load_notes(Notes::from_releases(vec![note("0.1.1")]));
        let listed: Vec<String> = updates
            .notes()
            .newer_than(installed())
            .map(|note| note.version.to_string())
            .collect();
        assert_eq!(listed, ["0.1.2"]);
    }

    #[test]
    fn the_cache_is_what_shows_before_any_check() {
        let mut updates = loaded(Record::default());
        updates.load_notes(Notes::from_releases(vec![note("0.1.1")]));
        assert_eq!(updates.notes().newer_than(installed()).count(), 1);
        assert!(updates.snapshot_for_save().notes.is_none());
    }

    #[test]
    fn an_answer_is_kept_for_the_next_session() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Startup);
        updates.finish_ok(found(Some(version("0.1.1"))), installed(), 900);
        updates.mark_announced(version("0.1.1"));
        let restarted = loaded(updates.record.clone());
        assert_eq!(restarted.status(), Status::Available(version("0.1.1")));
        assert_eq!(restarted.record.announced, Some(version("0.1.1")));
    }
}
