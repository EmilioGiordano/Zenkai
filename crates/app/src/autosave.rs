use std::time::Duration;

// A recovery write serializes the whole workbook under its read lock, so it starts only once
// the user has paused.
pub const AUTOSAVE_IDLE: Duration = Duration::from_secs(3);

// `since_last_edit` is `None` for work restored from a previous run, which is idle by definition.
pub fn is_due(
    since_last_edit: Option<Duration>,
    since_last_attempt: Duration,
    interval: Duration,
) -> bool {
    since_last_attempt >= interval && since_last_edit.is_none_or(|idle| idle >= AUTOSAVE_IDLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_secs(30);

    #[test]
    fn nothing_is_written_while_edits_are_recent() {
        let recent = Some(AUTOSAVE_IDLE - Duration::from_millis(1));
        assert!(!is_due(recent, INTERVAL * 10, INTERVAL));
    }

    #[test]
    fn a_pause_after_the_interval_allows_the_write() {
        assert!(is_due(Some(AUTOSAVE_IDLE), INTERVAL, INTERVAL));
    }

    #[test]
    fn at_most_one_write_per_interval() {
        let idle = Some(Duration::from_secs(60));
        assert!(!is_due(idle, INTERVAL - Duration::from_millis(1), INTERVAL));
    }

    #[test]
    fn restored_work_counts_as_idle() {
        assert!(is_due(None, INTERVAL, INTERVAL));
    }
}
