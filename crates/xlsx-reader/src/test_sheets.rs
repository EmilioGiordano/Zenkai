use std::fmt::Write as _;
use std::io::Write;

pub(crate) const SHEET_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetData>"#;
pub(crate) const SHEET_TAIL: &str = "</sheetData></worksheet>";

const TEXTS: [&str; 6] = ["north", "south", "a &amp; b", "x &lt; y", "total", "ünï"];
const SHARED_RUN: u32 = 8;
const ARRAY_EVERY: u32 = 97;

// What a generated sheet holds: every kind of cell the streaming reader builds
// differently, so a comparison of two reading paths touches all of them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SheetShape {
    pub(crate) rows: u32,
    pub(crate) seed: u64,
    pub(crate) max_cells: u32,
    pub(crate) spills: bool,
    pub(crate) gaps: bool,
    pub(crate) shared_strings: u32,
}

impl SheetShape {
    pub(crate) fn plain(rows: u32, seed: u64) -> SheetShape {
        SheetShape {
            rows,
            seed,
            max_cells: 12,
            spills: false,
            gaps: false,
            shared_strings: 0,
        }
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u32) -> u32 {
        (self.next() % u64::from(bound.max(1))) as u32
    }
}

pub(crate) fn column_name(column: u32) -> String {
    let mut name = Vec::new();
    let mut rest = column;
    while rest > 0 {
        name.push(b'A' + ((rest - 1) % 26) as u8);
        rest = (rest - 1) / 26;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}

pub(crate) fn sheet_xml(shape: &SheetShape) -> String {
    let mut out = Vec::new();
    write_sheet(shape, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

pub(crate) fn write_sheet(shape: &SheetShape, out: &mut impl Write) -> std::io::Result<()> {
    let mut rng = Rng(shape.seed | 1);
    out.write_all(SHEET_HEAD.as_bytes())?;
    let mut row = 0u32;
    let mut text = String::new();
    for _ in 0..shape.rows {
        row += 1 + u32::from(shape.gaps && rng.below(10) == 0) * rng.below(3);
        text.clear();
        row_xml(&mut rng, shape, row, &mut text);
        out.write_all(text.as_bytes())?;
    }
    out.write_all(SHEET_TAIL.as_bytes())
}

fn row_xml(rng: &mut Rng, shape: &SheetShape, row: u32, out: &mut String) {
    out.push_str(&format!("<row r=\"{row}\""));
    match rng.below(12) {
        0 => out.push_str(" ht=\"21\" customHeight=\"1\""),
        1 => out.push_str(" hidden=\"1\""),
        2 => out.push_str(" outlineLevel=\"1\""),
        3 => out.push_str(" s=\"1\" customFormat=\"1\""),
        _ => {}
    }
    let mut cells = rng.below(shape.max_cells + 1);
    if rng.below(40) == 0 {
        cells *= 8;
    }
    let spilled = shape.spills && matches!((row + ARRAY_EVERY - 5) % ARRAY_EVERY, 1 | 2);
    if cells == 0 && !spilled && rng.below(2) == 0 {
        out.push_str("/>");
    } else {
        out.push('>');
        for column in 1..=cells.min(40) {
            cell_xml(rng, shape, row, column, out);
        }
        run_cells(shape, row, spilled, out);
        out.push_str("</row>");
    }
    if rng.below(5) == 0 {
        out.push('\n');
    }
}

// Columns past the random ones hold the cells whose content depends on the row alone:
// shared formula runs and the array formula with the cells it spills into.
fn run_cells(shape: &SheetShape, row: u32, spilled: bool, out: &mut String) {
    let at = (row - 1) % SHARED_RUN;
    let si = (row - 1) / SHARED_RUN;
    if at == 0 {
        let _ = write!(
            out,
            "<c r=\"BA{row}\"><f t=\"shared\" ref=\"BA{row}:BA{}\" si=\"{si}\">A{row}*2</f><v>{}</v></c>",
            row + SHARED_RUN - 1,
            row * 2
        );
    } else {
        let _ = write!(
            out,
            "<c r=\"BA{row}\"><f t=\"shared\" si=\"{si}\"/><v>{}</v></c>",
            row * 2
        );
    }
    if shape.spills {
        if (row + ARRAY_EVERY - 5) % ARRAY_EVERY == 0 {
            let _ = write!(
                out,
                "<c r=\"BB{row}\" cm=\"1\"><f t=\"array\" ref=\"BB{row}:BB{}\">1+1</f><v>2</v></c>",
                row + 2
            );
        } else if spilled {
            let _ = write!(out, "<c r=\"BB{row}\"><v>2</v></c>");
        }
    }
}

fn cell_xml(rng: &mut Rng, shape: &SheetShape, row: u32, column: u32, out: &mut String) {
    let place = format!("{}{row}", column_name(column));
    let style = rng.below(3);
    match rng.below(14) {
        0..=3 => {
            let _ = write!(
                out,
                "<c r=\"{place}\" s=\"{style}\"><v>{}</v></c>",
                rng.below(100_000) as f64 / 8.0
            );
        }
        4 if shape.shared_strings > 0 => {
            let _ = write!(
                out,
                "<c r=\"{place}\" t=\"s\"><v>{}</v></c>",
                rng.below(shape.shared_strings)
            );
        }
        5 | 6 => {
            let text = text_of(rng);
            let _ = write!(
                out,
                "<c r=\"{place}\" t=\"inlineStr\"><is><t>{text}</t></is></c>"
            );
        }
        7 => {
            let text = text_of(rng);
            let _ = write!(out, "<c r=\"{place}\" t=\"str\"><v>{text}</v></c>");
        }
        8 => {
            let _ = write!(
                out,
                "<c r=\"{place}\"><f>SUM(A{row}:A{})</f><v>{}</v></c>",
                row + 3,
                rng.below(50)
            );
        }
        9 => {
            let text = text_of(rng);
            let _ = write!(
                out,
                "<c r=\"{place}\" t=\"str\"><f>\"{text}\"&amp;\"!\"</f><v>{text}</v></c>"
            );
        }
        10 => {
            let _ = write!(out, "<c r=\"{place}\" t=\"b\"><v>{}</v></c>", rng.below(2));
        }
        11 => {
            let _ = write!(out, "<c r=\"{place}\" t=\"e\"><v>#DIV/0!</v></c>");
        }
        12 => {
            let _ = write!(out, "<c r=\"{place}\" s=\"{style}\"/>");
        }
        _ => {
            let _ = write!(out, "<c r=\"{place}\"><v>{}</v></c>", rng.below(1000));
        }
    }
}

// A small pool most of the time, so strings repeat across rows and pieces, and now
// and then a text no other cell has.
fn text_of(rng: &mut Rng) -> String {
    if rng.below(6) == 0 {
        format!("unique {}", rng.next() % 1_000_000)
    } else {
        TEXTS[rng.below(TEXTS.len() as u32) as usize].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet_data::scan_rows;

    #[test]
    fn a_generated_sheet_is_deterministic_and_reads_whole() {
        let shape = SheetShape {
            spills: true,
            shared_strings: 5,
            ..SheetShape::plain(400, 7)
        };
        let xml = sheet_xml(&shape);
        assert_eq!(xml, sheet_xml(&shape));
        let rows = &xml[SHEET_HEAD.len()..xml.len() - SHEET_TAIL.len()];
        let cells = scan_rows(rows, &[Vec::new()], "Sheet1").unwrap();
        assert!(cells.data.row_count() >= 400);
        assert!(!cells.strings.is_empty());
        assert!(!cells.events.is_empty());
        assert!(!cells.rows.is_empty());
    }
}
