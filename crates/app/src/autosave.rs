use std::time::Duration;

// A recovery write serializes the whole workbook under its read lock, so it starts only once
// the user has paused.
pub const AUTOSAVE_IDLE: Duration = Duration::from_secs(3);

const MIN_STALE_CAP: Duration = Duration::from_secs(60);

// Pauses shorter than the idle time must not postpone the recovery copy forever.
fn stale_cap(interval: Duration) -> Duration {
    (interval * 3).max(MIN_STALE_CAP)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    No,
    AfterPause,
    // Over the staleness cap: written during edits and never cancelled by them, so it completes.
    Overdue,
}

// `since_last_edit` is `None` for work restored from a previous run, which is idle by definition.
// `stale_for` is how long the recovery copy has missed edits, `None` when it holds them all.
pub fn due(
    since_last_edit: Option<Duration>,
    since_last_attempt: Duration,
    stale_for: Option<Duration>,
    interval: Duration,
) -> Due {
    if since_last_attempt < interval {
        return Due::No;
    }
    if stale_for.is_some_and(|stale| stale >= stale_cap(interval)) {
        return Due::Overdue;
    }
    if since_last_edit.is_none_or(|idle| idle >= AUTOSAVE_IDLE) {
        Due::AfterPause
    } else {
        Due::No
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_secs(30);

    const RECENT: Option<Duration> = Some(Duration::from_millis(200));

    #[test]
    fn nothing_is_written_while_edits_are_recent() {
        let stale = Some(INTERVAL);
        assert_eq!(due(RECENT, INTERVAL * 10, stale, INTERVAL), Due::No);
    }

    #[test]
    fn a_pause_after_the_interval_allows_the_write() {
        let paused = Some(AUTOSAVE_IDLE);
        assert_eq!(due(paused, INTERVAL, None, INTERVAL), Due::AfterPause);
    }

    #[test]
    fn at_most_one_write_per_interval() {
        let paused = Some(Duration::from_secs(60));
        let early = INTERVAL - Duration::from_millis(1);
        assert_eq!(due(paused, early, None, INTERVAL), Due::No);
        assert_eq!(due(RECENT, early, Some(INTERVAL * 9), INTERVAL), Due::No);
    }

    #[test]
    fn restored_work_counts_as_idle() {
        assert_eq!(due(None, INTERVAL, None, INTERVAL), Due::AfterPause);
    }

    #[test]
    fn continuous_edits_still_get_a_copy_within_the_cap() {
        let cap = stale_cap(INTERVAL);
        assert_eq!(due(RECENT, INTERVAL, Some(cap), INTERVAL), Due::Overdue);
        let below = cap - Duration::from_millis(1);
        assert_eq!(due(RECENT, INTERVAL, Some(below), INTERVAL), Due::No);
    }

    #[test]
    fn the_cap_is_three_intervals_and_at_least_a_minute() {
        assert_eq!(stale_cap(Duration::from_secs(10)), MIN_STALE_CAP);
        assert_eq!(
            stale_cap(Duration::from_secs(120)),
            Duration::from_secs(360)
        );
    }
}
