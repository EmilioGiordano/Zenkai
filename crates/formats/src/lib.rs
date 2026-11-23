#![forbid(unsafe_code)]

mod csv_format;
mod csv_write;
mod dates;
mod numbers;
mod values;

pub use csv_format::{CsvError, Delimiter, Encoding, LineEnding, ParsedCsv, parse_csv};
pub use csv_write::{CsvFormat, CsvWriter, FormulaText, write_csv};
pub use dates::{
    DateLayout, DateOrder, DateSeparator, NORMALIZED_NUMBER_FORMAT, detect_date_layout,
    detect_date_order, normalize_day_first,
};
pub use numbers::{detect_decimal_comma, normalize_decimal_comma};
pub use values::{
    Block, MAX_ALLOCATED_CELLS, MAX_CELL_BYTES, MAX_CELLS, MAX_TEXT_BYTES, SheetValues,
    ValuesError, read_values,
};
