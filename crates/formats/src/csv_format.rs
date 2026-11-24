use encoding_rs::{UTF_16BE, UTF_16LE, WINDOWS_1252};
use serde::{Deserialize, Serialize};
use zenkai_types::{MAX_COLS, MAX_ROWS, RowIdx};

pub const MAX_CSV_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_FIELD_BYTES: usize = 32_767;
// About 1 GB of parsed rows at worst (empty fields); 100k x 20 typical imports use 2M.
pub const MAX_CSV_FIELDS: usize = 40_000_000;
const SNIFF_LINES: usize = 50;

#[derive(Debug, thiserror::Error)]
pub enum CsvError {
    #[error("the file is {0} MB, larger than the 512 MB supported for CSV")]
    TooLarge(usize),
    #[error("the CSV has more than {MAX_ROWS} rows, {MAX_COLS} columns or {MAX_CSV_FIELDS} cells")]
    TooManyCells,
    #[error("the CSV could not be read: {0}")]
    Malformed(String),
    #[error("the CSV could not be written: {0}")]
    Write(#[from] std::io::Error),
}

impl From<csv::Error> for CsvError {
    // The kind of the underlying write failure survives, so callers can tell text an encoding
    // cannot hold (InvalidData) from a disk error.
    fn from(error: csv::Error) -> CsvError {
        let kind = match error.kind() {
            csv::ErrorKind::Io(source) => source.kind(),
            _ => std::io::ErrorKind::Other,
        };
        CsvError::Write(std::io::Error::new(kind, error))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delimiter {
    Comma,
    Semicolon,
    Tab,
    Pipe,
}

impl Delimiter {
    const ALL: [Delimiter; 4] = [
        Delimiter::Comma,
        Delimiter::Semicolon,
        Delimiter::Tab,
        Delimiter::Pipe,
    ];

    pub fn byte(self) -> u8 {
        match self {
            Delimiter::Comma => b',',
            Delimiter::Semicolon => b';',
            Delimiter::Tab => b'\t',
            Delimiter::Pipe => b'|',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Delimiter::Comma => "comma",
            Delimiter::Semicolon => "semicolon",
            Delimiter::Tab => "tab",
            Delimiter::Pipe => "pipe",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

impl Encoding {
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 with BOM",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Windows1252 => "Windows-1252",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEnding {
    Crlf,
    Lf,
    Cr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinalNewline {
    Present,
    Missing,
}

// Whether the rows are the file's own, so that writing them back leaves it as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fidelity {
    Exact,
    // Some bytes were not valid in the encoding and were replaced.
    BytesReplaced,
    // The empty lines of the file could not be put back among its rows.
    EmptyLinesUnplaced,
}

impl Fidelity {
    fn from_decoding(had_errors: bool) -> Fidelity {
        if had_errors {
            Fidelity::BytesReplaced
        } else {
            Fidelity::Exact
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedCsv {
    pub rows: Vec<Vec<String>>,
    pub delimiter: Delimiter,
    pub encoding: Encoding,
    pub line_ending: LineEnding,
    pub final_newline: FinalNewline,
    pub fidelity: Fidelity,
}

struct Decoded {
    text: String,
    encoding: Encoding,
    fidelity: Fidelity,
}

fn decode(bytes: &[u8]) -> Decoded {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return match String::from_utf8_lossy(rest) {
            std::borrow::Cow::Borrowed(text) => Decoded {
                text: text.to_string(),
                encoding: Encoding::Utf8Bom,
                fidelity: Fidelity::Exact,
            },
            std::borrow::Cow::Owned(text) => Decoded {
                text,
                encoding: Encoding::Utf8Bom,
                fidelity: Fidelity::BytesReplaced,
            },
        };
    }
    if bytes.starts_with(b"\xFF\xFE") {
        let (text, _, lossy) = UTF_16LE.decode(bytes);
        return Decoded {
            text: text.into_owned(),
            encoding: Encoding::Utf16Le,
            fidelity: Fidelity::from_decoding(lossy),
        };
    }
    if bytes.starts_with(b"\xFE\xFF") {
        let (text, _, lossy) = UTF_16BE.decode(bytes);
        return Decoded {
            text: text.into_owned(),
            encoding: Encoding::Utf16Be,
            fidelity: Fidelity::from_decoding(lossy),
        };
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Decoded {
            text: text.to_string(),
            encoding: Encoding::Utf8,
            fidelity: Fidelity::Exact,
        },
        Err(_) => {
            let (text, _, lossy) = WINDOWS_1252.decode(bytes);
            Decoded {
                text: text.into_owned(),
                encoding: Encoding::Windows1252,
                fidelity: Fidelity::from_decoding(lossy),
            }
        }
    }
}

fn sniff(text: &str, hint: Option<Delimiter>) -> Delimiter {
    if let Some(hint) = hint {
        return hint;
    }
    let sample: Vec<&str> = text.lines().take(SNIFF_LINES).collect();
    let score = |delimiter: Delimiter| -> (usize, usize) {
        let counts: Vec<usize> = sample
            .iter()
            .map(|line| count_outside_quotes(line, delimiter.byte() as char))
            .collect();
        let first = counts.first().copied().unwrap_or(0);
        let consistent = counts.iter().filter(|c| **c == first && first > 0).count();
        (consistent, first)
    };
    Delimiter::ALL
        .into_iter()
        .max_by_key(|d| score(*d))
        .filter(|d| score(*d).1 > 0)
        .unwrap_or(Delimiter::Comma)
}

// The first line break outside quotes decides; a file without one gets what exports write.
// Only a quote that starts a field opens a quoted one, as in the csv reader.
fn detect_line_ending(text: &str, delimiter: Delimiter) -> LineEnding {
    let delimiter = char::from(delimiter.byte());
    let mut quoted = false;
    let mut field_start = true;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                } else {
                    quoted = false;
                    field_start = false;
                }
            }
            continue;
        }
        match c {
            '"' if field_start => quoted = true,
            '\n' => return LineEnding::Lf,
            '\r' => {
                return if chars.peek() == Some(&'\n') {
                    LineEnding::Crlf
                } else {
                    LineEnding::Cr
                };
            }
            c => field_start = c == delimiter,
        }
    }
    LineEnding::Crlf
}

// The csv reader skips empty lines, so they are found by reading the text as it does. For each,
// how many records came before it; and how many records there are in all.
struct EmptyLines {
    after_records: Vec<RowIdx>,
    records: usize,
}

// Records and empty lines together may not pass a sheet's rows, checked as they are counted so
// a file of bare line breaks cannot grow the list.
fn empty_lines(text: &str, delimiter: Delimiter) -> Result<EmptyLines, CsvError> {
    let delimiter = char::from(delimiter.byte());
    let mut found = EmptyLines {
        after_records: Vec::new(),
        records: 0,
    };
    let mut quoted = false;
    let mut field_start = true;
    let mut line_has_content = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                } else {
                    quoted = false;
                    field_start = false;
                }
            }
            continue;
        }
        match c {
            '"' if field_start => {
                quoted = true;
                line_has_content = true;
            }
            '\n' | '\r' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                if line_has_content {
                    found.records += 1;
                } else {
                    let before = i64::try_from(found.records).unwrap_or(i64::MAX);
                    found.after_records.push(RowIdx::clamped(before));
                }
                if found.records + found.after_records.len() > MAX_ROWS as usize {
                    return Err(CsvError::TooManyCells);
                }
                line_has_content = false;
                field_start = true;
            }
            c => {
                line_has_content = true;
                field_start = c == delimiter;
            }
        }
    }
    if line_has_content {
        found.records += 1;
    }
    Ok(found)
}

// Puts the empty lines back among the records; they stay unplaced when the two readings counted
// different records (malformed quoting).
fn with_empty_lines(
    records: Vec<Vec<String>>,
    empty: &EmptyLines,
) -> Result<(Vec<Vec<String>>, Fidelity), CsvError> {
    if empty.after_records.is_empty() {
        return Ok((records, Fidelity::Exact));
    }
    if empty.records != records.len() {
        return Ok((records, Fidelity::EmptyLinesUnplaced));
    }
    if records.len() + empty.after_records.len() > MAX_ROWS as usize {
        return Err(CsvError::TooManyCells);
    }
    let mut rows = Vec::with_capacity(records.len() + empty.after_records.len());
    let mut empties = empty.after_records.iter().peekable();
    for (index, record) in records.into_iter().enumerate() {
        while empties
            .next_if(|before| before.get() as usize == index)
            .is_some()
        {
            rows.push(Vec::new());
        }
        rows.push(record);
    }
    rows.extend(empties.map(|_| Vec::new()));
    Ok((rows, Fidelity::Exact))
}

fn count_outside_quotes(line: &str, delimiter: char) -> usize {
    let mut quoted = false;
    line.chars()
        .filter(|c| {
            if *c == '"' {
                quoted = !quoted;
            }
            !quoted && *c == delimiter
        })
        .count()
}

pub fn parse_csv(bytes: &[u8], hint: Option<Delimiter>) -> Result<ParsedCsv, CsvError> {
    if bytes.len() > MAX_CSV_BYTES {
        return Err(CsvError::TooLarge(bytes.len() / 1024 / 1024));
    }
    let Decoded {
        text,
        encoding,
        fidelity: decoded,
    } = decode(bytes);
    let delimiter = sniff(&text, hint);
    let line_ending = detect_line_ending(&text, delimiter);
    let final_newline = if text.is_empty() || text.ends_with(['\n', '\r']) {
        FinalNewline::Present
    } else {
        FinalNewline::Missing
    };
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter.byte())
        .from_reader(text.as_bytes());
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut fields = 0usize;
    for record in reader.records() {
        let record = record.map_err(|e| CsvError::Malformed(e.to_string()))?;
        // Checked before the row is allocated, so a file of bare separators cannot
        // build billions of empty strings.
        fields += record.len();
        if records.len() >= MAX_ROWS as usize
            || record.len() > usize::from(MAX_COLS)
            || fields > MAX_CSV_FIELDS
        {
            return Err(CsvError::TooManyCells);
        }
        let row = record
            .iter()
            .map(|field| {
                if field.len() > MAX_FIELD_BYTES {
                    Err(CsvError::Malformed(format!(
                        "a field has {} bytes, more than a cell can hold",
                        field.len()
                    )))
                } else {
                    Ok(field.to_string())
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        records.push(row);
    }
    let (rows, placed) = with_empty_lines(records, &empty_lines(&text, delimiter)?)?;
    let fidelity = if decoded == Fidelity::Exact {
        placed
    } else {
        decoded
    };
    Ok(ParsedCsv {
        rows,
        delimiter,
        encoding,
        line_ending,
        final_newline,
        fidelity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csv_write::write_csv;

    #[test]
    fn refuses_more_columns_than_a_sheet_holds() {
        let wide = ",".repeat(usize::from(MAX_COLS));
        assert!(matches!(
            parse_csv(wide.as_bytes(), Some(Delimiter::Comma)),
            Err(CsvError::TooManyCells)
        ));
        let fits = ",".repeat(usize::from(MAX_COLS) - 1);
        assert!(parse_csv(fits.as_bytes(), Some(Delimiter::Comma)).is_ok());
    }

    #[test]
    fn detects_semicolon_and_quoted_newlines() {
        let csv = "name;note;amount\n\"Ana\";\"line 1\nline 2\";1,5\nLuis;ok;2\n";
        let parsed = parse_csv(csv.as_bytes(), None).unwrap();
        assert_eq!(parsed.delimiter, Delimiter::Semicolon);
        assert_eq!(parsed.encoding, Encoding::Utf8);
        assert_eq!(parsed.rows[1], vec!["Ana", "line 1\nline 2", "1,5"]);
        assert_eq!(parsed.rows.len(), 3);
    }

    #[test]
    fn decodes_bom_utf16_and_windows_1252() {
        let parsed = parse_csv(b"\xEF\xBB\xBFa,b\n1,2\n", None).unwrap();
        assert_eq!(parsed.encoding, Encoding::Utf8Bom);
        assert_eq!(parsed.rows[0], vec!["a", "b"]);

        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("x\ty\n".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let parsed = parse_csv(&utf16, None).unwrap();
        assert_eq!(parsed.encoding, Encoding::Utf16Le);
        assert_eq!(parsed.delimiter, Delimiter::Tab);
        assert_eq!(parsed.rows[0], vec!["x", "y"]);

        let parsed = parse_csv(b"caf\xe9,ni\xf1o\n", None).unwrap();
        assert_eq!(parsed.encoding, Encoding::Windows1252);
        assert_eq!(parsed.rows[0], vec!["café", "niño"]);
    }

    #[test]
    fn detects_the_line_ending_outside_quotes() {
        let ending = |text: &str| {
            parse_csv(text.as_bytes(), Some(Delimiter::Comma))
                .unwrap()
                .line_ending
        };
        assert_eq!(ending("a,b\r\n1,2\r\n"), LineEnding::Crlf);
        assert_eq!(ending("a,b\n1,2\n"), LineEnding::Lf);
        assert_eq!(ending("a,b\r1,2\r"), LineEnding::Cr);
        assert_eq!(ending("\"x\r\ny\",b\n1,2\n"), LineEnding::Lf);
        assert_eq!(ending("a,b"), LineEnding::Crlf);
        assert_eq!(ending("5\" pipe,x\n1,2\n"), LineEnding::Lf);
        assert_eq!(ending("5\" pipe,\"y\r\nz\"\n"), LineEnding::Lf);
        assert_eq!(ending("\"say \"\"hi\"\"\",b\r\n1,2\r\n"), LineEnding::Crlf);
    }

    #[test]
    fn empty_lines_stay_empty_rows_so_the_rows_after_them_keep_their_place() {
        let rows = |text: &str| {
            parse_csv(text.as_bytes(), Some(Delimiter::Comma))
                .unwrap()
                .rows
        };
        let empty: Vec<String> = Vec::new();
        let row = |fields: &[&str]| fields.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(rows("a\n\nb\n"), [row(&["a"]), empty.clone(), row(&["b"])]);
        assert_eq!(
            rows("\n\na,b\r\n\r\nc\r\n"),
            [
                empty.clone(),
                empty.clone(),
                row(&["a", "b"]),
                empty.clone(),
                row(&["c"])
            ]
        );
        assert_eq!(
            rows("\"x\ny\",1\n\nz\n"),
            [row(&["x\ny", "1"]), empty.clone(), row(&["z"])]
        );
        assert_eq!(rows("a\n\n"), [row(&["a"]), empty.clone()]);
        assert_eq!(rows("a\r\rb\r"), [row(&["a"]), empty.clone(), row(&["b"])]);
        assert_eq!(rows("\n\n"), [empty.clone(), empty.clone()]);
        assert_eq!(rows("a,b\n,,\n"), [row(&["a", "b"]), row(&["", "", ""])]);
        assert_eq!(
            rows("5\" pipe\n\nz"),
            [row(&["5\" pipe"]), empty, row(&["z"])]
        );
    }

    #[test]
    fn a_file_of_bare_line_breaks_is_refused_before_it_fills_memory() {
        let breaks = "\n".repeat(MAX_ROWS as usize + 1);
        assert!(matches!(
            parse_csv(breaks.as_bytes(), Some(Delimiter::Comma)),
            Err(CsvError::TooManyCells)
        ));
        let fits = "\n".repeat(MAX_ROWS as usize - 1) + "a\n";
        assert_eq!(
            parse_csv(fits.as_bytes(), Some(Delimiter::Comma))
                .unwrap()
                .rows
                .len(),
            MAX_ROWS as usize
        );
    }

    #[test]
    fn empty_lines_that_cannot_be_placed_make_the_shape_unknown() {
        let record = vec![vec!["a".to_string()]];
        let disagreeing = EmptyLines {
            after_records: vec![RowIdx::clamped(0)],
            records: 3,
        };
        let (rows, fidelity) = with_empty_lines(record.clone(), &disagreeing).unwrap();
        assert_eq!(fidelity, Fidelity::EmptyLinesUnplaced);
        assert_eq!(rows, record);
        let agreeing = EmptyLines {
            after_records: vec![RowIdx::clamped(0)],
            records: 1,
        };
        let (rows, fidelity) = with_empty_lines(record, &agreeing).unwrap();
        assert_eq!(fidelity, Fidelity::Exact);
        assert_eq!(rows.len(), 2);
        let parsed = parse_csv(b"a\n\nb\n", Some(Delimiter::Comma)).unwrap();
        assert_eq!(parsed.fidelity, Fidelity::Exact);
    }

    #[test]
    fn says_whether_the_file_ends_with_a_line_break() {
        let ends = |text: &str| parse_csv(text.as_bytes(), None).unwrap().final_newline;
        assert_eq!(ends("a,b\n"), FinalNewline::Present);
        assert_eq!(ends("a,b\r\n"), FinalNewline::Present);
        assert_eq!(ends("a,b"), FinalNewline::Missing);
    }

    #[test]
    fn reports_when_decoding_replaced_something() {
        let lossy =
            |bytes: &[u8]| parse_csv(bytes, None).unwrap().fidelity == Fidelity::BytesReplaced;
        assert!(!lossy(b"a,b\n"));
        assert!(!lossy(b"\xEF\xBB\xBFa,b\n"));
        assert!(!lossy(b"caf\xe9,x\n"));
        assert!(lossy(b"\xEF\xBB\xBFa,\xFF\n"));
        let utf16 = |text: &str| -> Vec<u8> {
            [0xFF, 0xFE]
                .into_iter()
                .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
                .collect()
        };
        assert!(!lossy(&utf16("a,b\n")));
        let mut odd = utf16("a,b\n");
        odd.push(0x61);
        assert!(lossy(&odd));
        let mut surrogate = vec![0xFF, 0xFE, 0x00, 0xD8];
        surrogate.extend(utf16("a,b\n").into_iter().skip(2));
        assert!(lossy(&surrogate));
    }

    #[test]
    fn export_has_bom_and_neutralizes_formulas() {
        let rows = vec![
            vec!["=HYPERLINK(\"x\")".to_string(), "-5".to_string()],
            vec!["@SUM(A1)".to_string(), "a,b".to_string()],
        ];
        let bytes = write_csv(&rows, Delimiter::Comma).unwrap();
        let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
        assert!(bytes.starts_with(b"\xEF\xBB\xBF"));
        assert_eq!(
            text,
            "\"'=HYPERLINK(\"\"x\"\")\",-5\r\n'@SUM(A1),\"a,b\"\r\n"
        );
    }

    #[test]
    fn round_trips_through_export_and_import() {
        let rows = vec![
            vec!["héllo".to_string(), "multi\nline".to_string()],
            vec!["1".to_string(), String::new()],
        ];
        let bytes = write_csv(&rows, Delimiter::Comma).unwrap();
        let parsed = parse_csv(&bytes, None).unwrap();
        assert_eq!(parsed.rows, rows);
    }
}

// Any bytes a file may hold must parse or fail cleanly, never panic.
#[cfg(test)]
mod no_panic {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 1_000, ..ProptestConfig::default() })]

        #[test]
        fn csv_accepts_any_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
            let _ = super::parse_csv(&bytes, None);
        }
    }
}
