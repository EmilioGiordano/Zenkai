use std::time::{Duration, Instant};

pub const SHOW_AFTER: Duration = Duration::from_millis(300);

// Declared from least to most important: when jobs overlap the largest one is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LongJob {
    Autosaving,
    Calculating,
    Saving,
}

const JOBS: [LongJob; 3] = [LongJob::Autosaving, LongJob::Calculating, LongJob::Saving];

#[derive(Default)]
struct Running {
    count: u32,
    since: Option<Instant>,
}

#[derive(Default)]
pub struct JobPill {
    running: [Running; 3],
}

impl JobPill {
    pub fn start(&mut self, job: LongJob, now: Instant) {
        let running = &mut self.running[job as usize];
        running.count += 1;
        running.since.get_or_insert(now);
    }

    pub fn end(&mut self, job: LongJob) {
        let running = &mut self.running[job as usize];
        running.count = running.count.saturating_sub(1);
        if running.count == 0 {
            running.since = None;
        }
    }

    pub fn shown(&self, now: Instant) -> Option<LongJob> {
        JOBS.into_iter().rev().find(|job| {
            self.running[*job as usize]
                .since
                .is_some_and(|since| now.duration_since(since) >= SHOW_AFTER)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn after(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    #[test]
    fn a_short_job_never_shows_the_pill() {
        let start = Instant::now();
        let mut pill = JobPill::default();
        pill.start(LongJob::Calculating, start);
        assert_eq!(pill.shown(after(start, 299)), None);
        pill.end(LongJob::Calculating);
        assert_eq!(pill.shown(after(start, 400)), None);
    }

    #[test]
    fn a_job_shows_after_the_delay_and_hides_when_it_ends() {
        let start = Instant::now();
        let mut pill = JobPill::default();
        pill.start(LongJob::Autosaving, start);
        assert_eq!(pill.shown(after(start, 300)), Some(LongJob::Autosaving));
        pill.end(LongJob::Autosaving);
        assert_eq!(pill.shown(after(start, 301)), None);
    }

    #[test]
    fn overlapping_jobs_show_saving_then_calculating_then_autosaving() {
        let start = Instant::now();
        let mut pill = JobPill::default();
        pill.start(LongJob::Autosaving, start);
        pill.start(LongJob::Calculating, start);
        pill.start(LongJob::Saving, start);
        let later = after(start, 500);
        assert_eq!(pill.shown(later), Some(LongJob::Saving));
        pill.end(LongJob::Saving);
        assert_eq!(pill.shown(later), Some(LongJob::Calculating));
        pill.end(LongJob::Calculating);
        assert_eq!(pill.shown(later), Some(LongJob::Autosaving));
    }

    #[test]
    fn a_job_that_is_not_yet_due_does_not_hide_a_due_one() {
        let start = Instant::now();
        let mut pill = JobPill::default();
        pill.start(LongJob::Autosaving, start);
        pill.start(LongJob::Saving, after(start, 200));
        assert_eq!(pill.shown(after(start, 350)), Some(LongJob::Autosaving));
        assert_eq!(pill.shown(after(start, 500)), Some(LongJob::Saving));
    }

    #[test]
    fn overlapping_runs_of_one_job_end_together_and_keep_the_first_start() {
        let start = Instant::now();
        let mut pill = JobPill::default();
        pill.start(LongJob::Autosaving, start);
        pill.start(LongJob::Autosaving, after(start, 250));
        assert_eq!(pill.shown(after(start, 300)), Some(LongJob::Autosaving));
        pill.end(LongJob::Autosaving);
        assert_eq!(pill.shown(after(start, 300)), Some(LongJob::Autosaving));
        pill.end(LongJob::Autosaving);
        assert_eq!(pill.shown(after(start, 300)), None);
    }
}
