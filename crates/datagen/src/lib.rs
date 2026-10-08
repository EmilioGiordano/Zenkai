#![forbid(unsafe_code)]

mod date;
mod detect;
mod distinct;
mod email;
mod error;
mod generate;
mod limits;
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
pub use detect::{detect_kind, detect_kinds};
pub use error::{ColumnProblem, DatagenError, TextField};
pub use generate::{generate, validate};
pub use percent::{InvalidPercent, Percent};
pub use spec::{
    ColumnKind, ColumnSpec, EmailFormat, Gender, GenerationSpec, LastNameCount, ListOption, Locale,
};
