use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::{Builder, Scope, ScopedJoinHandle};

use crate::ReadError;
use crate::sheet_data::{SheetCells, scan_rows_into, unreadable};
use crate::sheet_stream::rowtags_from_unit;
use crate::stream::Budgets;

// What one unit of rows gave when scanned on its own; the sheet takes it in order.
pub(crate) struct UnitScan {
    pub(crate) cells: SheetCells,
    pub(crate) rowtags: Vec<u8>,
    pub(crate) rowtags_bytes: u64,
    // Whether an array formula of the unit spills past its last row.
    pub(crate) outcome: Result<bool, ReadError>,
}

pub(crate) fn scan_unit(
    cut: &[u8],
    unit: &[u8],
    prefixes: &[Vec<u8>],
    sheet_name: &str,
) -> UnitScan {
    let mut rowtags = Vec::new();
    let rowtags_bytes = rowtags_from_unit(&mut rowtags, cut, unit);
    let mut cells = SheetCells::default();
    let outcome = std::str::from_utf8(unit)
        .map_err(unreadable)
        .and_then(|text| scan_rows_into(&mut cells, text, prefixes, sheet_name));
    UnitScan {
        cells,
        rowtags,
        rowtags_bytes,
        outcome,
    }
}

pub(crate) struct PoolEnv<'e> {
    pub(crate) cut: &'e [u8],
    pub(crate) prefixes: &'e [Vec<u8>],
    pub(crate) sheet_name: &'e str,
    pub(crate) budgets: &'e Budgets,
}

struct Worker<'scope> {
    units: SyncSender<Vec<u8>>,
    scans: Receiver<UnitScan>,
    handle: ScopedJoinHandle<'scope, ()>,
}

// Threads that scan units of a worksheet while the caller goes on inflating the next. The
// unit k goes to worker k % n, and the scans are taken back in the same order, so they
// arrive in document order with no queue to reorder. One unit more than there are workers
// may be in flight, so that a single worker still overlaps with the caller; each is a
// copy of a piece the reader held, so the memory the pool adds is IN_FLIGHT_EXTRA + n
// pieces of at most the window size.
pub(crate) const IN_FLIGHT_EXTRA: usize = 1;

pub(crate) struct UnitPool<'scope> {
    workers: Vec<Worker<'scope>>,
    dispatched: usize,
    merged: usize,
    #[cfg(test)]
    peak_in_flight: usize,
}

fn stopped() -> ReadError {
    unreadable("a worksheet worker stopped")
}

impl<'scope> UnitPool<'scope> {
    // None for no workers, or when no thread could start.
    pub(crate) fn start<'env>(
        scope: &'scope Scope<'scope, 'env>,
        count: usize,
        env: &'env PoolEnv<'env>,
    ) -> Option<UnitPool<'scope>> {
        let parent = tracing::Span::current();
        let mut workers = Vec::new();
        for number in 0..count {
            let (units, unit_receiver) = sync_channel::<Vec<u8>>(1);
            let (scan_sender, scans) = sync_channel::<UnitScan>(1);
            let parent = parent.clone();
            let spawned = Builder::new()
                .name(format!("xlsx-rows-{number}"))
                .spawn_scoped(scope, move || {
                    parent.in_scope(|| work(env, &unit_receiver, &scan_sender));
                });
            match spawned {
                Ok(handle) => workers.push(Worker {
                    units,
                    scans,
                    handle,
                }),
                Err(error) => {
                    tracing::warn!(%error, "a row worker did not start");
                    break;
                }
            }
        }
        (!workers.is_empty()).then_some(UnitPool {
            workers,
            dispatched: 0,
            merged: 0,
            #[cfg(test)]
            peak_in_flight: 0,
        })
    }

    // Hands the unit over; when every worker is busy, first takes the oldest scan in
    // through `merge`. An error from `merge` is the sheet's first failure so far.
    pub(crate) fn submit(
        &mut self,
        unit: &[u8],
        merge: &mut impl FnMut(UnitScan) -> Result<(), ReadError>,
    ) -> Result<(), ReadError> {
        if self.dispatched - self.merged >= self.workers.len() + IN_FLIGHT_EXTRA {
            self.merge_oldest(merge)?;
        }
        let worker = &self.workers[self.dispatched % self.workers.len()];
        worker.units.send(unit.to_vec()).map_err(|_| stopped())?;
        self.dispatched += 1;
        #[cfg(test)]
        {
            self.peak_in_flight = self.peak_in_flight.max(self.dispatched - self.merged);
        }
        Ok(())
    }

    pub(crate) fn drain(
        &mut self,
        merge: &mut impl FnMut(UnitScan) -> Result<(), ReadError>,
    ) -> Result<(), ReadError> {
        while self.merged < self.dispatched {
            self.merge_oldest(merge)?;
        }
        Ok(())
    }

    fn merge_oldest(
        &mut self,
        merge: &mut impl FnMut(UnitScan) -> Result<(), ReadError>,
    ) -> Result<(), ReadError> {
        let worker = &self.workers[self.merged % self.workers.len()];
        let scan = worker.scans.recv().map_err(|_| stopped())?;
        self.merged += 1;
        merge(scan)
    }

    #[cfg(test)]
    pub(crate) fn peak_in_flight(&self) -> usize {
        self.peak_in_flight
    }

    // Lets the workers go once the unit they are on is done and waits for them.
    pub(crate) fn close(self) {
        let handles: Vec<_> = self
            .workers
            .into_iter()
            .map(|worker| worker.handle)
            .collect();
        for handle in handles {
            if handle.join().is_err() {
                tracing::error!("a row worker panicked");
            }
        }
    }
}

fn work(env: &PoolEnv<'_>, units: &Receiver<Vec<u8>>, scans: &SyncSender<UnitScan>) {
    while let Ok(unit) = units.recv() {
        if env.budgets.cancelled() {
            return;
        }
        let scan = scan_unit(env.cut, &unit, env.prefixes, env.sheet_name);
        if scans.send(scan).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_in_flight_stay_within_the_workers_and_the_extra() {
        let budgets = Budgets::with_cells(crate::limits::CellBudget::max_cells_for_test(u64::MAX));
        let prefixes = [Vec::new()];
        let cut = b"<row ".to_vec();
        let env = PoolEnv {
            cut: &cut,
            prefixes: &prefixes,
            sheet_name: "Sheet1",
            budgets: &budgets,
        };
        for workers in 1..=4 {
            let mut merged = Vec::new();
            let peak = std::thread::scope(|scope| {
                let mut pool = UnitPool::start(scope, workers, &env).unwrap();
                for row in 1..=40 {
                    let unit = format!(r#"<row r="{row}"><c r="A{row}"><v>1</v></c></row>"#);
                    pool.submit(unit.as_bytes(), &mut |scan| {
                        merged.push(scan.cells.last_row());
                        scan.outcome.map(|_| ())
                    })
                    .unwrap();
                }
                pool.drain(&mut |scan| {
                    merged.push(scan.cells.last_row());
                    scan.outcome.map(|_| ())
                })
                .unwrap();
                let peak = pool.peak_in_flight();
                pool.close();
                peak
            });
            assert!(peak <= workers + IN_FLIGHT_EXTRA, "{peak} for {workers}");
            assert_eq!(merged, (1..=40).collect::<Vec<i32>>());
        }
    }
}
