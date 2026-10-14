// What a busy workbook kept the UI from reading, and the documents a background task is
// already waiting on to hand it back. Nothing is dropped: each miss is retried once free.
// `Replace` is for a change in how every cell is displayed: the kept cache must go too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellsRefresh {
    Merge,
    Replace,
}

#[derive(Debug, Default)]
pub struct MissedRefreshes {
    cells: Option<CellsRefresh>,
    stats: bool,
    waiting: Vec<u64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Missed {
    pub cells: Option<CellsRefresh>,
    pub stats: bool,
}

impl MissedRefreshes {
    pub fn miss_cells(&mut self, refresh: CellsRefresh) {
        if self.cells != Some(CellsRefresh::Replace) {
            self.cells = Some(refresh);
        }
    }

    pub fn cells_done(&mut self) {
        self.cells = None;
    }

    pub fn miss_stats(&mut self) {
        self.stats = true;
    }

    pub fn stats_done(&mut self) {
        self.stats = false;
    }

    pub fn take(&mut self) -> Missed {
        Missed {
            cells: self.cells.take(),
            stats: std::mem::take(&mut self.stats),
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missed_refresh_is_taken_once_for_the_retry() {
        let mut missed = MissedRefreshes::default();
        missed.miss_cells(CellsRefresh::Merge);
        missed.miss_stats();
        assert_eq!(
            missed.take(),
            Missed {
                cells: Some(CellsRefresh::Merge),
                stats: true
            }
        );
        assert_eq!(missed.take(), Missed::default());
    }

    #[test]
    fn a_refresh_that_ran_clears_its_miss() {
        let mut missed = MissedRefreshes::default();
        missed.miss_cells(CellsRefresh::Replace);
        missed.miss_stats();
        missed.cells_done();
        missed.stats_done();
        assert_eq!(missed.take(), Missed::default());
    }

    #[test]
    fn a_missed_replace_is_not_downgraded_by_a_later_merge() {
        let mut missed = MissedRefreshes::default();
        missed.miss_cells(CellsRefresh::Replace);
        missed.miss_cells(CellsRefresh::Merge);
        assert_eq!(missed.take().cells, Some(CellsRefresh::Replace));
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
