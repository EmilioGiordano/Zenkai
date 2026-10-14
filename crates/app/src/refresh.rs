use std::collections::HashMap;

// What a busy workbook kept the UI from reading, per document generation, and the documents
// a background task is already waiting on. Each miss is retried once the workbook is free.
// `Replace` is for a change in how every cell is displayed: the kept cache must go too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellsRefresh {
    Merge,
    Replace,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Missed {
    pub cells: Option<CellsRefresh>,
    pub stats: bool,
}

#[derive(Debug, Default)]
pub struct MissedRefreshes {
    missed: HashMap<u64, Missed>,
    waiting: Vec<u64>,
}

impl MissedRefreshes {
    pub fn miss_cells(&mut self, generation: u64, refresh: CellsRefresh) {
        let missed = self.missed.entry(generation).or_default();
        if missed.cells != Some(CellsRefresh::Replace) {
            missed.cells = Some(refresh);
        }
    }

    // A pending `Replace` is never lost to a plain merge: the merge run becomes the replace.
    pub fn upgraded(&self, generation: u64, refresh: CellsRefresh) -> CellsRefresh {
        match self.missed.get(&generation).and_then(|missed| missed.cells) {
            Some(CellsRefresh::Replace) => CellsRefresh::Replace,
            _ => refresh,
        }
    }

    pub fn cells_done(&mut self, generation: u64) {
        if let Some(missed) = self.missed.get_mut(&generation) {
            missed.cells = None;
        }
        self.forget_if_empty(generation);
    }

    pub fn miss_stats(&mut self, generation: u64) {
        self.missed.entry(generation).or_default().stats = true;
    }

    pub fn stats_done(&mut self, generation: u64) {
        if let Some(missed) = self.missed.get_mut(&generation) {
            missed.stats = false;
        }
        self.forget_if_empty(generation);
    }

    pub fn take(&mut self, generation: u64) -> Missed {
        self.missed.remove(&generation).unwrap_or_default()
    }

    pub fn start_waiting(&mut self, generation: u64) -> bool {
        if self.waiting.contains(&generation) {
            return false;
        }
        self.waiting.push(generation);
        true
    }

    pub fn stop_waiting(&mut self, generation: u64) {
        self.waiting.retain(|waiting| *waiting != generation);
    }

    fn forget_if_empty(&mut self, generation: u64) {
        if self.missed.get(&generation) == Some(&Missed::default()) {
            self.missed.remove(&generation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missed_refresh_is_taken_once_for_the_retry() {
        let mut missed = MissedRefreshes::default();
        missed.miss_cells(1, CellsRefresh::Merge);
        missed.miss_stats(1);
        assert_eq!(
            missed.take(1),
            Missed {
                cells: Some(CellsRefresh::Merge),
                stats: true
            }
        );
        assert_eq!(missed.take(1), Missed::default());
    }

    #[test]
    fn a_refresh_that_ran_clears_its_miss() {
        let mut missed = MissedRefreshes::default();
        missed.miss_cells(1, CellsRefresh::Replace);
        missed.miss_stats(1);
        missed.cells_done(1);
        missed.stats_done(1);
        assert_eq!(missed.take(1), Missed::default());
    }

    #[test]
    fn a_missed_replace_is_not_downgraded_by_a_later_merge() {
        let mut missed = MissedRefreshes::default();
        missed.miss_cells(1, CellsRefresh::Replace);
        missed.miss_cells(1, CellsRefresh::Merge);
        assert_eq!(missed.take(1).cells, Some(CellsRefresh::Replace));
    }

    #[test]
    fn a_merge_run_while_a_replace_is_pending_becomes_the_replace() {
        let mut missed = MissedRefreshes::default();
        missed.miss_cells(1, CellsRefresh::Replace);
        let run = missed.upgraded(1, CellsRefresh::Merge);
        assert_eq!(run, CellsRefresh::Replace);
        missed.cells_done(1);
        assert_eq!(missed.upgraded(1, CellsRefresh::Merge), CellsRefresh::Merge);
        assert_eq!(missed.take(1), Missed::default());
    }

    #[test]
    fn misses_belong_to_their_document() {
        let mut missed = MissedRefreshes::default();
        missed.miss_cells(1, CellsRefresh::Replace);
        missed.miss_stats(1);
        assert_eq!(missed.take(2), Missed::default());
        missed.cells_done(2);
        assert_eq!(missed.upgraded(1, CellsRefresh::Merge), CellsRefresh::Replace);
        assert!(missed.take(1).stats);
    }

    #[test]
    fn one_task_waits_per_document_until_it_ends() {
        let mut missed = MissedRefreshes::default();
        assert!(missed.start_waiting(1));
        assert!(!missed.start_waiting(1));
        assert!(missed.start_waiting(2));
        missed.stop_waiting(1);
        assert!(missed.start_waiting(1));
    }
}
