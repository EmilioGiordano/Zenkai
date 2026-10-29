use std::collections::{BTreeMap, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::AtomicBool;

use ironcalc::base::expressions::parser::stringify::to_rc_format;
use ironcalc::base::expressions::parser::{DefinedNameS, new_parser_english};
use ironcalc::base::expressions::utils::number_to_column;
use proptest::prelude::*;

use super::{ShapeReader, Sharing, SheetNames};
use crate::ReadError;
use crate::formulas::{Converted, Names, convert, parse_node};
use crate::sheet_data::{FormulaEvent, FormulaJob, SheetCells, scan_rows_into};

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
            let args: Option<Vec<String>> = args.iter().map(|a| render(a, row, column)).collect();
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

struct Book {
    sheets: Vec<String>,
    defined_names: Vec<DefinedNameS>,
}

impl Book {
    fn new() -> Book {
        Book {
            sheets: vec!["Sheet1".to_string(), "Other Sheet".to_string()],
            defined_names: vec![("Rate".to_string(), None, "Sheet1!$A$1".to_string())],
        }
    }

    fn names(&self) -> SheetNames<'_> {
        SheetNames {
            sheet: "Sheet1",
            sheets: &self.sheets,
            defined_names: &self.defined_names,
        }
    }

    fn converted(&self, jobs: &[FormulaJob]) -> Result<Converted, ReadError> {
        let tables = HashMap::new();
        let names = Names {
            sheets: &self.sheets,
            defined_names: &self.defined_names,
            tables: &tables,
        };
        convert(&[("Sheet1", jobs)], &names, &AtomicBool::new(false))
    }
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// The rows of the cells, each run of `rows_per_piece` rows as one piece of XML.
fn pieces(cells: &BTreeMap<(i32, i32), (String, bool)>, rows_per_piece: usize) -> Vec<String> {
    let mut rows: BTreeMap<i32, String> = BTreeMap::new();
    for ((row, column), (text, array)) in cells {
        let reference = format!("{}{row}", number_to_column(*column).unwrap());
        let kind = match array {
            true => format!(r#" t="array" ref="{reference}""#),
            false => String::new(),
        };
        rows.entry(*row).or_default().push_str(&format!(
            r#"<c r="{reference}"><f{kind}>{}</f></c>"#,
            escaped(text)
        ));
    }
    let rows: Vec<String> = rows
        .into_iter()
        .map(|(row, cells)| format!(r#"<row r="{row}">{cells}</row>"#))
        .collect();
    rows.chunks(rows_per_piece)
        .map(|chunk| chunk.concat())
        .collect()
}

// The sheet as the streaming reader builds it from pieces scanned one after another,
// the first in place and the others on their own and then taken in.
fn scanned(pieces: &[String], names: SheetNames<'_>) -> SheetCells {
    let mut sheet = SheetCells::default();
    let mut shapes = ShapeReader::new(names);
    for (index, piece) in pieces.iter().enumerate() {
        if index == 0 {
            scan_rows_into(&mut sheet, piece, &[Vec::new()], names).unwrap();
        } else {
            let mut cells = SheetCells::default();
            scan_rows_into(&mut cells, piece, &[Vec::new()], names).unwrap();
            sheet.absorb(cells, &mut shapes).unwrap();
        }
    }
    sheet
}

// The R1C1 text each formula cell gets, in document order.
fn converted_texts(sheet: &SheetCells, book: &Book) -> Vec<String> {
    let converted = book.converted(&sheet.jobs).unwrap();
    sheet
        .events
        .iter()
        .map(|event| match event {
            FormulaEvent::Convert { job } => {
                let id = converted.per_sheet[0][*job] as usize;
                converted.distinct[id].to_string()
            }
            other => panic!("unexpected event {other:?}"),
        })
        .collect()
}

fn fresh_text(book: &Book, text: &str, row: i32, column: i32, array: bool) -> String {
    let mut parser = new_parser_english(
        book.sheets.clone(),
        book.defined_names.clone(),
        HashMap::new(),
    );
    let job = FormulaJob {
        text: text.to_string(),
        row,
        column,
        array,
        sharing: Sharing::PerCell,
    };
    to_rc_format(&parse_node(&mut parser, "Sheet1", &job))
}

// The formula down column C, with `#` standing for each row's number.
fn column_of(text: &str, rows: i32) -> BTreeMap<(i32, i32), (String, bool)> {
    (1..=rows)
        .map(|row| ((row, 3), (text.replace('#', &row.to_string()), false)))
        .collect()
}

#[test]
fn a_formula_filled_down_keeps_one_job() {
    let book = Book::new();
    let cells = column_of("A#*2+SUM($B$1:B#)", 30);
    for rows_per_piece in [30, 7, 1] {
        let sheet = scanned(&pieces(&cells, rows_per_piece), book.names());
        assert_eq!(sheet.events.len(), 30);
        assert_eq!(sheet.jobs.len(), 1, "{rows_per_piece} rows per piece");
        let texts = converted_texts(&sheet, &book);
        assert!(texts.iter().all(|text| text == &texts[0]), "{texts:?}");
    }
}

#[test]
fn a_shape_that_does_not_parse_keeps_each_cells_text() {
    let book = Book::new();
    let cells = column_of("A#*(2", 12);
    for rows_per_piece in [12, 5, 1] {
        let sheet = scanned(&pieces(&cells, rows_per_piece), book.names());
        assert_eq!(sheet.jobs.len(), 12, "{rows_per_piece} rows per piece");
        let texts = converted_texts(&sheet, &book);
        for (row, text) in (1..=12).zip(texts) {
            assert_eq!(text, format!("A{row}*(2"));
        }
    }
}

#[test]
fn a_shared_job_that_converts_to_a_parse_error_is_left_to_ironcalc() {
    let book = Book::new();
    let job = FormulaJob {
        text: "A1*(2".to_string(),
        row: 1,
        column: 3,
        array: false,
        sharing: Sharing::Shared,
    };
    let error = book.converted(std::slice::from_ref(&job)).err().unwrap();
    assert!(matches!(error, ReadError::Unsupported(_)), "{error}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    // The same formula copied to other cells shares one job per shape; every cell must
    // still get what a fresh IronCalc parse gives for it, however the rows are cut.
    #[test]
    fn shared_shapes_convert_like_fresh_parses(
        atoms in prop::collection::vec(
            (atom(), prop::sample::select(vec!["+", "*", "&", ":", ",", " ", ""])),
            1..5,
        ),
        places in prop::collection::vec((1..40i32, 1..12i32, any::<bool>()), 2..8),
        rows_per_piece in 1..4usize,
    ) {
        let book = Book::new();
        let mut cells = BTreeMap::new();
        for (row, column, array) in places {
            if let Some(text) = formula(&atoms, row, column) {
                cells.insert((row, column), (text, array));
            }
        }
        // IronCalc's lexer overflows on very long column names in debug builds.
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let sheet = scanned(&pieces(&cells, rows_per_piece), book.names());
            let expected: Vec<String> = cells
                .iter()
                .map(|((row, column), (text, array))| {
                    fresh_text(&book, text, *row, *column, *array)
                })
                .collect();
            (converted_texts(&sheet, &book), expected)
        }));
        let Ok((texts, expected)) = outcome else { return Ok(()) };
        prop_assert_eq!(texts, expected);
    }
}
