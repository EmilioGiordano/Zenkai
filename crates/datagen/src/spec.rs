use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::date::Date;
use crate::limits::{
    MAX_DOMAIN_CHARS, MAX_DOMAINS, MAX_HEADER_CHARS, MAX_OPTION_CHARS, MAX_OPTIONS,
    MAX_PATTERN_CHARS,
};
use crate::percent::Percent;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    description = "Recipe for fake table rows. The same spec always produces the same rows."
)]
pub struct GenerationSpec {
    #[schemars(description = "Number of data rows to generate, headers not included.")]
    pub rows: u32,
    #[schemars(description = "Language and country of names, cities, streets and companies.")]
    pub locale: Locale,
    #[schemars(description = "Seed of the random sequence; change it for different rows.")]
    pub seed: u64,
    #[schemars(description = "Columns in sheet order, left to right.")]
    pub columns: Vec<ColumnSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Locale {
    #[serde(rename = "es-AR")]
    SpanishArgentina,
    #[serde(rename = "en-US")]
    EnglishUnitedStates,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ColumnSpec {
    #[schemars(description = "Header text. Email columns refer to name columns by it.")]
    #[schemars(length(max = MAX_HEADER_CHARS))]
    pub header: String,
    pub kind: ColumnKind,
    #[serde(default)]
    #[schemars(description = "Share of rows left empty, chosen at random.")]
    pub blanks: Percent,
    // A plain flag: it mirrors the "unique" checkbox and reads naturally in the JSON spec.
    #[serde(default)]
    #[schemars(description = "Every non-empty cell differs. Fails when too few values exist.")]
    pub unique: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(description = "What a column holds, with the options of that kind.")]
pub enum ColumnKind {
    #[schemars(description = "Given name, such as Lucía.")]
    FirstName {
        #[serde(default)]
        gender: Gender,
    },
    #[schemars(description = "Family name, such as Fernández or Gómez Ruiz.")]
    LastName {
        #[serde(default)]
        count: LastNameCount,
    },
    #[schemars(description = "Given name followed by last names.")]
    FullName {
        #[serde(default)]
        gender: Gender,
        #[serde(default)]
        last_names: LastNameCount,
    },
    #[schemars(
        description = "Email built from the name columns of the same row, lowercased, without accents or spaces. A missing or empty source gets a made-up name."
    )]
    Email {
        format: EmailFormat,
        #[serde(default)]
        #[schemars(
            description = "Header of a first_name or full_name column that gives the first name."
        )]
        first_name_from: Option<String>,
        #[serde(default)]
        #[schemars(
            description = "Header of a last_name or full_name column that gives the last name."
        )]
        last_name_from: Option<String>,
        #[schemars(description = "Domains chosen at random, such as gmail.com.")]
        #[schemars(length(min = 1, max = MAX_DOMAINS), inner(length(max = MAX_DOMAIN_CHARS)))]
        domains: Vec<String>,
    },
    #[schemars(description = "Phone number. In the pattern # is a digit; \\ escapes.")]
    Phone {
        #[schemars(example = "+54 9 11 ####-####")]
        #[schemars(length(min = 1, max = MAX_PATTERN_CHARS))]
        pattern: String,
    },
    #[schemars(description = "Street and number.")]
    StreetAddress {},
    City {},
    #[schemars(description = "Made-up company name with a legal suffix.")]
    Company {},
    #[schemars(description = "Whole number between min and max, both included.")]
    Integer {
        min: i64,
        max: i64,
    },
    #[schemars(description = "Number between min and max with a fixed count of decimal places.")]
    Decimal {
        min: f64,
        max: f64,
        places: u8,
    },
    #[schemars(description = "Date between from and to, both included.")]
    Date {
        from: Date,
        to: Date,
    },
    #[schemars(description = "TRUE or FALSE.")]
    Boolean {},
    #[schemars(description = "One of the listed values, optionally weighted.")]
    OneOf {
        #[schemars(length(min = 1, max = MAX_OPTIONS))]
        options: Vec<ListOption>,
    },
    #[schemars(description = "start, start + step, start + 2 * step, ... by row.")]
    SequentialId {
        #[serde(default = "one_i64")]
        start: i64,
        #[serde(default = "one_i64")]
        step: i64,
    },
    #[schemars(description = "Random version 4 UUID.")]
    Uuid {},
    #[schemars(description = "Lorem ipsum sentence with a word count in the range.")]
    Lorem {
        min_words: u16,
        max_words: u16,
    },
    #[schemars(
        description = "Code from a pattern: A is a capital letter, # a digit, \\ escapes, anything else is kept."
    )]
    Pattern {
        #[schemars(example = "AAA-####")]
        #[schemars(length(min = 1, max = MAX_PATTERN_CHARS))]
        pattern: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Gender {
    #[default]
    Any,
    Female,
    Male,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LastNameCount {
    #[default]
    One,
    Two,
    #[schemars(description = "Mostly one, sometimes two.")]
    OneOrTwo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EmailFormat {
    #[schemars(description = "lucia.fernandez")]
    FirstDotLast,
    #[schemars(description = "luciafernandez")]
    FirstLast,
    #[schemars(description = "lfernandez")]
    InitialLast,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListOption {
    #[schemars(length(min = 1, max = MAX_OPTION_CHARS))]
    pub value: String,
    #[serde(default = "one_u32")]
    #[schemars(description = "Relative frequency; 2 appears twice as often as 1.")]
    pub weight: u32,
}

fn one_i64() -> i64 {
    1
}

fn one_u32() -> u32 {
    1
}
