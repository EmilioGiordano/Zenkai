// Work over every row and parsed formula of a sheet that runs under the workbook's read
// lock, which edits wait on, is split over several threads.

use std::collections::HashMap;
use std::num::NonZeroUsize;

use ironcalc::base::expressions::parser::Node;
use ironcalc::base::expressions::parser::static_analysis::StaticResult;
use ironcalc::base::types::{Cell, SheetData};

use crate::error::EngineError;
use crate::preflight::ENGINE_STACK_BYTES;

const MAX_THREADS: usize = 8;
// A share of this many rows is about 2 ms of hashing or copying, against tens of microseconds
// to spawn the thread that takes it.
pub(crate) const MIN_ROWS_PER_THREAD: usize = 4 * 1024;

pub(crate) type Rows<'s> = [(&'s i32, &'s HashMap<i32, Cell>)];

fn threads() -> usize {
    std::thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .min(MAX_THREADS)
}

// Runs `share` over consecutive shares of the rows, in no defined order, and returns their
// results in share order. The calling thread, which may be the engine's deep stack, runs
// `meanwhile` and the first share; the other threads only run `share`.
pub(crate) fn over_rows<T: Send, U>(
    sheet_data: &SheetData,
    share: impl Fn(&Rows<'_>) -> T + Sync,
    meanwhile: impl FnOnce() -> Result<U, EngineError>,
) -> Result<(Vec<T>, U), EngineError> {
    let rows: Vec<_> = sheet_data.iter().collect();
    let per_thread = rows.len().div_ceil(threads()).max(MIN_ROWS_PER_THREAD);
    let share = &share;
    std::thread::scope(|scope| {
        let workers: Vec<_> = rows
            .chunks(per_thread)
            .skip(1)
            .map(|rows| scope.spawn(move || share(rows)))
            .collect();
        let first = rows.chunks(per_thread).next().map(share);
        let also = meanwhile()?;
        let mut results = Vec::with_capacity(workers.len() + 1);
        results.extend(first);
        for worker in workers {
            let result = worker.join().map_err(|_| {
                EngineError::Rejected("work over a sheet's rows stopped".to_string())
            })?;
            results.push(result);
        }
        Ok((results, also))
    })
}

// A formula costs about a microsecond to hash or copy, a share about 4 ms.
pub(crate) const MIN_FORMULAS_PER_THREAD: usize = 4 * 1024;

pub(crate) type Formulas = [(Node, StaticResult)];

// Runs `share` with the index of its first formula over consecutive shares of a sheet's
// parsed formulas and returns the results in share order. Formulas are trees walked
// recursively, so the other threads get the engine's stack; the calling thread runs the
// first share.
pub(crate) fn over_formulas<T: Send>(
    formulas: &Formulas,
    share: impl Fn(usize, &Formulas) -> T + Sync,
) -> Result<Vec<T>, EngineError> {
    let per_thread = formulas
        .len()
        .div_ceil(threads())
        .max(MIN_FORMULAS_PER_THREAD);
    let share = &share;
    std::thread::scope(|scope| {
        let workers = formulas
            .chunks(per_thread)
            .enumerate()
            .skip(1)
            .map(|(index, formulas)| {
                std::thread::Builder::new()
                    .stack_size(ENGINE_STACK_BYTES)
                    .spawn_scoped(scope, move || share(index * per_thread, formulas))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| EngineError::Rejected(format!("could not start a thread: {e}")))?;
        let first = formulas
            .chunks(per_thread)
            .next()
            .map(|first| share(0, first));
        let mut results = Vec::with_capacity(workers.len() + 1);
        results.extend(first);
        for worker in workers {
            let result = worker.join().map_err(|_| {
                EngineError::Rejected("work over a sheet's formulas stopped".to_string())
            })?;
            results.push(result);
        }
        Ok(results)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ironcalc::base::Model;
    use ironcalc::base::types::Cell;

    use super::{MIN_FORMULAS_PER_THREAD, MIN_ROWS_PER_THREAD, over_formulas, over_rows};

    #[test]
    fn every_row_is_in_exactly_one_share() {
        let rows = i32::try_from(MIN_ROWS_PER_THREAD * 3 + 5).unwrap();
        let sheet_data: HashMap<i32, HashMap<i32, Cell>> =
            (1..=rows).map(|row| (row, HashMap::new())).collect();
        let (shares, also) = over_rows(
            &sheet_data,
            |rows| rows.iter().map(|(row, _)| i64::from(**row)).sum::<i64>(),
            || Ok("meanwhile"),
        )
        .unwrap();
        assert!(shares.len() > 1);
        assert_eq!(
            shares.iter().sum::<i64>(),
            (1..=i64::from(rows)).sum::<i64>()
        );
        assert_eq!(also, "meanwhile");
        let (none, _) = over_rows(&HashMap::new(), |rows| rows.len(), || Ok(())).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn every_formula_is_in_exactly_one_share_at_its_index() {
        let mut model = Model::new_empty("b", "en", "UTC", "en").unwrap();
        let count = i32::try_from(MIN_FORMULAS_PER_THREAD * 2 + 3).unwrap();
        for row in 1..=count {
            model
                .set_user_input(0, row, 1, format!("=B1+{row}"))
                .unwrap();
        }
        let formulas = &model.parsed_formulas[0];
        assert_eq!(formulas.len(), MIN_FORMULAS_PER_THREAD * 2 + 3);
        let shares = over_formulas(formulas, |first, share| (first, share.to_vec())).unwrap();
        assert!(shares.len() > 1);
        let mut joined = Vec::new();
        for (first, share) in shares {
            assert_eq!(first, joined.len());
            joined.extend(share);
        }
        assert_eq!(&joined, formulas);
        assert!(
            over_formulas(&[], |_, share| share.len())
                .unwrap()
                .is_empty()
        );
    }
}
