use std::time::{Duration, Instant};

use zenkai_types::WorkbookId;

pub const DEFAULT_BUDGET_MB: u64 = 4096;
pub const BUDGET_VARIABLE: &str = "ZENKAI_MEMORY_BUDGET_MB";
const COOLDOWN: Duration = Duration::from_secs(15);

// The budget the process may use before idle workbooks are unloaded.
pub fn budget_mb() -> u64 {
    parse_budget(std::env::var(BUDGET_VARIABLE).ok().as_deref())
}

fn parse_budget(value: Option<&str>) -> u64 {
    value
        .and_then(|text| text.trim().parse::<u64>().ok())
        .filter(|megabytes| *megabytes > 0)
        .unwrap_or(DEFAULT_BUDGET_MB)
}

pub struct Candidate {
    pub id: WorkbookId,
    pub last_used: Instant,
    // Clean, off screen, not busy and with a file to come back from.
    pub idle: bool,
}

// One workbook per look, the longest unused first. Memory takes a while to come back to
// the process, so waiting a cooldown before the next one avoids emptying the sidebar.
pub fn next_to_unload(
    used_mb: u64,
    budget_mb: u64,
    since_last_unload: Option<Duration>,
    candidates: &[Candidate],
) -> Option<WorkbookId> {
    if used_mb <= budget_mb || since_last_unload.is_some_and(|since| since < COOLDOWN) {
        return None;
    }
    candidates
        .iter()
        .filter(|candidate| candidate.idle)
        .min_by_key(|candidate| candidate.last_used)
        .map(|candidate| candidate.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A larger `used_after` is a more recent use.
    fn candidate(id: u64, used_after: u64, idle: bool, base: Instant) -> Candidate {
        Candidate {
            id: WorkbookId(id),
            last_used: base + Duration::from_secs(used_after),
            idle,
        }
    }

    #[test]
    fn nothing_is_unloaded_while_under_the_budget() {
        let now = Instant::now();
        let candidates = [candidate(1, 0, true, now)];
        assert_eq!(next_to_unload(900, 1000, None, &candidates), None);
        assert_eq!(next_to_unload(1000, 1000, None, &candidates), None);
    }

    #[test]
    fn over_the_budget_the_longest_unused_idle_workbook_goes_first() {
        let now = Instant::now();
        let candidates = [
            candidate(1, 60, true, now),
            candidate(2, 10, true, now),
            candidate(3, 0, false, now),
        ];
        assert_eq!(
            next_to_unload(1200, 1000, None, &candidates),
            Some(WorkbookId(2))
        );
    }

    #[test]
    fn workbooks_that_are_not_idle_are_never_chosen() {
        let now = Instant::now();
        let candidates = [candidate(1, 0, false, now), candidate(2, 60, false, now)];
        assert_eq!(next_to_unload(5000, 1000, None, &candidates), None);
        assert_eq!(next_to_unload(5000, 1000, None, &[]), None);
    }

    #[test]
    fn a_second_workbook_waits_for_the_cooldown() {
        let now = Instant::now();
        let candidates = [candidate(1, 0, true, now)];
        let soon = Some(Duration::from_secs(3));
        let later = Some(Duration::from_secs(20));
        assert_eq!(next_to_unload(5000, 1000, soon, &candidates), None);
        assert_eq!(
            next_to_unload(5000, 1000, later, &candidates),
            Some(WorkbookId(1))
        );
    }

    #[test]
    fn the_budget_comes_from_the_environment_or_falls_back() {
        assert_eq!(parse_budget(Some(" 2048 ")), 2048);
        assert_eq!(parse_budget(Some("0")), DEFAULT_BUDGET_MB);
        assert_eq!(parse_budget(Some("many")), DEFAULT_BUDGET_MB);
        assert_eq!(parse_budget(None), DEFAULT_BUDGET_MB);
    }
}
