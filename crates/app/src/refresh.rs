// What a busy workbook kept the UI from reading, and the documents a background task is
// already waiting on to hand it back. Nothing is dropped: each miss is retried once free.
#[derive(Debug, Default)]
pub struct MissedRefreshes {
    cells: bool,
    stats: bool,
    waiting: Vec<u64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Missed {
    pub cells: bool,
    pub stats: bool,
}

impl MissedRefreshes {
    pub fn miss_cells(&mut self) {
        self.cells = true;
    }

    pub fn cells_done(&mut self) {
        self.cells = false;
    }

    pub fn miss_stats(&mut self) {
        self.stats = true;
    }

    pub fn stats_done(&mut self) {
        self.stats = false;
    }

    pub fn take(&mut self) -> Missed {
        Missed {
            cells: std::mem::take(&mut self.cells),
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
        missed.miss_cells();
        missed.miss_stats();
        assert_eq!(
            missed.take(),
            Missed {
                cells: true,
                stats: true
            }
        );
        assert_eq!(missed.take(), Missed::default());
    }

    #[test]
    fn a_refresh_that_ran_clears_its_miss() {
        let mut missed = MissedRefreshes::default();
        missed.miss_cells();
        missed.miss_stats();
        missed.cells_done();
        missed.stats_done();
        assert_eq!(missed.take(), Missed::default());
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
