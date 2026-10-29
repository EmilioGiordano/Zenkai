use std::collections::HashMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ironcalc::base::expressions::lexer::{Lexer, LexerMode};
use ironcalc::base::expressions::parser::static_analysis::add_implicit_intersection;
use ironcalc::base::expressions::parser::stringify::to_rc_format;
use ironcalc::base::expressions::parser::{DefinedNameS, Parser, new_parser_english};
use ironcalc::base::expressions::token::TokenType;
use ironcalc::base::expressions::types::CellReferenceRC;
use ironcalc::base::language::get_default_language;
use ironcalc::base::locale::get_default_locale;
use ironcalc::base::types::Table;

use crate::ReadError;
use crate::sheet_data::FormulaJob;

const CHUNK_FORMULAS: usize = 4_096;
// IronCalc's parser and stringifier recurse on the formula tree; the engine gives them
// this same stack, sized for the longest formula the limits allow.
pub(crate) const WORKER_STACK_BYTES: usize = 256 * 1024 * 1024;

pub(crate) struct Names<'n> {
    pub(crate) sheets: &'n [String],
    pub(crate) defined_names: &'n [DefinedNameS],
    pub(crate) tables: &'n HashMap<String, Table>,
}

// The R1C1 text of every job, as one table of distinct formulas and, per sheet, the index
// of each job's formula in it.
pub(crate) struct Converted {
    pub(crate) distinct: Vec<Arc<str>>,
    pub(crate) per_sheet: Vec<Vec<u32>>,
}

struct Chunk {
    sheet: usize,
    start: usize,
    end: usize,
}

struct WorkerOutput {
    distinct: Vec<Arc<str>>,
    chunks: Vec<(usize, Vec<u32>)>,
}

pub(crate) fn convert(
    sheets: &[(&str, &[FormulaJob])],
    names: &Names<'_>,
    stop: &AtomicBool,
) -> Result<Converted, ReadError> {
    let mut chunks = Vec::new();
    for (sheet, (_, jobs)) in sheets.iter().enumerate() {
        for start in (0..jobs.len()).step_by(CHUNK_FORMULAS) {
            chunks.push(Chunk {
                sheet,
                start,
                end: (start + CHUNK_FORMULAS).min(jobs.len()),
            });
        }
    }
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(chunks.len())
        .max(1);
    let next = AtomicUsize::new(0);
    let outputs = Mutex::new(Vec::new());
    std::thread::scope(|scope| -> Result<(), ReadError> {
        let mut handles = Vec::new();
        for _ in 0..workers {
            let handle = std::thread::Builder::new()
                .name("zenkai-xlsx-formulas".to_string())
                .stack_size(WORKER_STACK_BYTES)
                .spawn_scoped(scope, || {
                    let output = work(sheets, &chunks, &next, names, stop);
                    if let Ok(mut outputs) = outputs.lock() {
                        outputs.push(output);
                    }
                })
                .map_err(|e| ReadError::Unsupported(format!("no formula worker: {e}")))?;
            handles.push(handle);
        }
        for handle in handles {
            handle
                .join()
                .map_err(|_| ReadError::Unsupported("a formula worker stopped".to_string()))?;
        }
        Ok(())
    })?;
    let outputs = outputs
        .into_inner()
        .map_err(|_| ReadError::Unsupported("a formula worker stopped".to_string()))?;

    let mut distinct = Vec::new();
    let mut index_of: HashMap<Arc<str>, u32> = HashMap::new();
    let mut per_sheet: Vec<Vec<u32>> = sheets.iter().map(|(_, jobs)| vec![0; jobs.len()]).collect();
    let mut done = 0;
    for output in outputs {
        let mut global = Vec::with_capacity(output.distinct.len());
        for text in output.distinct {
            let next =
                u32::try_from(distinct.len()).map_err(|e| ReadError::Unsupported(e.to_string()))?;
            let id = *index_of.entry(text.clone()).or_insert_with(|| {
                distinct.push(text);
                next
            });
            global.push(id);
        }
        for (chunk, ids) in output.chunks {
            let Chunk { sheet, start, end } = chunks[chunk];
            for (slot, id) in per_sheet[sheet][start..end].iter_mut().zip(ids) {
                *slot = global[id as usize];
            }
            done += 1;
        }
    }
    if done != chunks.len() {
        return Err(ReadError::Unsupported(
            "a formula chunk was lost".to_string(),
        ));
    }
    Ok(Converted {
        distinct,
        per_sheet,
    })
}

fn work(
    sheets: &[(&str, &[FormulaJob])],
    chunks: &[Chunk],
    next: &AtomicUsize,
    names: &Names<'_>,
    stop: &AtomicBool,
) -> WorkerOutput {
    let mut converter = Converter::new(names);
    let mut chunk_ids = Vec::new();
    loop {
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some(chunk) = chunks.get(index) else {
            break;
        };
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let (sheet_name, jobs) = sheets[chunk.sheet];
        let ids = jobs[chunk.start..chunk.end]
            .iter()
            .map(|job| converter.convert(chunk.sheet, sheet_name, job))
            .collect();
        chunk_ids.push((index, ids));
    }
    WorkerOutput {
        distinct: converter.distinct,
        chunks: chunk_ids,
    }
}

pub(crate) struct Converter<'n> {
    parser: Parser<'n>,
    lexer: Lexer<'n>,
    key: String,
    cache: HashMap<String, u32>,
    fill_key: String,
    fill_cache: HashMap<String, u32>,
    index_of: HashMap<Arc<str>, u32>,
    distinct: Vec<Arc<str>>,
}

impl<'n> Converter<'n> {
    pub(crate) fn new(names: &Names<'_>) -> Converter<'n> {
        Converter {
            parser: new_parser_english(
                names.sheets.to_vec(),
                names.defined_names.to_vec(),
                names.tables.clone(),
            ),
            lexer: Lexer::new(
                "",
                LexerMode::A1,
                get_default_locale(),
                get_default_language(),
            ),
            key: String::new(),
            cache: HashMap::new(),
            fill_key: String::new(),
            fill_cache: HashMap::new(),
            index_of: HashMap::new(),
            distinct: Vec::new(),
        }
    }

    pub(crate) fn convert(&mut self, sheet: usize, sheet_name: &str, job: &FormulaJob) -> u32 {
        let filled = self.fill_key(sheet, job).unwrap_or(false);
        if filled && let Some(id) = self.fill_cache.get(&self.fill_key) {
            return *id;
        }
        let cacheable = self.cache_key(sheet, job).unwrap_or(false);
        if cacheable && let Some(id) = self.cache.get(&self.key) {
            let id = *id;
            if filled {
                self.fill_cache.insert(self.fill_key.clone(), id);
            }
            return id;
        }
        let (text, reusable) = self.parse(sheet_name, job);
        let id = self.intern(text);
        if cacheable && reusable {
            self.cache.insert(self.key.clone(), id);
            if filled {
                self.fill_cache.insert(self.fill_key.clone(), id);
            }
        }
        id
    }

    // A key much cheaper than `cache_key` for a formula filled down a column: its text with
    // the row of every relative cell reference written as its offset from the cell. False
    // for any text whose reading by IronCalc's lexer this scan does not mirror exactly, such
    // as rows alone (`3:5`), structured references or characters past ASCII outside quotes.
    fn fill_key(&mut self, sheet: usize, job: &FormulaJob) -> Result<bool, std::fmt::Error> {
        self.fill_key.clear();
        write!(self.fill_key, "{sheet}|{}|{}|", job.array, job.column)?;
        let text = job.text.as_str();
        let bytes = text.as_bytes();
        // The key marks references and swapped ranges with these.
        if bytes.contains(&1) || bytes.contains(&2) {
            return Ok(false);
        }
        let mut previous = Word::Other;
        let mut after_colon = false;
        let mut at = 0;
        while at < bytes.len() {
            let byte = bytes[at];
            if byte == b'"' || byte == b'\'' {
                let start = at;
                at += 1;
                loop {
                    match bytes.get(at) {
                        None => return Ok(false),
                        Some(&b) if b == byte && bytes.get(at + 1) == Some(&byte) => at += 2,
                        Some(&b) if b == byte => break,
                        Some(_) => at += 1,
                    }
                }
                at += 1;
                self.fill_key.push_str(&text[start..at]);
                previous = Word::Other;
                continue;
            }
            if is_word_byte(byte) {
                let start = at;
                while at < bytes.len() && is_word_byte(bytes[at]) {
                    at += 1;
                }
                let word = &text[start..at];
                let next = bytes.get(at).copied();
                let kind = if matches!(next, Some(b'(' | b'!')) {
                    Word::Other
                } else {
                    word_kind(word)
                };
                match kind {
                    Word::Invalid => return Ok(false),
                    Word::Cell {
                        letters,
                        row,
                        relative_row: true,
                    } => {
                        write!(self.fill_key, "\u{1}{letters}{}\u{1}", row - job.row)?;
                    }
                    _ => self.fill_key.push_str(word),
                }
                if after_colon {
                    match (previous, kind) {
                        // IronCalc puts the lower row first, so which end that is depends
                        // on the cell when only one row is relative.
                        (Word::Cell { row: first, .. }, Word::Cell { row: last, .. }) => {
                            if first > last {
                                self.fill_key.push('\u{2}');
                            }
                        }
                        (Word::Column, Word::Column) => {}
                        _ => return Ok(false),
                    }
                }
                after_colon = false;
                previous = kind;
                continue;
            }
            if !byte.is_ascii() || byte == b'[' || byte == b']' {
                return Ok(false);
            }
            if byte == b':' {
                if at == 0 || !is_word_byte(bytes[at - 1]) {
                    return Ok(false);
                }
                after_colon = true;
            } else if after_colon {
                return Ok(false);
            } else {
                previous = Word::Other;
            }
            self.fill_key.push(char::from(byte));
            at += 1;
        }
        Ok(!after_colon)
    }

    fn intern(&mut self, text: String) -> u32 {
        let next = u32::try_from(self.distinct.len()).unwrap_or(u32::MAX);
        if let Some(id) = self.index_of.get(text.as_str()) {
            return *id;
        }
        let shared: Arc<str> = Arc::from(text);
        self.index_of.insert(shared.clone(), next);
        self.distinct.push(shared);
        next
    }

    // IronCalc's own conversion. The result is reusable for another cell with the same
    // key unless a parse error kept the raw formula text in it.
    pub(crate) fn parse(&mut self, sheet_name: &str, job: &FormulaJob) -> (String, bool) {
        let context = CellReferenceRC {
            sheet: sheet_name.to_string(),
            column: job.column,
            row: job.row,
        };
        let mut node = self.parser.parse(&job.text, &context);
        if !job.array {
            add_implicit_intersection(&mut node, true);
        }
        let reusable = !format!("{node:?}").contains("ParseErrorKind");
        (to_rc_format(&node), reusable)
    }

    // The formula's tokens from IronCalc's own lexer, with every relative reference written
    // as its offset from the cell: two formulas with the same key parse to the same R1C1
    // text. Structured references read the cell's row, so they are never cached.
    fn cache_key(&mut self, sheet: usize, job: &FormulaJob) -> Result<bool, std::fmt::Error> {
        self.key.clear();
        write!(self.key, "{sheet}|{}|", job.array)?;
        self.lexer.set_formula(&job.text);
        for _ in 0..=job.text.len() + 1 {
            let token = self.lexer.next_token();
            match token {
                TokenType::EOF => return Ok(true),
                TokenType::Illegal(_) | TokenType::StructuredReference { .. } => return Ok(false),
                TokenType::Reference {
                    sheet,
                    row,
                    column,
                    absolute_column,
                    absolute_row,
                } => {
                    let row = if absolute_row { row } else { row - job.row };
                    let column = if absolute_column {
                        column
                    } else {
                        column - job.column
                    };
                    write!(
                        self.key,
                        "ref({sheet:?},{absolute_row},{row},{absolute_column},{column})\u{1}"
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
                        self.key,
                        "range({sheet:?},{absolute_row1},{},{absolute_column1},{},{absolute_row2},{},{absolute_column2},{})\u{1}",
                        offset(row1, absolute_row1, job.row),
                        offset(column1, absolute_column1, job.column),
                        offset(row2, absolute_row2, job.row),
                        offset(column2, absolute_column2, job.column),
                    )?;
                }
                other => {
                    write!(self.key, "{other:?}\u{1}")?;
                }
            }
        }
        Ok(false)
    }
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$' | b'\\' | b'?')
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Word<'w> {
    // A cell reference: its column as written and its row.
    Cell {
        letters: &'w str,
        row: i32,
        relative_row: bool,
    },
    // A column alone, as in `A:C`.
    Column,
    // Looks like a reference IronCalc may read otherwise, such as a column past XFD.
    Invalid,
    Other,
}

const LAST_COLUMN: i32 = 16_384;
const LAST_ROW: i32 = 1_048_576;

fn word_kind(word: &str) -> Word<'_> {
    let bytes = word.as_bytes();
    let column_absolute = bytes.first() == Some(&b'$');
    let letters_start = usize::from(column_absolute);
    let letters_end = letters_start
        + bytes[letters_start..]
            .iter()
            .take_while(|b| b.is_ascii_alphabetic())
            .count();
    let row_absolute = bytes.get(letters_end) == Some(&b'$');
    let digits_start = letters_end + usize::from(row_absolute);
    let digits_end = digits_start
        + bytes[digits_start..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
    let letters = &word[letters_start..letters_end];
    let dollar = bytes.contains(&b'$');
    if digits_end != bytes.len() || letters.is_empty() {
        return if dollar { Word::Invalid } else { Word::Other };
    }
    if digits_start == digits_end {
        return if row_absolute {
            Word::Invalid
        } else if letters.len() <= 3 && column_number(letters) <= LAST_COLUMN {
            Word::Column
        } else if dollar {
            Word::Invalid
        } else {
            Word::Other
        };
    }
    if letters.len() > 3 {
        return if dollar { Word::Invalid } else { Word::Other };
    }
    let row = word[digits_start..].parse::<i32>().unwrap_or(0);
    if column_number(letters) > LAST_COLUMN || !(1..=LAST_ROW).contains(&row) {
        return Word::Invalid;
    }
    Word::Cell {
        letters: &word[..digits_start],
        row,
        relative_row: !row_absolute,
    }
}

fn column_number(letters: &str) -> i32 {
    letters.bytes().fold(0, |number, letter| {
        number * 26 + i32::from(letter.to_ascii_uppercase() - b'A') + 1
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use ironcalc::base::expressions::utils::number_to_column;

    #[test]
    fn worker_indexes_share_each_formula_text() {
        let sheets = vec!["Sheet1".to_string()];
        let defined_names = Vec::new();
        let tables = HashMap::new();
        let names = Names {
            sheets: &sheets,
            defined_names: &defined_names,
            tables: &tables,
        };
        let mut converter = Converter::new(&names);
        let first = converter.intern("A1*2".to_string());
        let second = converter.intern("A1*2".to_string());
        assert_eq!((first, second), (0, 0));
        assert_eq!(converter.distinct.len(), 1);
        assert!(Arc::ptr_eq(
            &converter.distinct[0],
            converter.index_of.keys().next().unwrap()
        ));
    }

    #[derive(Clone, Debug)]
    enum Atom {
        Relative(i32, i32),
        Absolute(i32, i32),
        MixedRow(i32, i32),
        Range(i32, i32, i32, i32),
        FixedStartRange(i32, i32),
        OtherSheet(i32, i32),
        Literal(&'static str),
        Function(&'static str, Vec<Atom>),
    }

    const LITERALS: [&str; 26] = [
        "1E5",
        "R1C1",
        "A1B",
        "XFE1",
        "\"x\"\"A1\"",
        "1:1",
        "$A:$B",
        "A1#",
        "#REF!",
        "A\u{f1}o2",
        "Rate2",
        "$5",
        "1",
        "2.5",
        "\"A1\"",
        "TRUE",
        "#N/A",
        "Rate",
        "Missing",
        "A:A",
        "3:5",
        "LOG10(100)",
        "Table1[Col]",
        "(",
        ")",
        "-",
    ];

    fn atom() -> impl Strategy<Value = Atom> {
        let offset = -3..4i32;
        let leaf = prop_oneof![
            (offset.clone(), offset.clone()).prop_map(|(r, c)| Atom::Relative(r, c)),
            (1..9i32, 1..9i32).prop_map(|(r, c)| Atom::Absolute(r, c)),
            (1..9i32, offset.clone()).prop_map(|(r, c)| Atom::MixedRow(r, c)),
            (
                offset.clone(),
                offset.clone(),
                offset.clone(),
                offset.clone()
            )
                .prop_map(|(a, b, c, d)| Atom::Range(a, b, c, d)),
            (1..20i32, offset.clone()).prop_map(|(r, c)| Atom::FixedStartRange(r, c)),
            (offset.clone(), offset).prop_map(|(r, c)| Atom::OtherSheet(r, c)),
            prop::sample::select(LITERALS.to_vec()).prop_map(Atom::Literal),
        ];
        leaf.prop_recursive(3, 12, 3, |inner| {
            (
                prop::sample::select(vec!["SUM", "IF", "SEQUENCE", "INDEX"]),
                prop::collection::vec(inner, 1..3),
            )
                .prop_map(|(name, args)| Atom::Function(name, args))
        })
    }

    fn cell(row: i32, column: i32) -> Option<String> {
        if row < 1 || column < 1 {
            return None;
        }
        Some(format!("{}{row}", number_to_column(column)?))
    }

    fn render(atom: &Atom, row: i32, column: i32) -> Option<String> {
        Some(match atom {
            Atom::Relative(r, c) => cell(row + r, column + c)?,
            Atom::Absolute(r, c) => format!("${}${r}", number_to_column(*c)?),
            Atom::MixedRow(r, c) => {
                let column = number_to_column(column + c)?;
                format!("{column}${r}")
            }
            Atom::Range(a, b, c, d) => {
                format!(
                    "{}:{}",
                    cell(row + a, column + b)?,
                    cell(row + c, column + d)?
                )
            }
            Atom::FixedStartRange(r, c) => format!("$A${r}:{}", cell(row, column + c)?),
            Atom::OtherSheet(r, c) => format!("'Other Sheet'!{}", cell(row + r, column + c)?),
            Atom::Literal(text) => (*text).to_string(),
            Atom::Function(name, args) => {
                let args: Option<Vec<String>> =
                    args.iter().map(|a| render(a, row, column)).collect();
                format!("{name}({})", args?.join(","))
            }
        })
    }

    fn formula(atoms: &[(Atom, &str)], row: i32, column: i32) -> Option<String> {
        let mut text = String::new();
        for (atom, operator) in atoms {
            text.push_str(&render(atom, row, column)?);
            text.push_str(operator);
        }
        Some(text)
    }

    #[test]
    fn fill_keys_shift_only_the_rows_of_relative_cells() {
        let sheets = vec!["Sheet1".to_string()];
        let defined_names = Vec::new();
        let tables = HashMap::new();
        let names = Names {
            sheets: &sheets,
            defined_names: &defined_names,
            tables: &tables,
        };
        let mut converter = Converter::new(&names);
        let mut key = |text: &str, row: i32| {
            let job = FormulaJob {
                text: text.to_string(),
                row,
                column: 3,
                array: false,
            };
            converter
                .fill_key(0, &job)
                .unwrap()
                .then(|| converter.fill_key.clone())
        };
        let second = key("IF(N2=\"A1\",E2*2,$B$1+LOG10(4)+A$7+SUM(A2:B3))", 2);
        let third = key("IF(N3=\"A1\",E3*2,$B$1+LOG10(4)+A$7+SUM(A3:B4))", 3);
        assert!(second.is_some());
        assert_eq!(second, third);
        assert_ne!(
            second,
            key("IF(N3=\"A1\",E3*3,$B$1+LOG10(4)+A$7+SUM(A3:B4))", 3)
        );
        assert_ne!(
            second,
            key("IF(N3=\"A2\",E3*2,$B$1+LOG10(4)+A$7+SUM(A3:B4))", 3)
        );
        assert_ne!(
            second,
            key("IF(N3=\"A1\",E3*2,$B$1+LOG10(4)+A$8+SUM(A3:B4))", 3)
        );
        assert_ne!(key("SUM($A$5:A4)", 4), key("SUM($A$5:A6)", 6));
        assert_eq!(key("SUM($A$5:A6)", 6), key("SUM($A$5:A7)", 7));
        for refused in [
            "SUM(3:5)",
            "SUM(A1:B)",
            "Table1[Col]",
            "A\u{f1}o1+1",
            "XFE1",
            "\"open",
            "SUM(A1:'Other'!B2)",
            "SUM($5:$7)",
        ] {
            assert_eq!(key(refused, 2), None, "{refused}");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

        // A formula filled down a column hits the fill cache; every result must be what a
        // fresh IronCalc parse gives for that cell.
        #[test]
        fn filled_conversions_equal_fresh_ones(
            atoms in prop::collection::vec(
                (atom(), prop::sample::select(vec!["+", "*", "&", ":", ",", " ", ""])),
                1..5,
            ),
            column in 1..12i32,
            rows in prop::collection::vec(1..60i32, 2..8),
        ) {
            let sheets = vec!["Sheet1".to_string(), "Other Sheet".to_string()];
            let defined_names = vec![("Rate".to_string(), None, "Sheet1!$A$1".to_string())];
            let tables = HashMap::new();
            let names = Names { sheets: &sheets, defined_names: &defined_names, tables: &tables };
            let mut cached = Converter::new(&names);
            let mut fresh = Converter::new(&names);
            for row in rows {
                let Some(text) = formula(&atoms, row, column) else { continue };
                let job = FormulaJob { text: text.clone(), row, column, array: false };
                let converted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (cached.convert(0, "Sheet1", &job), fresh.parse("Sheet1", &job).0)
                }));
                let Ok((id, expected)) = converted else { continue };
                prop_assert_eq!(&cached.distinct[id as usize][..], &expected, "{} at row {}", text, row);
            }
        }

        // The same formula copied to other cells hits the cache; every result must be what
        // a fresh IronCalc parse gives for that cell.
        #[test]
        fn cached_conversions_equal_fresh_ones(
            atoms in prop::collection::vec(
                (atom(), prop::sample::select(vec!["+", "*", "&", ":", ",", " ", ""])),
                1..5,
            ),
            places in prop::collection::vec((1..40i32, 1..12i32, any::<bool>()), 2..6),
        ) {
            let sheets = vec!["Sheet1".to_string(), "Other Sheet".to_string()];
            let defined_names = vec![("Rate".to_string(), None, "Sheet1!$A$1".to_string())];
            let tables = HashMap::new();
            let names = Names { sheets: &sheets, defined_names: &defined_names, tables: &tables };
            let mut cached = Converter::new(&names);
            let mut fresh = Converter::new(&names);
            for (row, column, array) in places {
                let Some(text) = formula(&atoms, row, column) else { continue };
                let job = FormulaJob { text: text.clone(), row, column, array };
                // IronCalc's lexer overflows on very long column names in debug builds.
                let converted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (cached.convert(0, "Sheet1", &job), fresh.parse("Sheet1", &job).0)
                }));
                let Ok((id, expected)) = converted else { continue };
            prop_assert_eq!(&cached.distinct[id as usize][..], &expected, "{} at {:?}", text, (row, column));
            }
        }
    }
}
