#![forbid(unsafe_code)]

use std::fmt;

pub const MAX_ROWS: u32 = 1_048_576;
pub const MAX_COLS: u16 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct SheetId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct RowIdx(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ColIdx(u16);

impl RowIdx {
    pub const LAST: RowIdx = RowIdx(MAX_ROWS - 1);

    pub fn new(index: u32) -> Option<RowIdx> {
        (index < MAX_ROWS).then_some(RowIdx(index))
    }

    pub fn clamped(index: i64) -> RowIdx {
        RowIdx(index.clamp(0, i64::from(MAX_ROWS - 1)) as u32)
    }

    pub fn get(self) -> u32 {
        self.0
    }

    pub fn offset(self, delta: i64) -> RowIdx {
        RowIdx::clamped(i64::from(self.0) + delta)
    }
}

impl ColIdx {
    pub const LAST: ColIdx = ColIdx(MAX_COLS - 1);

    pub fn new(index: u16) -> Option<ColIdx> {
        (index < MAX_COLS).then_some(ColIdx(index))
    }

    pub fn clamped(index: i64) -> ColIdx {
        ColIdx(index.clamp(0, i64::from(MAX_COLS - 1)) as u16)
    }

    pub fn get(self) -> u16 {
        self.0
    }

    pub fn offset(self, delta: i64) -> ColIdx {
        ColIdx::clamped(i64::from(self.0) + delta)
    }

    pub fn letters(self) -> String {
        let mut n = u32::from(self.0) + 1;
        let mut out = Vec::new();
        while n > 0 {
            let rem = (n - 1) % 26;
            out.push(b'A' + rem as u8);
            n = (n - 1) / 26;
        }
        out.reverse();
        String::from_utf8_lossy(&out).into_owned()
    }
}

impl fmt::Display for RowIdx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0 + 1)
    }
}

impl fmt::Display for ColIdx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.letters())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct CellPos {
    pub row: RowIdx,
    pub col: ColIdx,
}

impl CellPos {
    pub fn new(row: RowIdx, col: ColIdx) -> CellPos {
        CellPos { row, col }
    }

    pub fn parse_a1(text: &str) -> Option<CellPos> {
        let text = text.trim().replace('$', "").to_ascii_uppercase();
        let split = text.find(|c: char| c.is_ascii_digit())?;
        let (letters, digits) = text.split_at(split);
        if letters.is_empty()
            || letters.len() > 3
            || !letters.bytes().all(|b| b.is_ascii_uppercase())
        {
            return None;
        }
        let col = letters
            .bytes()
            .fold(0u32, |acc, b| acc * 26 + u32::from(b - b'A' + 1));
        let row: u32 = digits.parse().ok()?;
        Some(CellPos {
            row: RowIdx::new(row.checked_sub(1)?)?,
            col: ColIdx::new(u16::try_from(col.checked_sub(1)?).ok()?)?,
        })
    }
}

impl fmt::Display for CellPos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.col, self.row)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellRef {
    pub sheet: SheetId,
    pub pos: CellPos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: CellPos,
    pub end: CellPos,
}

impl Range {
    pub fn new(a: CellPos, b: CellPos) -> Range {
        Range {
            start: CellPos::new(a.row.min(b.row), a.col.min(b.col)),
            end: CellPos::new(a.row.max(b.row), a.col.max(b.col)),
        }
    }

    pub fn parse_a1(text: &str) -> Option<Range> {
        match text.split_once(':') {
            Some((a, b)) => Some(Range::new(CellPos::parse_a1(a)?, CellPos::parse_a1(b)?)),
            None => CellPos::parse_a1(text).map(Range::single),
        }
    }

    pub fn intersects(&self, other: &Range) -> bool {
        self.start.row <= other.end.row
            && other.start.row <= self.end.row
            && self.start.col <= other.end.col
            && other.start.col <= self.end.col
    }

    // Clipping must not go through `new`: sorting the corners would mirror a range
    // that starts past `end` onto unrelated cells.
    pub fn clip_to(&self, end: CellPos) -> Option<Range> {
        if self.start.row > end.row || self.start.col > end.col {
            return None;
        }
        Some(Range {
            start: self.start,
            end: CellPos::new(self.end.row.min(end.row), self.end.col.min(end.col)),
        })
    }

    pub fn single(pos: CellPos) -> Range {
        Range {
            start: pos,
            end: pos,
        }
    }

    pub fn contains(&self, pos: CellPos) -> bool {
        (self.start.row..=self.end.row).contains(&pos.row)
            && (self.start.col..=self.end.col).contains(&pos.col)
    }

    pub fn rows(&self) -> u32 {
        self.end.row.get() - self.start.row.get() + 1
    }

    pub fn cols(&self) -> u16 {
        self.end.col.get() - self.start.col.get() + 1
    }

    pub fn cell_count(&self) -> u64 {
        u64::from(self.rows()) * u64::from(self.cols())
    }

    pub fn positions(&self) -> impl Iterator<Item = CellPos> + '_ {
        (self.start.row.get()..=self.end.row.get()).flat_map(move |r| {
            (self.start.col.get()..=self.end.col.get())
                .map(move |c| CellPos::new(RowIdx(r), ColIdx(c)))
        })
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start == self.end {
            write!(f, "{}", self.start)
        } else {
            write!(f, "{}:{}", self.start, self.end)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u32);

impl Rgb {
    pub fn parse_hex(text: &str) -> Option<Rgb> {
        let hex = text.trim_start_matches('#');
        let hex = if hex.len() == 8 { &hex[2..] } else { hex };
        (hex.len() == 6)
            .then(|| u32::from_str_radix(hex, 16).ok())
            .flatten()
            .map(Rgb)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HAlign {
    #[default]
    General,
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ValueKind {
    #[default]
    Empty,
    Number,
    Text,
    Bool,
    Error,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub font_size: Option<f32>,
    pub font_color: Option<Rgb>,
    pub fill: Option<Rgb>,
    pub align: HAlign,
    pub border_bottom: bool,
    pub border_right: bool,
    pub num_fmt: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CellView {
    pub text: String,
    pub kind: ValueKind,
    pub number: Option<f64>,
    pub style: CellStyle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SheetInfo {
    pub id: SheetId,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleChange {
    Bold(bool),
    Italic(bool),
    Underline(bool),
    Strike(bool),
    FontSize(u16),
    Align(HAlign),
    NumberFormat(NumberFormat),
    FontColor(Option<Rgb>),
    Fill(Option<Rgb>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberFormat {
    General,
    Number,
    Currency,
    Percent,
    Date,
    Time,
}

impl NumberFormat {
    pub fn code(self) -> &'static str {
        match self {
            NumberFormat::General => "general",
            NumberFormat::Number => "#,##0.00",
            NumberFormat::Currency => "$#,##0.00",
            NumberFormat::Percent => "0.00%",
            NumberFormat::Date => "dd/mm/yyyy",
            NumberFormat::Time => "h:mm",
        }
    }
}

pub const DEFAULT_COL_WIDTH: f32 = 64.0;
pub const DEFAULT_ROW_HEIGHT: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnSpan {
    pub first: ColIdx,
    pub last: ColIdx,
    pub width: f32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct SheetSizes {
    pub columns: Vec<ColumnSpan>,
    pub rows: Vec<(RowIdx, f32)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_letters_round_trip() {
        for (index, letters) in [(0, "A"), (25, "Z"), (26, "AA"), (701, "ZZ"), (702, "AAA")] {
            let col = ColIdx::new(index).unwrap();
            assert_eq!(col.letters(), letters);
            let pos = CellPos::parse_a1(&format!("{letters}1")).unwrap();
            assert_eq!(pos.col, col);
        }
    }

    #[test]
    fn parse_a1_rejects_garbage() {
        for text in ["", "1", "A", "A0", "ZZZZ1", "A1048577", "a-1", "XFE1"] {
            assert_eq!(CellPos::parse_a1(text), None, "{text}");
        }
        assert_eq!(CellPos::parse_a1("$b$7").unwrap().to_string(), "B7");
        assert_eq!(Range::parse_a1("C3:A1").unwrap().to_string(), "A1:C3");
        assert_eq!(Range::parse_a1("A1:"), None);
        let used = CellPos::parse_a1("B2").unwrap();
        let past = Range::parse_a1("C1:F3").unwrap();
        assert_eq!(past.clip_to(used), None);
        let overlapping = Range::parse_a1("A2:F9").unwrap();
        assert_eq!(overlapping.clip_to(used).unwrap().to_string(), "A2:B2");
    }
}
