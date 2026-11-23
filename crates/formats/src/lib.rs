#![forbid(unsafe_code)]

mod csv_format;
mod csv_guess;
mod csv_write;
mod dates;
mod numbers;
mod values;

pub use csv_format::{
    CsvError, Delimiter, Encoding, Fidelity, FinalNewline, LineEnding, MAX_CSV_BYTES, ParsedCsv,
    parse_csv,
};
pub use csv_guess::{DecimalMark, Guess, apply, guess, number_text};
pub use csv_write::{CsvFormat, CsvWriter, neutralize, write_csv};
pub use dates::{
    DateOrder, date_serial, day_first_field, detect_date_order, iso_date, normalize_day_first,
};
pub use numbers::{
    decimal_comma_field, detect_decimal_comma, normalize_decimal_comma, number_value,
    stored_number_text,
};
pub use values::{
    Block, MAX_ALLOCATED_CELLS, MAX_CELL_BYTES, MAX_CELLS, MAX_TEXT_BYTES, SheetValues,
    ValuesError, read_values,
};
