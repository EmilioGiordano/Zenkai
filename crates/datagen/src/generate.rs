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
    let mut table: Vec<Vec<String>> = (0..rows)
        .map(|_| vec![String::new(); plans.len()])
        .collect();
    // Emails read the name cells of their row, so every other column is filled first.
    let (emails, others): (Vec<_>, Vec<_>) = plans
        .iter()
        .enumerate()
        .partition(|(_, column)| matches!(column.values, Values::Email(_)));
    for (position, column) in others.into_iter().chain(emails) {
        let slots = blank_slots(spec.seed, position, rows, column.blanks);
        let mut rng = Rng::new(spec.seed, position, Stream::Values);
        match &column.values {
            Values::Set(set) => fill_set(&mut table, position, &slots, set, column, &mut rng),
            Values::Sequence { start, step } => fill(&mut table, position, &slots, |_, row| {
                (i128::from(*start) + i128::from(*step) * row as i128).to_string()
            }),
            Values::Email(email) => {
                let mut addresses = match column.uniqueness {
                    Uniqueness::Repeats => Addresses::MayRepeat,
                    Uniqueness::Unique => Addresses::Unique(TakenAddresses::default()),
                };
                fill(&mut table, position, &slots, |cells, _| {
                    email.address(cells, &mut rng, &mut addresses)
                });
            }
        }
    }
    Ok(table)
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

fn fill_set(
    table: &mut [Vec<String>],
    position: usize,
    slots: &[Slot],
    set: &ValueSet,
    column: &ColumnPlan,
    rng: &mut Rng,
) {
    match column.uniqueness {
        Uniqueness::Repeats => fill(table, position, slots, |_, _| set.sample(rng)),
        Uniqueness::Unique => {
            let mut draws = DistinctDraws::new(set.domain(), slots.len() - column.blanks);
            fill(table, position, slots, |_, _| set.value_at(draws.next(rng)));
        }
    }
}

fn fill(
    table: &mut [Vec<String>],
    position: usize,
    slots: &[Slot],
    mut value_for_row: impl FnMut(&[String], usize) -> String,
) {
    for (row, (cells, slot)) in table.iter_mut().zip(slots).enumerate() {
        if *slot == Slot::Value {
            let value = value_for_row(cells, row);
            cells[position] = value;
        }
    }
}
