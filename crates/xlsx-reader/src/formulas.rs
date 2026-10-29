use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ironcalc::base::expressions::parser::static_analysis::add_implicit_intersection;
use ironcalc::base::expressions::parser::stringify::to_rc_format;
use ironcalc::base::expressions::parser::{DefinedNameS, Node, Parser, new_parser_english};
use ironcalc::base::expressions::types::CellReferenceRC;
use ironcalc::base::types::Table;

use crate::ReadError;
use crate::shapes::Sharing;
use crate::sheet_data::FormulaJob;

const CHUNK_FORMULAS: usize = 4_096;
// IronCalc's parser and stringifier recurse on the formula tree; the engine gives them
// this same stack, sized for the longest formula the limits allow. Every reader thread
// that parses gets it: the sheet and row threads parse a formula's shape once it repeats.
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

fn stopped() -> ReadError {
    ReadError::Unsupported("a formula worker stopped".to_string())
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
            handle.join().map_err(|_| stopped())?;
        }
        Ok(())
    })?;
    let outputs = outputs.into_inner().map_err(|_| stopped())?;

    let mut distinct = Vec::new();
    let mut index_of: HashMap<Arc<str>, u32> = HashMap::new();
    let mut per_sheet: Vec<Vec<u32>> = sheets.iter().map(|(_, jobs)| vec![0; jobs.len()]).collect();
    let mut done = 0;
    for output in outputs {
        let output = output?;
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
) -> Result<WorkerOutput, ReadError> {
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
            .map(|job| converter.convert(sheet_name, job))
            .collect::<Result<_, _>>()?;
        chunk_ids.push((index, ids));
    }
    Ok(WorkerOutput {
        distinct: converter.distinct,
        chunks: chunk_ids,
    })
}

pub(crate) struct Converter<'n> {
    parser: Parser<'n>,
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
            index_of: HashMap::new(),
            distinct: Vec::new(),
        }
    }

    // A job the scan shared between cells must give text that holds for all of them; the
    // scan asked the same parse without the tables, so a disagreement is left to IronCalc.
    pub(crate) fn convert(&mut self, sheet_name: &str, job: &FormulaJob) -> Result<u32, ReadError> {
        let node = parse_node(&mut self.parser, sheet_name, job);
        if job.sharing == Sharing::Shared && keeps_cell_text(&node) {
            return Err(ReadError::Unsupported(
                "a repeated formula that parses differently".to_string(),
            ));
        }
        Ok(self.intern(to_rc_format(&node)))
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
}

// IronCalc's own conversion of one formula at its cell.
pub(crate) fn parse_node(parser: &mut Parser<'_>, sheet_name: &str, job: &FormulaJob) -> Node {
    let context = CellReferenceRC {
        sheet: sheet_name.to_string(),
        column: job.column,
        row: job.row,
    };
    let mut node = parser.parse(&job.text, &context);
    if !job.array {
        add_implicit_intersection(&mut node, true);
    }
    node
}

// A parse error keeps the raw formula text in the tree, so its R1C1 text is that cell's
// own and never holds for another cell. The walk keeps its own stack: a long operator
// chain nests thousands of levels deep.
pub(crate) fn keeps_cell_text(node: &Node) -> bool {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        match node {
            Node::ParseErrorKind { .. } => return true,
            Node::OpRangeKind { left, right }
            | Node::OpConcatenateKind { left, right }
            | Node::OpSumKind { left, right, .. }
            | Node::OpProductKind { left, right, .. }
            | Node::OpPowerKind { left, right }
            | Node::CompareKind { left, right, .. } => {
                pending.push(left);
                pending.push(right);
            }
            Node::FunctionKind { args, .. } | Node::NamedFunctionKind { args, .. } => {
                pending.extend(args);
            }
            Node::LambdaCallKind { lambda, args } => {
                pending.push(lambda);
                pending.extend(args);
            }
            Node::LambdaDefKind { body: child, .. }
            | Node::ImplicitIntersection { child, .. }
            | Node::SpillRangeOperator { child }
            | Node::UnaryKind { right: child, .. } => pending.push(child),
            Node::BooleanKind(_)
            | Node::NumberKind(_)
            | Node::StringKind(_)
            | Node::ReferenceKind { .. }
            | Node::RangeKind { .. }
            | Node::WrongReferenceKind { .. }
            | Node::WrongRangeKind { .. }
            | Node::ArrayKind(_)
            | Node::DefinedNameKind(_)
            | Node::TableNameKind(_)
            | Node::NamedVariableKind { .. }
            | Node::ErrorKind(_)
            | Node::EmptyArgKind => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn parsed(text: &str) -> Node {
        let mut parser = new_parser_english(vec!["Sheet1".to_string()], Vec::new(), HashMap::new());
        let job = FormulaJob {
            text: text.to_string(),
            row: 1,
            column: 1,
            array: false,
            sharing: Sharing::PerCell,
        };
        parse_node(&mut parser, "Sheet1", &job)
    }

    #[test]
    fn only_a_parse_error_keeps_the_cell_text() {
        assert!(keeps_cell_text(&parsed("A1*(2")));
        assert!(keeps_cell_text(&parsed("1+SUM(B1,(2")));
        assert!(!keeps_cell_text(&parsed("SUM(B1:B3)*2")));
        assert!(!keeps_cell_text(&parsed("\"ParseErrorKind\"&B1")));
    }
}
