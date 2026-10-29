use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ironcalc::base::expressions::lexer::{Lexer, LexerMode};
use ironcalc::base::expressions::parser::{DefinedNameS, Parser, new_parser_english};
use ironcalc::base::expressions::token::{OpCompare, OpProduct, OpSum, TokenType};
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
    key: Vec<u8>,
}

impl<'n> ShapeReader<'n> {
    pub(crate) fn new(names: SheetNames<'n>) -> ShapeReader<'n> {
        ShapeReader {
            names,
            lexer: empty_lexer(),
            parser: None,
            key: Vec::new(),
        }
    }

    // The shape of the formula, None when it has none: structured references read the
    // cell's row, and text IronCalc's lexer rejects is left to the parser cell by cell.
    // A lexer that panics leaves the cell to the conversion, which falls back on its own.
    pub(crate) fn key(&mut self, text: &str, row: i32, column: i32, array: bool) -> Option<&[u8]> {
        let (lexer, key) = (&mut self.lexer, &mut self.key);
        match catch_unwind(AssertUnwindSafe(|| {
            write_key(lexer, key, text, row, column, array)
        })) {
            Ok(true) => Some(&self.key),
            Ok(false) => None,
            Err(_) => {
                self.lexer = empty_lexer();
                None
            }
        }
    }

    // The parse the conversion will run, with the sheet and defined names it will have;
    // tables only tell a table name from another name, never a parse error. A parser that
    // panics leaves each cell to the conversion, which falls back on its own.
    pub(crate) fn sharing_of(&mut self, job: &FormulaJob) -> Sharing {
        let names = self.names;
        let parser = self.parser.get_or_insert_with(|| {
            new_parser_english(
                names.sheets.to_vec(),
                names.defined_names.to_vec(),
                HashMap::new(),
            )
        });
        match catch_unwind(AssertUnwindSafe(|| {
            keeps_cell_text(&parse_node(parser, names.sheet, job))
        })) {
            Ok(false) => Sharing::Shared,
            Ok(true) => Sharing::PerCell,
            Err(_) => {
                self.parser = None;
                Sharing::PerCell
            }
        }
    }
}

fn empty_lexer() -> Lexer<'static> {
    Lexer::new(
        "",
        LexerMode::A1,
        get_default_locale(),
        get_default_language(),
    )
}

// A reference's tag carries whether it names a sheet and which parts are absolute, so a
// plain reference takes three bytes; the other tags are ASCII and never collide.
const REFERENCE: u8 = 0x80;
const RANGE: u8 = 0xc0;

// The formula's tokens from IronCalc's own lexer, with every relative reference written
// as its offset from the cell: two formulas with the same key parse to the same R1C1
// text. A tag byte per token and lengths before texts keep the key about the size of the
// formula, since the keys of a piece waiting to be merged are held uncharged. False when
// the formula has no key.
fn write_key(
    lexer: &mut Lexer<'_>,
    key: &mut Vec<u8>,
    text: &str,
    row: i32,
    column: i32,
    array: bool,
) -> bool {
    key.clear();
    key.push(u8::from(array));
    lexer.set_formula(text);
    for _ in 0..=text.len() + 1 {
        let tag = match lexer.next_token() {
            TokenType::EOF => return true,
            TokenType::Illegal(_) | TokenType::StructuredReference { .. } => return false,
            TokenType::Ident(name) => {
                key.push(b'i');
                write_text(key, &name);
                continue;
            }
            TokenType::String(value) => {
                key.push(b's');
                write_text(key, &value);
                continue;
            }
            TokenType::Number(value) => {
                write_number(key, value);
                continue;
            }
            TokenType::Error(error) => {
                key.push(b'e');
                write_text(key, &error.to_string());
                continue;
            }
            TokenType::Reference {
                sheet,
                row: reference_row,
                column: reference_column,
                absolute_column,
                absolute_row,
            } => {
                key.push(
                    REFERENCE
                        | u8::from(sheet.is_some())
                        | u8::from(absolute_row) << 1
                        | u8::from(absolute_column) << 2,
                );
                write_sheet(key, sheet.as_deref());
                write_offset(key, reference_row, absolute_row, row);
                write_offset(key, reference_column, absolute_column, column);
                continue;
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
                key.push(
                    RANGE
                        | u8::from(sheet.is_some())
                        | u8::from(absolute_row1) << 1
                        | u8::from(absolute_column1) << 2
                        | u8::from(absolute_row2) << 3
                        | u8::from(absolute_column2) << 4,
                );
                write_sheet(key, sheet.as_deref());
                write_offset(key, row1, absolute_row1, row);
                write_offset(key, column1, absolute_column1, column);
                write_offset(key, row2, absolute_row2, row);
                write_offset(key, column2, absolute_column2, column);
                continue;
            }
            TokenType::Boolean(true) => b'T',
            TokenType::Boolean(false) => b'F',
            TokenType::Compare(OpCompare::LessThan) => b'<',
            TokenType::Compare(OpCompare::GreaterThan) => b'>',
            TokenType::Compare(OpCompare::Equal) => b'=',
            TokenType::Compare(OpCompare::LessOrEqualThan) => b'l',
            TokenType::Compare(OpCompare::GreaterOrEqualThan) => b'm',
            TokenType::Compare(OpCompare::NonEqual) => b'!',
            TokenType::Addition(OpSum::Add) => b'+',
            TokenType::Addition(OpSum::Minus) => b'-',
            TokenType::Product(OpProduct::Times) => b'*',
            TokenType::Product(OpProduct::Divide) => b'/',
            TokenType::Power => b'^',
            TokenType::LeftParenthesis => b'(',
            TokenType::RightParenthesis => b')',
            TokenType::Colon => b':',
            TokenType::Semicolon => b';',
            TokenType::LeftBracket => b'[',
            TokenType::RightBracket => b']',
            TokenType::LeftBrace => b'{',
            TokenType::RightBrace => b'}',
            TokenType::Comma => b',',
            TokenType::Bang => b'b',
            TokenType::Percent => b'%',
            TokenType::And => b'&',
            TokenType::At => b'@',
            TokenType::Spill => b'#',
            TokenType::Backslash => b'\\',
        };
        key.push(tag);
    }
    false
}

// Whole numbers, the usual ones in a formula, take a byte or two; the others their
// shortest exact text, a few bytes past what the formula wrote.
fn write_number(key: &mut Vec<u8>, value: f64) {
    if value.is_sign_positive() && value.fract() == 0.0 && value < 9_007_199_254_740_992.0 {
        key.push(b'n');
        write_unsigned(key, value as u64);
    } else {
        key.push(b'f');
        write_text(key, &format!("{value:e}"));
    }
}

fn write_text(key: &mut Vec<u8>, text: &str) {
    write_unsigned(key, text.len() as u64);
    key.extend_from_slice(text.as_bytes());
}

fn write_sheet(key: &mut Vec<u8>, sheet: Option<&str>) {
    if let Some(sheet) = sheet {
        write_text(key, sheet);
    }
}

fn write_offset(key: &mut Vec<u8>, value: i32, absolute: bool, origin: i32) {
    let value = if absolute {
        i64::from(value)
    } else {
        i64::from(value) - i64::from(origin)
    };
    write_unsigned(key, ((value << 1) ^ (value >> 63)) as u64);
}

fn write_unsigned(key: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        key.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    key.push(value as u8);
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
    shapes: &mut HashMap<Box<[u8]>, usize>,
    mut piece_jobs: Vec<FormulaJob>,
    piece_shapes: HashMap<Box<[u8]>, usize>,
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
