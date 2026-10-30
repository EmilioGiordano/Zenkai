use std::time::{Duration, Instant};

use zenkai_types::WorkbookId;

// Used only when the machine's memory cannot be read.
const FALLBACK_BUDGET_MB: u64 = 4096;
const SHARE_OF_RAM_PERCENT: u64 = 60;
const BUDGET_VARIABLE: &str = "ZENKAI_MEMORY_BUDGET_MB";
const COOLDOWN: Duration = Duration::from_secs(15);

// 60 % of the physical memory, unless the variable names a number of MB.
pub fn budget_mb() -> u64 {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let total_mb = system.total_memory() / 1024 / 1024;
    budget_from(
        std::env::var(BUDGET_VARIABLE).ok().as_deref(),
        (total_mb > 0).then_some(total_mb),
    )
}

fn budget_from(override_mb: Option<&str>, total_mb: Option<u64>) -> u64 {
    override_mb
        .and_then(|text| text.trim().parse::<u64>().ok())
        .filter(|megabytes| *megabytes > 0)
        .or_else(|| total_mb.map(|total| total * SHARE_OF_RAM_PERCENT / 100))
        .unwrap_or(FALLBACK_BUDGET_MB)
}

// Committed private bytes of this process, never the working set: Windows pages
// a process out under pressure, so the working set stays low while committed grows.
pub fn current_committed_mb() -> Option<u64> {
    let bytes = memory_stats::memory_stats()?.virtual_mem;
    u64::try_from(bytes / 1024 / 1024).ok()
}

// How a workbook can leave memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resting {
    // Saved, so it is loaded again from its file.
    Unload,
    // With unsaved work, so it is dumped to disk with its undo history.
    Hibernate,
}

pub struct Candidate {
    pub id: WorkbookId,
    pub last_used: Instant,
    // Off screen, not busy and with something to come back from; `None` if it must stay.
    pub resting: Option<Resting>,
}

// A saved workbook costs nothing to put back but a read of its file, so those go first; then
// the longest unused.
fn longest_unused<'a>(
    candidates: impl Iterator<Item = &'a Candidate>,
) -> Option<(WorkbookId, Resting)> {
    candidates
        .filter_map(|candidate| Some((candidate, candidate.resting?)))
        .min_by_key(|(candidate, resting)| (*resting == Resting::Hibernate, candidate.last_used))
        .map(|(candidate, resting)| (candidate.id, resting))
}

// One workbook per look. Memory takes a while to come back to the process, so waiting a
// cooldown before the next one avoids emptying the sidebar.
pub fn next_under_pressure(
    used_mb: u64,
    budget_mb: u64,
    since_last_rest: Option<Duration>,
    candidates: &[Candidate],
) -> Option<(WorkbookId, Resting)> {
    if used_mb <= budget_mb || since_last_rest.is_some_and(|since| since < COOLDOWN) {
        return None;
    }
    longest_unused(candidates.iter())
}

// A workbook that can rest because the user is not using it.
pub struct Sleeper {
    pub id: WorkbookId,
    // When it left the screen. Unused for the workbook on screen.
    pub last_used: Instant,
    pub on_screen: bool,
}

// Workbooks off screen rest `rest_after` after they left it. The one on screen rests only
// while the window is hidden in the tray, `rest_after` after it was hidden. The one that has
// waited longest goes first, one per cooldown, so memory is freed gradually.
pub fn next_idle(
    rest_after: Option<Duration>,
    hidden_since: Option<Instant>,
    now: Instant,
    since_last_rest: Option<Duration>,
    sleepers: &[Sleeper],
) -> Option<WorkbookId> {
    let rest_after = rest_after?;
    if since_last_rest.is_some_and(|since| since < COOLDOWN) {
        return None;
    }
    sleepers
        .iter()
        .filter_map(|sleeper| {
            let since = if sleeper.on_screen {
                hidden_since?
            } else {
                sleeper.last_used
            };
            let waited = now.saturating_duration_since(since);
            (waited >= rest_after).then_some((sleeper.id, waited))
        })
        .max_by_key(|(_, waited)| *waited)
        .map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A larger `used_after` is a more recent use.
    fn candidate(id: u64, used_after: u64, resting: Option<Resting>, base: Instant) -> Candidate {
        Candidate {
            id: WorkbookId(id),
            last_used: base + Duration::from_secs(used_after),
            resting,
        }
    }

    const UNLOAD: Option<Resting> = Some(Resting::Unload);
    const HIBERNATE: Option<Resting> = Some(Resting::Hibernate);

    #[test]
    fn nothing_rests_while_under_the_budget() {
        let now = Instant::now();
        let candidates = [candidate(1, 0, UNLOAD, now)];
        assert_eq!(next_under_pressure(900, 1000, None, &candidates), None);
        assert_eq!(next_under_pressure(1000, 1000, None, &candidates), None);
    }

    #[test]
    fn over_the_budget_the_longest_unused_saved_workbook_goes_first() {
        let now = Instant::now();
        let candidates = [
            candidate(1, 60, UNLOAD, now),
            candidate(2, 10, UNLOAD, now),
            candidate(3, 0, None, now),
        ];
        assert_eq!(
            next_under_pressure(1200, 1000, None, &candidates),
            Some((WorkbookId(2), Resting::Unload))
        );
    }

    #[test]
    fn a_workbook_with_unsaved_work_goes_only_when_no_saved_one_can() {
        let now = Instant::now();
        let candidates = [
            candidate(1, 0, HIBERNATE, now),
            candidate(2, 60, UNLOAD, now),
        ];
        assert_eq!(
            next_under_pressure(1200, 1000, None, &candidates),
            Some((WorkbookId(2), Resting::Unload))
        );
        assert_eq!(
            next_under_pressure(1200, 1000, None, &candidates[..1]),
            Some((WorkbookId(1), Resting::Hibernate))
        );
    }

    #[test]
    fn workbooks_that_must_stay_are_never_chosen() {
        let now = Instant::now();
        let candidates = [candidate(1, 0, None, now), candidate(2, 60, None, now)];
        assert_eq!(next_under_pressure(5000, 1000, None, &candidates), None);
        assert_eq!(next_under_pressure(5000, 1000, None, &[]), None);
    }

    #[test]
    fn a_second_workbook_waits_for_the_cooldown() {
        let now = Instant::now();
        let candidates = [candidate(1, 0, UNLOAD, now)];
        let soon = Some(Duration::from_secs(3));
        let later = Some(Duration::from_secs(20));
        assert_eq!(next_under_pressure(5000, 1000, soon, &candidates), None);
        assert_eq!(
            next_under_pressure(5000, 1000, later, &candidates),
            Some((WorkbookId(1), Resting::Unload))
        );
    }

    const REST_AFTER: Duration = Duration::from_secs(15 * 60);

    fn sleeper(id: u64, left_screen: Instant, on_screen: bool) -> Sleeper {
        Sleeper {
            id: WorkbookId(id),
            last_used: left_screen,
            on_screen,
        }
    }

    fn minutes(count: u64) -> Duration {
        Duration::from_secs(count * 60)
    }

    #[test]
    fn a_workbook_off_screen_rests_once_the_delay_has_passed() {
        let base = Instant::now();
        let sleepers = [sleeper(1, base, false)];
        let before = base + REST_AFTER - Duration::from_secs(1);
        assert_eq!(
            next_idle(Some(REST_AFTER), None, before, None, &sleepers),
            None
        );
        assert_eq!(
            next_idle(Some(REST_AFTER), None, base + REST_AFTER, None, &sleepers),
            Some(WorkbookId(1))
        );
    }

    #[test]
    fn never_means_nothing_rests() {
        let base = Instant::now();
        let sleepers = [sleeper(1, base, false), sleeper(2, base, true)];
        let late = base + minutes(10_000);
        assert_eq!(next_idle(None, Some(base), late, None, &sleepers), None);
    }

    #[test]
    fn the_workbook_on_screen_stays_while_the_window_is_shown() {
        let base = Instant::now();
        let sleepers = [sleeper(1, base, true)];
        let late = base + minutes(10_000);
        assert_eq!(
            next_idle(Some(REST_AFTER), None, late, None, &sleepers),
            None
        );
    }

    #[test]
    fn the_workbook_on_screen_rests_after_the_window_was_hidden_long_enough() {
        let base = Instant::now() + minutes(1_000);
        let hidden = base - minutes(5);
        // On screen since long before the window was hidden: only the hiding counts.
        let sleepers = [sleeper(1, base - minutes(500), true)];
        let early = base + minutes(9);
        let due = base + minutes(10);
        assert_eq!(
            next_idle(Some(REST_AFTER), Some(hidden), early, None, &sleepers),
            None
        );
        assert_eq!(
            next_idle(Some(REST_AFTER), Some(hidden), due, None, &sleepers),
            Some(WorkbookId(1))
        );
    }

    #[test]
    fn a_background_workbook_counts_from_when_it_left_the_screen_not_from_hiding() {
        let base = Instant::now() + minutes(1_000);
        let sleepers = [sleeper(1, base - minutes(20), false)];
        assert_eq!(
            next_idle(Some(REST_AFTER), Some(base), base, None, &sleepers),
            Some(WorkbookId(1))
        );
        let fresh = [sleeper(2, base - minutes(2), false)];
        assert_eq!(
            next_idle(
                Some(REST_AFTER),
                Some(base - minutes(60)),
                base,
                None,
                &fresh
            ),
            None
        );
    }

    #[test]
    fn the_longest_waiting_workbook_goes_first_and_one_per_cooldown() {
        let base = Instant::now() + minutes(1_000);
        let sleepers = [
            sleeper(1, base - minutes(20), false),
            sleeper(2, base - minutes(40), false),
            sleeper(3, base - minutes(30), false),
        ];
        assert_eq!(
            next_idle(Some(REST_AFTER), None, base, None, &sleepers),
            Some(WorkbookId(2))
        );
        let soon = Some(Duration::from_secs(3));
        assert_eq!(
            next_idle(Some(REST_AFTER), None, base, soon, &sleepers),
            None
        );
        let later = Some(COOLDOWN);
        assert_eq!(
            next_idle(Some(REST_AFTER), None, base, later, &sleepers),
            Some(WorkbookId(2))
        );
    }

    #[test]
    fn a_workbook_touched_by_an_agent_does_not_rest_before_the_delay_runs_from_the_touch() {
        let base = Instant::now() + minutes(1_000);
        let touched_at = base - minutes(1);
        let sleepers = [sleeper(1, touched_at, false)];
        let almost = touched_at + REST_AFTER - Duration::from_secs(1);
        assert_eq!(
            next_idle(Some(REST_AFTER), None, almost, None, &sleepers),
            None
        );
        assert_eq!(
            next_idle(
                Some(REST_AFTER),
                None,
                touched_at + REST_AFTER,
                None,
                &sleepers
            ),
            Some(WorkbookId(1))
        );
    }

    #[test]
    fn nobody_to_rest_is_nothing() {
        assert_eq!(
            next_idle(Some(REST_AFTER), None, Instant::now(), None, &[]),
            None
        );
    }

    #[test]
    fn the_budget_is_sixty_percent_of_ram_unless_overridden() {
        assert_eq!(budget_from(None, Some(16_000)), 9_600);
        assert_eq!(budget_from(Some(" 2048 "), Some(16_000)), 2048);
        assert_eq!(budget_from(Some("0"), Some(16_000)), 9_600);
        assert_eq!(budget_from(Some("many"), Some(16_000)), 9_600);
        assert_eq!(budget_from(None, None), FALLBACK_BUDGET_MB);
        assert_eq!(budget_from(Some("512"), None), 512);
    }
}
