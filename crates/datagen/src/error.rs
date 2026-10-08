use std::fmt;

use crate::text::clip;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DatagenError {
    #[error("the spec has no columns")]
    NoColumns,
    #[error("{requested} rows do not fit below a header row; the limit is {limit}")]
    TooManyRows { requested: u32, limit: u32 },
    #[error("{requested} columns do not fit in a sheet of {limit} columns")]
    TooManyColumns { requested: usize, limit: u16 },
    #[error("{requested} cells exceed the limit of {limit} in one generation")]
    TooManyCells { requested: u64, limit: u64 },
    #[error("the rows would take up to {estimated_bytes} bytes; the limit is {limit}")]
    OutputTooLarge { estimated_bytes: u64, limit: u64 },
    #[error("column {} \"{}\": {problem}", .position + 1, clip(.header))]
    Column {
        position: usize,
        header: String,
        problem: ColumnProblem,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ColumnProblem {
    #[error("the minimum {min} is greater than the maximum {max}")]
    MinAboveMax { min: String, max: String },
    #[error("{value} is not a finite number")]
    NotFinite { value: String },
    #[error("{value} is beyond ±{limit}, the largest number a cell holds exactly")]
    BeyondPrecision { value: String, limit: i64 },
    #[error("{places} decimal places is more than the {limit} allowed")]
    TooManyPlaces { places: u8, limit: u8 },
    #[error("the step is zero, so every id would be the same")]
    ZeroStep,
    #[error("{needed} distinct values are needed but only {available} exist")]
    DomainTooSmall { needed: u64, available: u128 },
    #[error("no column is named \"{}\"", clip(.source_header))]
    UnknownSource { source_header: String },
    #[error("more than one column is named \"{}\"", clip(.source_header))]
    AmbiguousSource { source_header: String },
    #[error("column \"{}\" holds no first names", clip(.source_header))]
    NotFirstNameSource { source_header: String },
    #[error("column \"{}\" holds no last names", clip(.source_header))]
    NotLastNameSource { source_header: String },
    #[error("no email domains are listed")]
    NoDomains,
    #[error("\"{}\" is not a valid email domain", clip(.domain))]
    InvalidDomain { domain: String },
    #[error("the pattern ends with an escape character")]
    DanglingEscape,
    #[error("the pattern has no # or A placeholder")]
    NoPlaceholders,
    #[error("the list has no values")]
    EmptyList,
    #[error("the list has an empty value")]
    EmptyOption,
    #[error("the list repeats \"{}\"", clip(.value))]
    DuplicateOption { value: String },
    #[error("every weight is zero")]
    AllWeightsZero,
    #[error("weights cannot apply to a column of unique values")]
    WeightsWithUnique,
    #[error("the word count must be between 1 and {limit}")]
    WordCountOutOfRange { limit: u16 },
    #[error("the {field} has {chars} characters; the limit is {limit}")]
    TextTooLong {
        field: TextField,
        chars: usize,
        limit: usize,
    },
    #[error("the {field} has a control character other than tab or line break")]
    ControlCharacter { field: TextField },
    #[error("the list has {count} values; the limit is {limit}")]
    TooManyOptions { count: usize, limit: usize },
    #[error("{count} email domains are listed; the limit is {limit}")]
    TooManyDomains { count: usize, limit: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextField {
    Header,
    ListValue,
    Pattern,
    Domain,
}

impl fmt::Display for TextField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            TextField::Header => "header",
            TextField::ListValue => "list value",
            TextField::Pattern => "pattern",
            TextField::Domain => "email domain",
        })
    }
}
