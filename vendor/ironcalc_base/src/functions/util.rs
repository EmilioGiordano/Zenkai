#[cfg(target_arch = "wasm32")]
use regex_lite as regex;

use std::cmp::Ordering;
use std::sync::OnceLock;

use crate::{
    calc_result::CalcResult,
    expressions::token::{is_english_error_string, Error},
    formatter::format::parse_date,
    locale::Locale,
    number_format::to_excel_precision,
};

/// If `s` looks like a date literal in the given locale, return its Excel
/// serial number as `f64`. Pure numeric strings are rejected here because the
/// numeric branch in `build_criteria` handles them already, and date parsing
/// must not shadow the simpler number path.
fn parse_date_criterion(s: &str, locale: &Locale) -> Option<f64> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.parse::<f64>().is_ok() {
        return None;
    }
    parse_date(trimmed, locale)
        .ok()
        .map(|(serial, _)| serial as f64)
}

fn error_sort_rank(e: &Error) -> u8 {
    match e {
        Error::NULL => 1,
        Error::DIV => 2,
        Error::VALUE => 3,
        Error::REF => 4,
        Error::NAME => 5,
        Error::NUM => 6,
        Error::NA => 7,
        _ => 8,
    }
}

/// This test for exact match (modulo case).
///   * strings are not cast into bools or numbers
///   * empty cell is not cast into empty string or zero
pub(crate) fn values_are_equal(left: &CalcResult, right: &CalcResult) -> bool {
    match (left, right) {
        (CalcResult::Number(value1), CalcResult::Number(value2)) => {
            if (value2 - value1).abs() < f64::EPSILON {
                return true;
            }
            false
        }
        (CalcResult::String(value1), CalcResult::String(value2)) => {
            let value1 = value1.to_uppercase();
            let value2 = value2.to_uppercase();
            value1 == value2
        }
        (CalcResult::Boolean(value1), CalcResult::Boolean(value2)) => value1 == value2,
        (CalcResult::EmptyCell, CalcResult::EmptyCell) => true,
        // NOTE: Errors and Ranges are not covered
        (_, _) => false,
    }
}

// In Excel there are two ways of comparing cell values.
// The old school comparison valid in formulas like D3 < D4 or HLOOKUP,... cast empty cells into empty strings or 0
// For the new formulas like XLOOKUP or SORT an empty cell is always larger than anything else.

// ..., -2, -1, 0, 1, 2, ..., A-Z, FALSE, TRUE;
pub(crate) fn compare_values(left: &CalcResult, right: &CalcResult) -> i32 {
    match (left, right) {
        (CalcResult::Number(value1), CalcResult::Number(value2)) => {
            let value1 = to_excel_precision(*value1, 15);
            let value2 = to_excel_precision(*value2, 15);
            if (value2 - value1).abs() < f64::EPSILON {
                return 0;
            }
            if value1 < value2 {
                return -1;
            }
            1
        }
        (CalcResult::Number(_value1), CalcResult::String(_value2)) => -1,
        (CalcResult::Number(_value1), CalcResult::Boolean(_value2)) => -1,
        (CalcResult::String(value1), CalcResult::String(value2)) => {
            let value1 = value1.to_uppercase();
            let value2 = value2.to_uppercase();
            match value1.cmp(&value2) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }
        }
        (CalcResult::String(_value1), CalcResult::Boolean(_value2)) => -1,
        (CalcResult::Boolean(value1), CalcResult::Boolean(value2)) => {
            if value1 == value2 {
                return 0;
            }
            if *value1 {
                return 1;
            }
            -1
        }
        (CalcResult::EmptyCell, CalcResult::String(_value2)) => {
            compare_values(&CalcResult::String("".to_string()), right)
        }
        (CalcResult::String(_value1), CalcResult::EmptyCell) => {
            compare_values(left, &CalcResult::String("".to_string()))
        }
        (CalcResult::EmptyCell, CalcResult::Number(_value2)) => {
            compare_values(&CalcResult::Number(0.0), right)
        }
        (CalcResult::Number(_value1), CalcResult::EmptyCell) => {
            compare_values(left, &CalcResult::Number(0.0))
        }
        (CalcResult::EmptyCell, CalcResult::Boolean(_)) => {
            compare_values(&CalcResult::Boolean(false), right)
        }
        (CalcResult::Boolean(_), CalcResult::EmptyCell) => {
            compare_values(left, &CalcResult::Boolean(false))
        }
        (CalcResult::EmptyCell, CalcResult::EmptyCell) => 0,
        // Errors sort after everything else, ordered by Excel's canonical rank
        (CalcResult::Error { error: e1, .. }, CalcResult::Error { error: e2, .. }) => {
            let r1 = error_sort_rank(e1);
            let r2 = error_sort_rank(e2);
            if r1 < r2 {
                -1
            } else if r1 > r2 {
                1
            } else {
                0
            }
        }
        (CalcResult::Error { .. }, _) => 1,
        (_, CalcResult::Error { .. }) => -1,
        (_, _) => 1,
    }
}

/// We convert an Excel wildcard into a Rust (Perl family) regex
pub(crate) fn from_wildcard_to_regex(
    wildcard: &str,
    exact: bool,
) -> Result<regex::Regex, regex::Error> {
    regex::Regex::new(&wildcard_to_pattern(wildcard, exact))
}

fn wildcard_to_pattern(wildcard: &str, exact: bool) -> String {
    // 1. Escape all
    let reg = &regex::escape(wildcard);

    // 2. We convert the escaped '?' into '.' (matches a single character)
    let reg = &reg.replace("\\?", ".");
    // 3. We convert the escaped '*' into '.*' (matches anything)
    let reg = &reg.replace("\\*", ".*");

    // 4. We send '\\~\\~' to '??' that is an unescaped regular expression, therefore cannot be in reg
    let reg = &reg.replace("\\~\\~", "??");

    // 5. If the escaped and converted '*' is preceded by '~' then it's a raw '*'
    let reg = &reg.replace("\\~.*", "\\*");
    // 6. If the escaped and converted '.' is preceded by '~' then it's a raw '?'
    let reg = &reg.replace("\\~.", "\\?");
    // '~' is used in Excel to escape any other character.
    //    So ~x goes to x (whatever x is)
    // 7. Remove all the others '\\~d' --> 'd'
    let reg = &reg.replace("\\~", "");
    // 8. Put back the '\\~\\~'  as '\\~'
    let reg = &reg.replace("??", "\\~");

    // And we have a valid Perl regex! (As Kim Kardashian said before me: "I know, right?")
    if exact {
        return format!("^{reg}$");
    }
    reg.to_string()
}

// NUMBERS ///
//*********///

// It could be either the number or a string representation of the number
// In the rest of the cases calc_result needs to be a number (cannot be the string "23", for instance)
fn result_is_equal_to_number(calc_result: &CalcResult, target: f64) -> bool {
    match calc_result {
        CalcResult::Number(f) => {
            if (f - target).abs() < f64::EPSILON {
                return true;
            }
            false
        }
        CalcResult::String(s) => {
            if let Ok(f) = s.parse::<f64>() {
                if (f - target).abs() < f64::EPSILON {
                    return true;
                }
                return false;
            }
            false
        }
        _ => false,
    }
}

fn result_is_less_than_number(calc_result: &CalcResult, target: f64) -> bool {
    match calc_result {
        CalcResult::Number(f) => *f < target,
        _ => false,
    }
}

fn result_is_less_or_equal_than_number(calc_result: &CalcResult, target: f64) -> bool {
    match calc_result {
        CalcResult::Number(f) => *f <= target,
        _ => false,
    }
}

fn result_is_greater_than_number(calc_result: &CalcResult, target: f64) -> bool {
    match calc_result {
        CalcResult::Number(f) => *f > target,
        _ => false,
    }
}

fn result_is_greater_or_equal_than_number(calc_result: &CalcResult, target: f64) -> bool {
    match calc_result {
        CalcResult::Number(f) => *f >= target,
        _ => false,
    }
}

fn result_is_not_equal_to_number(calc_result: &CalcResult, target: f64) -> bool {
    match calc_result {
        CalcResult::Number(f) => {
            if (f - target).abs() > f64::EPSILON {
                return true;
            }
            false
        }
        _ => true,
    }
}

// BOOLEANS ///
//**********///

// Booleans have to be "exactly" equal
fn result_is_equal_to_bool(calc_result: &CalcResult, target: bool) -> bool {
    match calc_result {
        CalcResult::Boolean(f) => target == *f,
        _ => false,
    }
}

fn result_is_not_equal_to_bool(calc_result: &CalcResult, target: bool) -> bool {
    match calc_result {
        CalcResult::Boolean(f) => target != *f,
        _ => true,
    }
}

// STRINGS ///
//*********///

// Note that strings are case insensitive. `target` must always be lower case.

pub(crate) fn result_matches_regex(calc_result: &CalcResult, reg: &regex::Regex) -> bool {
    match calc_result {
        CalcResult::String(s) => reg.is_match(&s.to_lowercase()),
        _ => false,
    }
}

// The same as `value.to_lowercase().as_str().cmp(target)`, without allocating per cell.
fn cmp_lowercase(value: &str, target: &str) -> Ordering {
    if value.is_ascii() {
        value
            .bytes()
            .map(|byte| byte.to_ascii_lowercase())
            .cmp(target.bytes())
    } else if value.contains('\u{3A3}') {
        // `to_lowercase` turns a word-final capital sigma into 'ς', which a
        // character-by-character mapping cannot know.
        value.to_lowercase().as_str().cmp(target)
    } else {
        value
            .chars()
            .flat_map(char::to_lowercase)
            .cmp(target.chars())
    }
}

fn result_is_equal_to_string(calc_result: &CalcResult, target: &str) -> bool {
    match calc_result {
        CalcResult::String(s) => cmp_lowercase(s, target) == Ordering::Equal,
        CalcResult::EmptyCell => target.is_empty(),
        _ => false,
    }
}

fn result_is_not_equal_to_string(calc_result: &CalcResult, target: &str) -> bool {
    match calc_result {
        CalcResult::String(s) => cmp_lowercase(s, target) != Ordering::Equal,
        _ => true,
    }
}

fn result_is_less_than_string(calc_result: &CalcResult, target: &str) -> bool {
    match calc_result {
        CalcResult::String(s) => cmp_lowercase(s, target) == Ordering::Less,
        _ => false,
    }
}

fn result_is_less_or_equal_than_string(calc_result: &CalcResult, target: &str) -> bool {
    match calc_result {
        CalcResult::String(s) => cmp_lowercase(s, target) != Ordering::Greater,
        _ => false,
    }
}

fn result_is_greater_than_string(calc_result: &CalcResult, target: &str) -> bool {
    match calc_result {
        CalcResult::String(s) => cmp_lowercase(s, target) == Ordering::Greater,
        _ => false,
    }
}

fn result_is_greater_or_equal_than_string(calc_result: &CalcResult, target: &str) -> bool {
    match calc_result {
        CalcResult::String(s) => cmp_lowercase(s, target) != Ordering::Less,
        _ => false,
    }
}

// Matches text against a wildcard criterion as `result_matches_regex` does. For ASCII text
// and an ASCII criterion, ignoring case gives the same answer without lowercasing a copy.
// Each regex is compiled on first need, so a criterion usually compiles one. A pattern
// that cannot compile (past the regex size limit) matches nothing, as before.
struct Wildcard {
    lowercase_pattern: String,
    ascii_criterion: bool,
    lowercase: OnceLock<Option<regex::Regex>>,
    ascii_any_case: OnceLock<Option<regex::Regex>>,
}

impl Wildcard {
    fn new(criterion: &str) -> Wildcard {
        Wildcard {
            lowercase_pattern: wildcard_to_pattern(&criterion.to_lowercase(), true),
            ascii_criterion: criterion.is_ascii(),
            lowercase: OnceLock::new(),
            ascii_any_case: OnceLock::new(),
        }
    }

    fn matches(&self, calc_result: &CalcResult) -> Option<bool> {
        match calc_result {
            CalcResult::String(s) if s.is_ascii() && self.ascii_criterion => self
                .ascii_any_case
                .get_or_init(|| regex::Regex::new(&format!("(?i){}", self.lowercase_pattern)).ok())
                .as_ref()
                .map(|any_case| any_case.is_match(s)),
            _ => self
                .lowercase
                .get_or_init(|| regex::Regex::new(&self.lowercase_pattern).ok())
                .as_ref()
                .map(|lowercase| result_matches_regex(calc_result, lowercase)),
        }
    }
}

// ERRORS ///
//********///

fn result_is_equal_to_error(calc_result: &CalcResult, target: &str) -> bool {
    match calc_result {
        CalcResult::Error { error, .. } => target == error.to_string(),
        _ => false,
    }
}

fn result_is_not_equal_to_error(calc_result: &CalcResult, target: &str) -> bool {
    match calc_result {
        CalcResult::Error { error, .. } => target != error.to_string(),
        _ => true,
    }
}

// EMPTY ///
//*******///

// Note that these two are not inverse of each other.
// In particular, you can never match an empty cell.

fn result_is_not_equal_to_empty(calc_result: &CalcResult) -> bool {
    !matches!(calc_result, CalcResult::EmptyCell)
}

fn result_is_equal_to_empty(calc_result: &CalcResult) -> bool {
    match calc_result {
        CalcResult::Number(f) => (f - 0.0).abs() < f64::EPSILON,
        _ => false,
    }
}

/// This returns a function (closure) of signature fn(&CalcResult) -> bool
/// It is Boxed because it returns different closures, so the size cannot be known at compile time
/// The lifetime (a) of value has to be longer or equal to the lifetime of the returned closure
///
/// `locale` is used to recognise date literals in the criterion string (e.g.
/// "<7/31/2023" or "<31/7/2023" depending on the locale's short date format).
/// When the post-operator string is not parseable as a number, we try to parse
/// it as a date and, on success, fall back to numeric comparison against the
/// resulting Excel serial. This is what lets COUNTIF/SUMIF/AVERAGEIF (and the
/// *IFS variants) match date-serial cells with date-string criteria.
/// A compiled criterion predicate, as returned by `build_criteria`.
pub(crate) type Criterion<'c> = Box<dyn Fn(&CalcResult) -> bool + 'c>;

pub(crate) fn build_criteria<'a>(value: &'a CalcResult, locale: &'a Locale) -> Criterion<'a> {
    match value {
        CalcResult::String(s) => {
            if let Some(v) = s.strip_prefix("<=") {
                // TODO: I am not implementing <= ERROR or <= BOOLEAN
                if let Ok(f) = v.parse::<f64>() {
                    Box::new(move |x| result_is_less_or_equal_than_number(x, f))
                } else if v.is_empty() {
                    Box::new(move |_x| false)
                } else if let Some(f) = parse_date_criterion(v, locale) {
                    Box::new(move |x| result_is_less_or_equal_than_number(x, f))
                } else {
                    let target = v.to_lowercase();
                    Box::new(move |x| result_is_less_or_equal_than_string(x, &target))
                }
            } else if let Some(v) = s.strip_prefix(">=") {
                // TODO: I am not implementing >= ERROR or >= BOOLEAN
                if let Ok(f) = v.parse::<f64>() {
                    Box::new(move |x| result_is_greater_or_equal_than_number(x, f))
                } else if v.is_empty() {
                    Box::new(move |_x| false)
                } else if let Some(f) = parse_date_criterion(v, locale) {
                    Box::new(move |x| result_is_greater_or_equal_than_number(x, f))
                } else {
                    let target = v.to_lowercase();
                    Box::new(move |x| result_is_greater_or_equal_than_string(x, &target))
                }
            } else if let Some(v) = s.strip_prefix("<>") {
                if let Ok(f) = v.parse::<f64>() {
                    Box::new(move |x| result_is_not_equal_to_number(x, f))
                } else if let Ok(b) = v.to_lowercase().parse::<bool>() {
                    Box::new(move |x| result_is_not_equal_to_bool(x, b))
                } else if is_english_error_string(v) {
                    Box::new(move |x| result_is_not_equal_to_error(x, v))
                } else if v.contains('*') || v.contains('?') {
                    let wildcard = Wildcard::new(v);
                    Box::new(move |x| wildcard.matches(x) == Some(false))
                } else if v.is_empty() {
                    Box::new(result_is_not_equal_to_empty)
                } else if let Some(f) = parse_date_criterion(v, locale) {
                    Box::new(move |x| result_is_not_equal_to_number(x, f))
                } else {
                    let target = v.to_lowercase();
                    Box::new(move |x| result_is_not_equal_to_string(x, &target))
                }
            } else if let Some(v) = s.strip_prefix('<') {
                // TODO: I am not implementing < ERROR or < BOOLEAN
                if let Ok(f) = v.parse::<f64>() {
                    Box::new(move |x| result_is_less_than_number(x, f))
                } else if v.is_empty() {
                    Box::new(move |_x| false)
                } else if let Some(f) = parse_date_criterion(v, locale) {
                    Box::new(move |x| result_is_less_than_number(x, f))
                } else {
                    let target = v.to_lowercase();
                    Box::new(move |x| result_is_less_than_string(x, &target))
                }
            } else if let Some(v) = s.strip_prefix('>') {
                // TODO: I am not implementing > ERROR or > BOOLEAN
                if let Ok(f) = v.parse::<f64>() {
                    Box::new(move |x| result_is_greater_than_number(x, f))
                } else if v.is_empty() {
                    Box::new(move |_x| false)
                } else if let Some(f) = parse_date_criterion(v, locale) {
                    Box::new(move |x| result_is_greater_than_number(x, f))
                } else {
                    let target = v.to_lowercase();
                    Box::new(move |x| result_is_greater_than_string(x, &target))
                }
            } else {
                let v = if let Some(a) = s.strip_prefix('=') {
                    a
                } else {
                    s
                };
                if let Ok(f) = v.parse::<f64>() {
                    Box::new(move |x| result_is_equal_to_number(x, f))
                } else if let Ok(b) = v.to_lowercase().parse::<bool>() {
                    Box::new(move |x| result_is_equal_to_bool(x, b))
                } else if is_english_error_string(v) {
                    Box::new(move |x| result_is_equal_to_error(x, v))
                } else if v.contains('*') || v.contains('?') {
                    let wildcard = Wildcard::new(v);
                    Box::new(move |x| wildcard.matches(x) == Some(true))
                } else if let Some(f) = parse_date_criterion(v, locale) {
                    Box::new(move |x| result_is_equal_to_number(x, f))
                } else {
                    let target = v.to_lowercase();
                    Box::new(move |x| result_is_equal_to_string(x, &target))
                }
            }
        }
        CalcResult::Number(target) => Box::new(move |x| result_is_equal_to_number(x, *target)),
        CalcResult::Boolean(b) => Box::new(move |x| result_is_equal_to_bool(x, *b)),
        CalcResult::Error { error, .. } => {
            // An error will match an error (never a string that is an error)
            let target = error.to_string();
            Box::new(move |x| result_is_equal_to_error(x, &target))
        }
        CalcResult::Range { left: _, right: _ } => Box::new(move |_x| false),
        CalcResult::Array(_) | CalcResult::Lambda(_) => Box::new(move |_x| false),
        CalcResult::EmptyCell | CalcResult::EmptyArg => Box::new(result_is_equal_to_empty),
    }
}

#[cfg(test)]
mod tests {
    use super::cmp_lowercase;

    #[test]
    fn cmp_lowercase_matches_lowercasing_a_copy() {
        let values = [
            "",
            "apple",
            "APPLE",
            "b*n",
            "Zebra",
            "ÉCLAIR",
            "Straße",
            "İstanbul",
            "ΟΔΟΣ",
            "ΣΑΣ",
            "日本",
        ];
        for value in values {
            for target in values.map(str::to_lowercase) {
                assert_eq!(
                    cmp_lowercase(value, &target),
                    value.to_lowercase().as_str().cmp(target.as_str()),
                    "{value} against {target}"
                );
            }
        }
    }
}
