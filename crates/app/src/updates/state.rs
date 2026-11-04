use super::record::Record;
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

pub struct Updates {
    pub record: Record,
    pub report: Option<Report>,
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

    pub fn finish_ok(&mut self, newest: Option<Version>, installed: Version, now: u64) {
        self.record.last_check = Some(now);
        self.record.last_success = Some(now);
        self.record.newest = newest;
        match newer_than(newest, installed) {
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

    pub fn mark_announced(&mut self, version: Version) {
        self.record.announced = Some(version);
    }
}

#[cfg(test)]
mod tests {
    use super::{Record, Report, SettingsReading, Source, StartupCheck, Status, Updates, Version};

    fn version(text: &str) -> Version {
        Version::parse_bare(text).unwrap()
    }

    fn installed() -> Version {
        version("0.1.0")
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
        updates.finish_ok(Some(installed()), installed(), 900);
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
        updates.finish_ok(Some(version("0.1.1")), installed(), 900);
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
        updates.finish_ok(Some(version("0.1.1")), installed(), 900);
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
        updates.finish_ok(Some(version("0.1.1")), installed(), 900);
        assert_eq!(updates.report, Some(Report::Available(version("0.1.1"))));
    }

    #[test]
    fn a_bogus_high_tag_that_goes_away_does_not_mute_the_real_update() {
        let mut updates = loaded(Record {
            announced: Some(version("9.9.9")),
            ..Record::default()
        });
        checking(&mut updates, Source::Startup);
        updates.finish_ok(Some(version("0.1.1")), installed(), 900);
        assert_eq!(updates.report, Some(Report::Available(version("0.1.1"))));
    }

    #[test]
    fn up_to_date_is_reported_only_to_the_palette() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Settings);
        updates.finish_ok(Some(installed()), installed(), 900);
        assert_eq!(updates.report, None);
        assert_eq!(updates.status(), Status::UpToDate { at: 900 });
    }

    #[test]
    fn an_answer_is_kept_for_the_next_session() {
        let mut updates = loaded(Record::default());
        checking(&mut updates, Source::Startup);
        updates.finish_ok(Some(version("0.1.1")), installed(), 900);
        updates.mark_announced(version("0.1.1"));
        let restarted = loaded(updates.record.clone());
        assert_eq!(restarted.status(), Status::Available(version("0.1.1")));
        assert_eq!(restarted.record.announced, Some(version("0.1.1")));
    }
}
