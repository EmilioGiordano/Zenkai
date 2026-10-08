#![forbid(unsafe_code)]

mod csv_format;

pub use csv_format::{CsvError, Delimiter, Encoding, ParsedCsv, parse_csv, write_csv};
