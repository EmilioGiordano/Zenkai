use encoding_rs::{UTF_16BE, UTF_16LE, WINDOWS_1252};
use serde::{Deserialize, Serialize};
use zenkai_types::{MAX_COLS, MAX_ROWS};

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

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedCsv {
    pub rows: Vec<Vec<String>>,
    pub delimiter: Delimiter,
    pub encoding: Encoding,
    pub line_ending: LineEnding,
    pub ends_with_newline: bool,
    // Some bytes were not valid in the encoding and were replaced, so the rows are not the
    // file's own text and writing them back would change it.
    pub lossy: bool,
}

struct Decoded {
    text: String,
    encoding: Encoding,
    lossy: bool,
}

fn decode(bytes: &[u8]) -> Decoded {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return match String::from_utf8_lossy(rest) {
            std::borrow::Cow::Borrowed(text) => Decoded {
                text: text.to_string(),
                encoding: Encoding::Utf8Bom,
                lossy: false,
            },
            std::borrow::Cow::Owned(text) => Decoded {
                text,
                encoding: Encoding::Utf8Bom,
                lossy: true,
            },
        };
    }
    if bytes.starts_with(b"\xFF\xFE") {
        let (text, _, lossy) = UTF_16LE.decode(bytes);
        return Decoded {
            text: text.into_owned(),
            encoding: Encoding::Utf16Le,
            lossy,
        };
    }
    if bytes.starts_with(b"\xFE\xFF") {
        let (text, _, lossy) = UTF_16BE.decode(bytes);
        return Decoded {
            text: text.into_owned(),
            encoding: Encoding::Utf16Be,
            lossy,
        };
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => Decoded {
            text: text.to_string(),
            encoding: Encoding::Utf8,
            lossy: false,
        },
        Err(_) => {
            let (text, _, lossy) = WINDOWS_1252.decode(bytes);
            Decoded {
                text: text.into_owned(),
                encoding: Encoding::Windows1252,
                lossy,
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
// As in the csv reader, only a quote that starts a field opens a quoted one, so a stray quote
// inside a plain field (`5" pipe`) changes nothing.
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

// The csv reader skips empty lines. Reading the text the way it does (a quote opens a quoted
// field only at a field's start, and `\n`, `\r\n` and a lone `\r` all end a line) finds them:
// for each, how many records came before it, and how many records there are in all.
struct EmptyLines {
    after_records: Vec<usize>,
    records: usize,
}

fn empty_lines(text: &str, delimiter: Delimiter) -> EmptyLines {
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
                    found.after_records.push(found.records);
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
    found
}

// Puts the empty lines back among the records. If the two readings disagree about how many
// records there are, as they can on malformed quoting, the empty lines are left out.
fn with_empty_lines(
    records: Vec<Vec<String>>,
    empty: &EmptyLines,
) -> Result<Vec<Vec<String>>, CsvError> {
    if empty.after_records.is_empty() || empty.records != records.len() {
        return Ok(records);
    }
    if records.len() + empty.after_records.len() > MAX_ROWS as usize {
        return Err(CsvError::TooManyCells);
    }
    let mut rows = Vec::with_capacity(records.len() + empty.after_records.len());
    let mut empties = empty.after_records.iter().peekable();
    for (index, record) in records.into_iter().enumerate() {
        while empties.next_if(|before| **before == index).is_some() {
            rows.push(Vec::new());
        }
        rows.push(record);
    }
    rows.extend(empties.map(|_| Vec::new()));
    Ok(rows)
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
        lossy,
    } = decode(bytes);
    let delimiter = sniff(&text, hint);
    let line_ending = detect_line_ending(&text, delimiter);
    let ends_with_newline = text.is_empty() || text.ends_with(['\n', '\r']);
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
    let rows = with_empty_lines(records, &empty_lines(&text, delimiter))?;
    Ok(ParsedCsv {
        rows,
        delimiter,
        encoding,
        line_ending,
        ends_with_newline,
        lossy,
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
    fn says_whether_the_file_ends_with_a_line_break() {
        let ends = |text: &str| parse_csv(text.as_bytes(), None).unwrap().ends_with_newline;
        assert!(ends("a,b\n"));
        assert!(ends("a,b\r\n"));
        assert!(!ends("a,b"));
    }

    #[test]
    fn reports_when_decoding_replaced_something() {
        let lossy = |bytes: &[u8]| parse_csv(bytes, None).unwrap().lossy;
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
