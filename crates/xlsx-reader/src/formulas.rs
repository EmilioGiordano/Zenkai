use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

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
use crate::sheet_xml::FormulaJob;

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
    pub(crate) distinct: Vec<String>,
    pub(crate) per_sheet: Vec<Vec<u32>>,
}

struct Chunk {
    sheet: usize,
    start: usize,
    end: usize,
}

struct WorkerOutput {
    distinct: Vec<String>,
    chunks: Vec<(usize, Vec<u32>)>,
}

pub(crate) fn convert(
    sheets: &[(&str, &[FormulaJob<'_>])],
    names: &Names<'_>,
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
                    let output = work(sheets, &chunks, &next, names);
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
    let mut index_of: HashMap<String, u32> = HashMap::new();
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
    sheets: &[(&str, &[FormulaJob<'_>])],
    chunks: &[Chunk],
    next: &AtomicUsize,
    names: &Names<'_>,
) -> WorkerOutput {
    let mut converter = Converter::new(names);
    let mut chunk_ids = Vec::new();
    loop {
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some(chunk) = chunks.get(index) else {
            break;
        };
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
    index_of: HashMap<String, u32>,
    distinct: Vec<String>,
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
            index_of: HashMap::new(),
            distinct: Vec::new(),
        }
    }

    pub(crate) fn convert(&mut self, sheet: usize, sheet_name: &str, job: &FormulaJob<'_>) -> u32 {
        let cacheable = self.cache_key(sheet, job).unwrap_or(false);
        if cacheable && let Some(id) = self.cache.get(&self.key) {
            return *id;
        }
        let (text, reusable) = self.parse(sheet_name, job);
        let id = self.intern(text);
        if cacheable && reusable {
            self.cache.insert(self.key.clone(), id);
        }
        id
    }

    fn intern(&mut self, text: String) -> u32 {
        let next = u32::try_from(self.distinct.len()).unwrap_or(u32::MAX);
        if let Some(id) = self.index_of.get(&text) {
            return *id;
        }
        self.index_of.insert(text.clone(), next);
        self.distinct.push(text);
        next
    }

    // IronCalc's own conversion. The result is reusable for another cell with the same
    // key unless a parse error kept the raw formula text in it.
    pub(crate) fn parse(&mut self, sheet_name: &str, job: &FormulaJob<'_>) -> (String, bool) {
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
    fn cache_key(&mut self, sheet: usize, job: &FormulaJob<'_>) -> Result<bool, std::fmt::Error> {
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use ironcalc::base::expressions::utils::number_to_column;

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

    const LITERALS: [&str; 14] = [
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

    proptest! {
        #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

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
                let job = FormulaJob { text: text.clone().into(), row, column, array };
                let id = cached.convert(0, "Sheet1", &job);
                let (expected, _) = fresh.parse("Sheet1", &job);
                prop_assert_eq!(&cached.distinct[id as usize], &expected, "{} at {:?}", text, (row, column));
            }
        }
    }
}
