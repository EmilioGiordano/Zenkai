use std::time::{Duration, Instant};

use zenkai_types::WorkbookId;

// Used only when the machine's memory cannot be read.
const FALLBACK_BUDGET_MB: u64 = 4096;
const SHARE_OF_RAM_PERCENT: u64 = 60;
const BUDGET_VARIABLE: &str = "ZENKAI_MEMORY_BUDGET_MB";
const COOLDOWN: Duration = Duration::from_secs(15);
const AWAY_AFTER: Duration = Duration::from_secs(30 * 60);

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

// A window hidden in the tray for half an hour is not being worked in. Only workbooks nobody
// has touched since then rest, and never because of a short pause: Excel keeps everything in
// memory, so Zenkai steps in only when the user has clearly gone away.
pub fn next_when_away(
    hidden_for: Option<Duration>,
    now: Instant,
    since_last_rest: Option<Duration>,
    candidates: &[Candidate],
) -> Option<(WorkbookId, Resting)> {
    if hidden_for? < AWAY_AFTER || since_last_rest.is_some_and(|since| since < COOLDOWN) {
        return None;
    }
    longest_unused(
        candidates
            .iter()
            .filter(|candidate| now.saturating_duration_since(candidate.last_used) >= AWAY_AFTER),
    )
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

    #[test]
    fn nothing_rests_while_the_window_is_shown_or_was_hidden_a_short_while() {
        let now = Instant::now() + AWAY_AFTER * 4;
        let candidates = [candidate(1, 0, UNLOAD, now - AWAY_AFTER * 3)];
        assert_eq!(next_when_away(None, now, None, &candidates), None);
        let short = Some(AWAY_AFTER - Duration::from_secs(1));
        assert_eq!(next_when_away(short, now, None, &candidates), None);
        let long = Some(AWAY_AFTER);
        assert_eq!(
            next_when_away(long, now, None, &candidates),
            Some((WorkbookId(1), Resting::Unload))
        );
    }

    #[test]
    fn a_workbook_used_during_the_last_half_hour_stays_when_the_user_is_away() {
        let now = Instant::now() + AWAY_AFTER * 4;
        let candidates = [
            candidate(1, 0, UNLOAD, now - AWAY_AFTER + Duration::from_secs(1)),
            candidate(2, 0, HIBERNATE, now - AWAY_AFTER),
        ];
        let away = Some(AWAY_AFTER * 2);
        assert_eq!(
            next_when_away(away, now, None, &candidates),
            Some((WorkbookId(2), Resting::Hibernate))
        );
        assert_eq!(next_when_away(away, now, None, &candidates[..1]), None);
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
