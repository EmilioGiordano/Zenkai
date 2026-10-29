use std::collections::HashMap;
use std::fmt::Write;

use ironcalc::base::expressions::lexer::{Lexer, LexerMode};
use ironcalc::base::expressions::parser::{DefinedNameS, Parser, new_parser_english};
use ironcalc::base::expressions::token::TokenType;
use ironcalc::base::language::get_default_language;
use ironcalc::base::locale::get_default_locale;

use crate::formulas::{keeps_cell_text, parse_node};
use crate::sheet_data::FormulaJob;

// The names a formula of one sheet can refer to, known from the package before any row
// is read.
#[derive(Clone, Copy)]
pub(crate) struct SheetNames<'n> {
    pub(crate) sheet: &'n str,
    pub(crate) sheets: &'n [String],
    pub(crate) defined_names: &'n [DefinedNameS],
}

// How the cells of one shape share its job. Cells with the same shape parse to the same
// R1C1 text unless the parse fails, which keeps each cell's own formula text instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sharing {
    // One cell so far, so whether its shape parses has not been asked.
    Single,
    // The shape parses: every later cell with it uses this job.
    Shared,
    // The shape does not parse, or has no key: each cell keeps a job of its own.
    PerCell,
}

// Every relative reference of a formula written as its offset from the cell, so a formula
// filled down a column has one shape and is kept and converted once.
pub(crate) struct ShapeReader<'n> {
    names: SheetNames<'n>,
    lexer: Lexer<'static>,
    parser: Option<Parser<'static>>,
    key: String,
}

impl<'n> ShapeReader<'n> {
    pub(crate) fn new(names: SheetNames<'n>) -> ShapeReader<'n> {
        ShapeReader {
            names,
            lexer: Lexer::new(
                "",
                LexerMode::A1,
                get_default_locale(),
                get_default_language(),
            ),
            parser: None,
            key: String::new(),
        }
    }

    // The shape of the formula, None when it has none: structured references read the
    // cell's row, and text IronCalc's lexer rejects is left to the parser cell by cell.
    pub(crate) fn key(&mut self, text: &str, row: i32, column: i32, array: bool) -> Option<&str> {
        match write_key(&mut self.lexer, &mut self.key, text, row, column, array) {
            Ok(true) => Some(&self.key),
            Ok(false) | Err(_) => None,
        }
    }

    // The parse the conversion will run, with the sheet and defined names it will have;
    // tables only tell a table name from another name, never a parse error.
    pub(crate) fn sharing_of(&mut self, job: &FormulaJob) -> Sharing {
        let names = self.names;
        let parser = self.parser.get_or_insert_with(|| {
            new_parser_english(
                names.sheets.to_vec(),
                names.defined_names.to_vec(),
                HashMap::new(),
            )
        });
        match keeps_cell_text(&parse_node(parser, names.sheet, job)) {
            true => Sharing::PerCell,
            false => Sharing::Shared,
        }
    }
}

// The formula's tokens from IronCalc's own lexer, with every relative reference written
// as its offset from the cell: two formulas with the same key parse to the same R1C1
// text. False when the formula has no key.
fn write_key(
    lexer: &mut Lexer<'_>,
    key: &mut String,
    text: &str,
    row: i32,
    column: i32,
    array: bool,
) -> Result<bool, std::fmt::Error> {
    key.clear();
    write!(key, "{array}|")?;
    lexer.set_formula(text);
    for _ in 0..=text.len() + 1 {
        let token = lexer.next_token();
        match token {
            TokenType::EOF => return Ok(true),
            TokenType::Illegal(_) | TokenType::StructuredReference { .. } => return Ok(false),
            TokenType::Reference {
                sheet,
                row: reference_row,
                column: reference_column,
                absolute_column,
                absolute_row,
            } => {
                let reference_row = if absolute_row {
                    reference_row
                } else {
                    reference_row - row
                };
                let reference_column = if absolute_column {
                    reference_column
                } else {
                    reference_column - column
                };
                write!(
                    key,
                    "ref({sheet:?},{absolute_row},{reference_row},{absolute_column},{reference_column})\u{1}"
                )?;
            }
            TokenType::Range { sheet, left, right } => {
                let (mut row1, mut row2) = (left.row, right.row);
                let (mut column1, mut column2) = (left.column, right.column);
                let (mut absolute_row1, mut absolute_row2) =
                    (left.absolute_row, right.absolute_row);
                let (mut absolute_column1, mut absolute_column2) =
                    (left.absolute_column, right.absolute_column);
                if row1 > row2 {
                    (row1, row2) = (row2, row1);
                    (absolute_row1, absolute_row2) = (absolute_row2, absolute_row1);
                }
                if column1 > column2 {
                    (column1, column2) = (column2, column1);
                    (absolute_column1, absolute_column2) = (absolute_column2, absolute_column1);
                }
                let offset = |value: i32, absolute: bool, origin: i32| {
                    if absolute { value } else { value - origin }
                };
                write!(
                    key,
                    "range({sheet:?},{absolute_row1},{},{absolute_column1},{},{absolute_row2},{},{absolute_column2},{})\u{1}",
                    offset(row1, absolute_row1, row),
                    offset(column1, absolute_column1, column),
                    offset(row2, absolute_row2, row),
                    offset(column2, absolute_column2, column),
                )?;
            }
            other => {
                write!(key, "{other:?}\u{1}")?;
            }
        }
    }
    Ok(false)
}

// Where the jobs of a piece went once taken into the sheet's, and the bytes of the job
// texts and shape keys the sheet kept from it.
pub(crate) struct Taken {
    pub(crate) moved: Vec<usize>,
    pub(crate) bytes: u64,
}

// Takes the jobs of a piece scanned on its own into the sheet's, joining each shape the
// sheet already has, so the sheet ends up as one scan of all its rows would leave it.
// Whether a shape parses is the same for every cell with it, so a side that asked
// answers for both.
pub(crate) fn merge_jobs(
    jobs: &mut Vec<FormulaJob>,
    shapes: &mut HashMap<Box<str>, usize>,
    mut piece_jobs: Vec<FormulaJob>,
    piece_shapes: HashMap<Box<str>, usize>,
    reader: &mut ShapeReader<'_>,
) -> Taken {
    let mut moved: Vec<Option<usize>> = vec![None; piece_jobs.len()];
    let mut fresh = Vec::new();
    for (key, piece_job) in piece_shapes {
        let Some(&found) = shapes.get(&key) else {
            fresh.push((key, piece_job));
            continue;
        };
        let piece_sharing = piece_jobs[piece_job].sharing;
        if jobs[found].sharing == Sharing::Single {
            jobs[found].sharing = match piece_sharing {
                Sharing::Single => reader.sharing_of(&jobs[found]),
                known => known,
            };
        }
        match (jobs[found].sharing, piece_sharing) {
            (Sharing::Shared, Sharing::Single | Sharing::Shared) => {
                moved[piece_job] = Some(found);
            }
            (Sharing::PerCell, Sharing::Single) => {
                piece_jobs[piece_job].sharing = Sharing::PerCell;
            }
            // The sides disagree only if the parse does; converting the shared job then
            // finds it and leaves the file to IronCalc.
            (Sharing::Shared, Sharing::PerCell)
            | (Sharing::PerCell, Sharing::Shared | Sharing::PerCell)
            | (Sharing::Single, _) => {}
        }
    }
    let mut bytes = 0;
    for (piece_job, job) in piece_jobs.into_iter().enumerate() {
        if moved[piece_job].is_none() {
            moved[piece_job] = Some(jobs.len());
            bytes += job.text.len() as u64;
            jobs.push(job);
        }
    }
    let moved: Vec<usize> = moved.into_iter().flatten().collect();
    for (key, piece_job) in fresh {
        bytes += key.len() as u64;
        shapes.insert(key, moved[piece_job]);
    }
    Taken { moved, bytes }
}

#[cfg(test)]
pub(crate) fn sheet1() -> SheetNames<'static> {
    static SHEETS: std::sync::LazyLock<Vec<String>> =
        std::sync::LazyLock::new(|| vec!["Sheet1".to_string()]);
    SheetNames {
        sheet: "Sheet1",
        sheets: &SHEETS,
        defined_names: &[],
    }
}

#[cfg(test)]
mod tests;
