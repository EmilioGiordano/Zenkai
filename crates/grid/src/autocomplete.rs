// Excel's Formula AutoComplete: while typing a function name in a formula, the matching
// functions are listed and Tab inserts the chosen one with its opening parenthesis.

const MAX_SHOWN: usize = 8;

// The engine keeps its function table private, so the commonly used Excel functions it
// supports are listed here.
const FUNCTIONS: &[&str] = &[
    "ABS",
    "ACOS",
    "AND",
    "ASIN",
    "ATAN",
    "ATAN2",
    "AVERAGE",
    "AVERAGEA",
    "AVERAGEIF",
    "AVERAGEIFS",
    "CEILING",
    "CHAR",
    "CHOOSE",
    "CLEAN",
    "CODE",
    "COLUMN",
    "COLUMNS",
    "CONCAT",
    "CONCATENATE",
    "COS",
    "COUNT",
    "COUNTA",
    "COUNTBLANK",
    "COUNTIF",
    "COUNTIFS",
    "DATE",
    "DATEDIF",
    "DATEVALUE",
    "DAY",
    "DAYS",
    "EDATE",
    "EOMONTH",
    "EXACT",
    "EXP",
    "FALSE",
    "FIND",
    "FLOOR",
    "FV",
    "HLOOKUP",
    "HOUR",
    "IF",
    "IFERROR",
    "IFNA",
    "IFS",
    "INDEX",
    "INDIRECT",
    "INT",
    "IPMT",
    "IRR",
    "ISBLANK",
    "ISERROR",
    "ISEVEN",
    "ISNA",
    "ISNUMBER",
    "ISODD",
    "ISTEXT",
    "LARGE",
    "LEFT",
    "LEN",
    "LN",
    "LOG",
    "LOG10",
    "LOOKUP",
    "LOWER",
    "MATCH",
    "MAX",
    "MAXIFS",
    "MEDIAN",
    "MID",
    "MIN",
    "MINIFS",
    "MINUTE",
    "MOD",
    "MONTH",
    "NETWORKDAYS",
    "NOT",
    "NOW",
    "NPER",
    "NPV",
    "OFFSET",
    "OR",
    "PI",
    "PMT",
    "POWER",
    "PPMT",
    "PRODUCT",
    "PROPER",
    "PV",
    "RAND",
    "RANDBETWEEN",
    "RANK",
    "RATE",
    "REPLACE",
    "REPT",
    "RIGHT",
    "ROUND",
    "ROUNDDOWN",
    "ROUNDUP",
    "ROW",
    "ROWS",
    "SEARCH",
    "SECOND",
    "SIGN",
    "SIN",
    "SMALL",
    "SQRT",
    "STDEV",
    "STDEV.P",
    "STDEV.S",
    "SUBSTITUTE",
    "SUBTOTAL",
    "SUM",
    "SUMIF",
    "SUMIFS",
    "SUMPRODUCT",
    "SWITCH",
    "TAN",
    "TEXT",
    "TEXTJOIN",
    "TIME",
    "TODAY",
    "TRIM",
    "TRUE",
    "TRUNC",
    "UPPER",
    "VALUE",
    "VAR",
    "VAR.P",
    "VAR.S",
    "VLOOKUP",
    "WEEKDAY",
    "WEEKNUM",
    "WORKDAY",
    "XLOOKUP",
    "XOR",
    "YEAR",
];

/// The function name being typed just before `caret`, with its byte offset.
pub fn token_at(text: &str, caret: usize) -> Option<(usize, &str)> {
    if !text.starts_with('=') || caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let before = &text[..caret];
    let start = before
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_'))
        .map_or(0, |at| at + 1);
    let token = &before[start..];
    let opener = before[..start].chars().last()?;
    let in_text = before[..start].matches('"').count() % 2 == 1;
    let starts_name = token
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic());
    (starts_name && !in_text && "=(,;+-*/^&<> :".contains(opener)).then_some((start, token))
}

/// Functions starting with `prefix`, ignoring case; an exact name alone is not offered.
pub fn suggestions(prefix: &str) -> Vec<&'static str> {
    let upper = prefix.to_ascii_uppercase();
    let found: Vec<&'static str> = FUNCTIONS
        .iter()
        .copied()
        .filter(|name| name.starts_with(&upper))
        .take(MAX_SHOWN)
        .collect();
    if found == [upper.as_str()] {
        return Vec::new();
    }
    found
}

#[cfg(test)]
mod tests {
    use super::{suggestions, token_at};

    #[test]
    fn finds_the_name_being_typed() {
        assert_eq!(token_at("=SU", 3), Some((1, "SU")));
        assert_eq!(token_at("=A1+vlo", 7), Some((4, "vlo")));
        assert_eq!(token_at("=IF(su", 6), Some((4, "su")));
        assert_eq!(token_at("SU", 2), None);
        assert_eq!(token_at("=\"su", 4), None);
        assert_eq!(token_at("=A1", 3), Some((1, "A1")));
        assert_eq!(token_at("=1", 2), None);
    }

    #[test]
    fn suggests_by_prefix() {
        assert_eq!(&suggestions("sumi")[..], ["SUMIF", "SUMIFS"]);
        assert!(suggestions("A1").is_empty());
        assert!(suggestions("SUM").contains(&"SUMPRODUCT"));
        assert!(suggestions("TODAY").is_empty());
    }
}
