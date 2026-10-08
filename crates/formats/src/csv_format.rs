use encoding_rs::{UTF_16BE, UTF_16LE, WINDOWS_1252};
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedCsv {
    pub rows: Vec<Vec<String>>,
    pub delimiter: Delimiter,
    pub encoding: Encoding,
}

fn decode(bytes: &[u8]) -> (String, Encoding) {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return (
            String::from_utf8_lossy(rest).into_owned(),
            Encoding::Utf8Bom,
        );
    }
    if bytes.starts_with(b"\xFF\xFE") {
        let (text, _, _) = UTF_16LE.decode(bytes);
        return (text.into_owned(), Encoding::Utf16Le);
    }
    if bytes.starts_with(b"\xFE\xFF") {
        let (text, _, _) = UTF_16BE.decode(bytes);
        return (text.into_owned(), Encoding::Utf16Be);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_string(), Encoding::Utf8),
        Err(_) => {
            let (text, _, _) = WINDOWS_1252.decode(bytes);
            (text.into_owned(), Encoding::Windows1252)
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
    let (text, encoding) = decode(bytes);
    let delimiter = sniff(&text, hint);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter.byte())
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    let mut fields = 0usize;
    for record in reader.records() {
        let record = record.map_err(|e| CsvError::Malformed(e.to_string()))?;
        // Checked before the row is allocated, so a file of bare separators cannot
        // build billions of empty strings.
        fields += record.len();
        if rows.len() >= MAX_ROWS as usize
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
        rows.push(row);
    }
    Ok(ParsedCsv {
        rows,
        delimiter,
        encoding,
    })
}

// Cells that start like a formula are prefixed so another spreadsheet opening the
// export shows the text instead of evaluating it (CSV formula injection).
fn neutralize(text: &str) -> String {
    match text.chars().next() {
        Some('=' | '+' | '-' | '@' | '\t' | '\r') if text.parse::<f64>().is_err() => {
            format!("'{text}")
        }
        _ => text.to_string(),
    }
}

pub fn write_csv(rows: &[Vec<String>], delimiter: Delimiter) -> Result<Vec<u8>, CsvError> {
    let mut out = b"\xEF\xBB\xBF".to_vec();
    {
        let mut writer = csv::WriterBuilder::new()
            .delimiter(delimiter.byte())
            .terminator(csv::Terminator::CRLF)
            .flexible(true)
            .from_writer(&mut out);
        for row in rows {
            writer
                .write_record(row.iter().map(|cell| neutralize(cell)))
                .map_err(|e| CsvError::Malformed(e.to_string()))?;
        }
        writer
            .flush()
            .map_err(|e| CsvError::Malformed(e.to_string()))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
