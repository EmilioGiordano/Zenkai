use crate::distinct::DistinctDraws;
use crate::email::{Addresses, TakenAddresses};
use crate::error::DatagenError;
use crate::plan::{ColumnPlan, Uniqueness, Values, plan};
use crate::rng::{Rng, Stream};
use crate::spec::GenerationSpec;
use crate::value_set::ValueSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Value,
    Blank,
}

pub fn validate(spec: &GenerationSpec) -> Result<(), DatagenError> {
    plan(spec).map(|_| ())
}

pub fn generate(spec: &GenerationSpec) -> Result<Vec<Vec<String>>, DatagenError> {
    let plans = plan(spec)?;
    let rows = spec.rows as usize;
    let slots: Vec<Vec<Slot>> = plans
        .iter()
        .enumerate()
        .map(|(position, column)| blank_slots(spec.seed, position, rows, column.blanks))
        .collect();
    // Emails read the name cells of their row, so every other column is filled first.
    let mut columns: Vec<Vec<String>> = plans
        .iter()
        .zip(&slots)
        .enumerate()
        .map(|(position, (column, slots))| {
            let mut rng = Rng::new(spec.seed, position, Stream::Values);
            match &column.values {
                Values::Set(set) => fill_set(set, column, slots, &mut rng),
                Values::Sequence { start, step } => fill(slots, |row| {
                    (i128::from(*start) + i128::from(*step) * row as i128).to_string()
                }),
                Values::Email(_) => Vec::new(),
            }
        })
        .collect();
    for (position, (column, slots)) in plans.iter().zip(&slots).enumerate() {
        if let Values::Email(email) = &column.values {
            let mut rng = Rng::new(spec.seed, position, Stream::Values);
            let mut addresses = match column.uniqueness {
                Uniqueness::Repeats => Addresses::MayRepeat,
                Uniqueness::Unique => Addresses::Unique(TakenAddresses::default()),
            };
            let filled = fill(slots, |row| {
                email.address(&columns, row, &mut rng, &mut addresses)
            });
            columns[position] = filled;
        }
    }
    Ok(into_rows(columns, rows))
}

fn blank_slots(seed: u64, position: usize, rows: usize, blanks: usize) -> Vec<Slot> {
    let mut slots = vec![Slot::Value; rows];
    let mut rng = Rng::new(seed, position, Stream::Blanks);
    let mut draws = DistinctDraws::new(rows as u128, blanks);
    for _ in 0..blanks {
        slots[draws.next(&mut rng) as usize] = Slot::Blank;
    }
    slots
}

fn fill_set(set: &ValueSet, column: &ColumnPlan, slots: &[Slot], rng: &mut Rng) -> Vec<String> {
    match column.uniqueness {
        Uniqueness::Repeats => fill(slots, |_| set.sample(rng)),
        Uniqueness::Unique => {
            let mut draws = DistinctDraws::new(set.domain(), slots.len() - column.blanks);
            fill(slots, |_| set.value_at(draws.next(rng)))
        }
    }
}

fn fill(slots: &[Slot], mut value_for_row: impl FnMut(usize) -> String) -> Vec<String> {
    slots
        .iter()
        .enumerate()
        .map(|(row, slot)| match slot {
            Slot::Value => value_for_row(row),
            Slot::Blank => String::new(),
        })
        .collect()
}

fn into_rows(columns: Vec<Vec<String>>, rows: usize) -> Vec<Vec<String>> {
    let width = columns.len();
    let mut table: Vec<Vec<String>> = (0..rows).map(|_| Vec::with_capacity(width)).collect();
    for column in columns {
        for (row, cell) in table.iter_mut().zip(column) {
            row.push(cell);
        }
    }
    table
}
