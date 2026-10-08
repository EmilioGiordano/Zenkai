#![forbid(unsafe_code)]

mod date;
mod distinct;
mod email;
mod error;
mod generate;
mod locale;
mod lorem;
mod pattern;
mod percent;
mod plan;
mod rng;
mod spec;
mod text;
mod value_set;

pub use date::{Date, InvalidDate};
pub use error::{ColumnProblem, DatagenError};
pub use generate::{generate, validate};
pub use percent::{InvalidPercent, Percent};
pub use spec::{
    ColumnKind, ColumnSpec, EmailFormat, Gender, GenerationSpec, LastNameCount, ListOption, Locale,
};
