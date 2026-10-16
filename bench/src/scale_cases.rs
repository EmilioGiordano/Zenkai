use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rust_xlsxwriter::{Color, Format, FormatAlign, FormatUnderline, Formula, Workbook};
use zenkai_datagen::{
    ColumnKind, ColumnSpec, Gender, GenerationSpec, LastNameCount, Locale, Percent,
};

pub const CORPUS_ENV: &str = "ZENKAI_CORPUS_DIR";
pub const SMALL_NAME: &str = "scale-small.xlsx";
pub const FULL_NAME: &str = "scale-full.xlsx";
pub const SMALL_ROWS: u32 = 2_000;
pub const FULL_ROWS: u32 = 200_000;
pub const SEED: u64 = 0x5CA1E;

pub struct Case {
    pub name: String,
    pub path: PathBuf,
    // Rows for the fill stage; zero means the fill stage does not run on this case.
    pub fill_rows: u32,
    // Rows to generate when the case is synthetic; None opens an existing file.
    pub synthetic_rows: Option<u32>,
}

// Small synthetic always runs, even on CI. Full sizes and the private corpus run
// only locally, when ZENKAI_CORPUS_DIR is set; without it they are skipped.
pub fn discover(workdir: &Path) -> Result<(Vec<Case>, bool)> {
    let corpus = std::env::var_os(CORPUS_ENV).map(PathBuf::from);
    let (cases, full) = list_cases(workdir, corpus.as_deref())?;
    for case in &cases {
        if let Some(rows) = case.synthetic_rows {
            write_synthetic(&case.path, rows)?;
        }
    }
    Ok((cases, full))
}

fn list_cases(workdir: &Path, corpus: Option<&Path>) -> Result<(Vec<Case>, bool)> {
    let synthetic = |name: &str, rows: u32| Case {
        name: name.to_string(),
        path: workdir.join(name),
        fill_rows: rows,
        synthetic_rows: Some(rows),
    };
    let mut cases = vec![synthetic(SMALL_NAME, SMALL_ROWS)];
    let Some(corpus) = corpus else {
        return Ok((cases, false));
    };
    cases.push(synthetic(FULL_NAME, FULL_ROWS));
    for file in corpus_files(corpus)? {
        cases.push(Case {
            name: file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: file,
            fill_rows: 0,
            synthetic_rows: None,
        });
    }
    Ok((cases, true))
}

fn corpus_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("xlsx"))
        })
        .collect();
    files.sort();
    Ok(files)
}

pub fn synthetic_spec(rows: u32) -> GenerationSpec {
    let column = |header: &str, kind: ColumnKind| ColumnSpec {
        header: header.to_string(),
        kind,
        blanks: Percent::ZERO,
        unique: false,
    };
    GenerationSpec {
        rows,
        locale: Locale::EnglishUnitedStates,
        seed: SEED,
        columns: vec![
            column("id", ColumnKind::SequentialId { start: 1, step: 1 }),
            column(
                "name",
                ColumnKind::FullName {
                    gender: Gender::Any,
                    last_names: LastNameCount::One,
                },
            ),
            column(
                "amount",
                ColumnKind::Integer {
                    min: 1,
                    max: 10_000,
                },
            ),
            column(
                "price",
                ColumnKind::Decimal {
                    min: 1.0,
                    max: 1000.0,
                    places: 2,
                },
            ),
            column(
                "code",
                ColumnKind::Pattern {
                    pattern: "AAA-####".to_string(),
                },
            ),
        ],
    }
}

// Values come from crates/datagen; the formula column repeats one formula per row
// and formats cycle through thousands of distinct styles, like the spec's CI corpus.
pub fn write_synthetic(path: &Path, rows: u32) -> Result<()> {
    let spec = synthetic_spec(rows);
    let table = zenkai_datagen::generate(&spec).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut book = Workbook::new();
    let sheet = book.add_worksheet();
    for (col, header) in ["id", "name", "amount", "price", "code", "total"]
        .into_iter()
        .enumerate()
    {
        sheet.write_string(0, col as u16, header)?;
    }
    let formats = distinct_formats(rows);
    let width = table.first().map_or(0, Vec::len) as u16;
    for (index, values) in table.iter().enumerate() {
        let row = index as u32 + 1;
        for (col, value) in values.iter().enumerate() {
            let format = &formats[(index + col) % formats.len()];
            sheet.write_string_with_format(row, col as u16, value, format)?;
        }
        // A formula repeated down the column, as in the spec's medium file.
        sheet.write_formula(
            row,
            width,
            Formula::new(format!("=C{}&D{}", row + 1, row + 1)).set_result(""),
        )?;
    }
    book.save(path)?;
    Ok(())
}

fn distinct_formats(rows: u32) -> Vec<Format> {
    let bases = [
        Format::new().set_bold(),
        Format::new().set_italic(),
        Format::new().set_font_size(14),
        Format::new().set_num_format("0.00"),
        Format::new().set_num_format("#,##0"),
        Format::new().set_align(FormatAlign::Center),
        Format::new().set_font_strikethrough(),
        Format::new().set_underline(FormatUnderline::Single),
    ];
    let count = (rows / 100).clamp(8, 2000) as usize;
    (0..count)
        .map(|i| {
            bases[i % bases.len()]
                .clone()
                .set_font_color(Color::RGB((i as u32).wrapping_mul(12345) % 0xFF_FFFF))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_carries_rows_and_value_columns() {
        let spec = synthetic_spec(8);
        assert_eq!(spec.rows, 8);
        assert_eq!(spec.columns.len(), 5);
        let table = zenkai_datagen::generate(&spec).unwrap();
        assert_eq!(table.len(), 8);
        assert!(table.iter().all(|row| row.len() == 5));
    }

    #[test]
    fn tiny_workbook_is_written_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.xlsx");
        write_synthetic(&path, 8).unwrap();
        assert!(path.is_file());
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }

    #[test]
    fn without_a_corpus_only_the_small_case_is_listed() {
        let dir = tempfile::tempdir().unwrap();
        let (cases, full) = list_cases(dir.path(), None).unwrap();
        assert!(!full);
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, SMALL_NAME);
        assert_eq!(cases[0].synthetic_rows, Some(SMALL_ROWS));
    }

    #[test]
    fn a_corpus_adds_the_full_size_and_every_xlsx() {
        let dir = tempfile::tempdir().unwrap();
        let corpus = dir.path().join("corpus");
        std::fs::create_dir(&corpus).unwrap();
        std::fs::write(corpus.join("b.xlsx"), "fake").unwrap();
        std::fs::write(corpus.join("a.xlsx"), "fake").unwrap();
        std::fs::write(corpus.join("notes.txt"), "fake").unwrap();
        let (cases, full) = list_cases(dir.path(), Some(&corpus)).unwrap();
        assert!(full);
        let names: Vec<&str> = cases.iter().map(|case| case.name.as_str()).collect();
        assert_eq!(names, [SMALL_NAME, FULL_NAME, "a.xlsx", "b.xlsx"]);
        assert_eq!(cases[2].synthetic_rows, None);
        assert_eq!(cases[2].fill_rows, 0);
    }
}
