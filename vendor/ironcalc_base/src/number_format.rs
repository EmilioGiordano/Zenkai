use crate::{
    formatter::{self, format::Formatted},
    locale::get_locale,
    types::NumFmt,
};

// The standard built-in number formats defined by ECMA-376 (§18.8.30), indexed by `numFmtId`
// so that `DEFAULT_NUM_FMTS[id]` is the format code for that built-in id. Ids 23–36 are
// reserved/undefined by the spec and resolve to "general". A workbook-defined `numFmt` always
// takes precedence over this table (see `get_num_fmt`), so this is only consulted for a
// referenced-but-undefined built-in id.
const DEFAULT_NUM_FMTS: &[&str] = &[
    "general",                                                              // 0
    "0",                                                                    // 1
    "0.00",                                                                 // 2
    "#,##0",                                                                // 3
    "#,##0.00",                                                             // 4
    "$#,##0_);($#,##0)",                                                    // 5
    "$#,##0_);[Red]($#,##0)",                                               // 6
    "$#,##0.00_);($#,##0.00)",                                              // 7
    "$#,##0.00_);[Red]($#,##0.00)",                                         // 8
    "0%",                                                                   // 9
    "0.00%",                                                                // 10
    "0.00E+00",                                                             // 11
    "# ?/?",                                                                // 12
    "# ??/??",                                                              // 13
    "mm-dd-yy",                                                             // 14
    "d-mmm-yy",                                                             // 15
    "d-mmm",                                                                // 16
    "mmm-yy",                                                               // 17
    "h:mm AM/PM",                                                           // 18
    "h:mm:ss AM/PM",                                                        // 19
    "h:mm",                                                                 // 20
    "h:mm:ss",                                                              // 21
    "m/d/yy h:mm",                                                          // 22
    "general",                                                              // 23 (reserved)
    "general",                                                              // 24 (reserved)
    "general",                                                              // 25 (reserved)
    "general",                                                              // 26 (reserved)
    "general",                                                              // 27 (reserved)
    "general",                                                              // 28 (reserved)
    "general",                                                              // 29 (reserved)
    "general",                                                              // 30 (reserved)
    "general",                                                              // 31 (reserved)
    "general",                                                              // 32 (reserved)
    "general",                                                              // 33 (reserved)
    "general",                                                              // 34 (reserved)
    "general",                                                              // 35 (reserved)
    "general",                                                              // 36 (reserved)
    "#,##0_);(#,##0)",                                                      // 37
    "#,##0_);[Red](#,##0)",                                                 // 38
    "#,##0.00_);(#,##0.00)",                                                // 39
    "#,##0.00_);[Red](#,##0.00)",                                           // 40
    "_(* #,##0_);_(* \\(#,##0\\);_(* \"-\"_);_(@_)",                        // 41
    "_(\"$\"* #,##0_);_(\"$\"* \\(#,##0\\);_(\"$\"* \"-\"_);_(@_)",         // 42
    "_(* #,##0.00_);_(* \\(#,##0.00\\);_(* \"-\"??_);_(@_)",                // 43
    "_(\"$\"* #,##0.00_);_(\"$\"* \\(#,##0.00\\);_(\"$\"* \"-\"??_);_(@_)", // 44
    "mm:ss",                                                                // 45
    "[h]:mm:ss",                                                            // 46
    "mmss.0",                                                               // 47
    "##0.0E+0",                                                             // 48
    "@",                                                                    // 49
];

pub fn get_default_num_fmt_id(num_fmt: &str) -> Option<i32> {
    for (index, default_num_fmt) in DEFAULT_NUM_FMTS.iter().enumerate() {
        if default_num_fmt == &num_fmt {
            return Some(index as i32);
        };
    }
    None
}

pub fn get_num_fmt(num_fmt_id: i32, num_fmts: &[NumFmt]) -> String {
    // Check if it defined
    for num_fmt in num_fmts {
        if num_fmt.num_fmt_id == num_fmt_id {
            return num_fmt.format_code.clone();
        }
    }
    // Return one of the default ones
    if num_fmt_id < DEFAULT_NUM_FMTS.len() as i32 {
        return DEFAULT_NUM_FMTS[num_fmt_id as usize].to_string();
    }
    // Return general
    DEFAULT_NUM_FMTS[0].to_string()
}

pub fn get_new_num_fmt_index(num_fmts: &[NumFmt]) -> i32 {
    let mut index = DEFAULT_NUM_FMTS.len() as i32;
    let mut found = true;
    while found {
        found = false;
        for num_fmt in num_fmts {
            if num_fmt.num_fmt_id == index {
                found = true;
                index += 1;
                break;
            }
        }
    }
    index
}

pub fn to_precision(value: f64, precision: usize) -> f64 {
    if value.is_infinite() || value.is_nan() || holds_precision(value, precision) {
        return value;
    }
    to_precision_str(value, precision)
        .parse::<f64>()
        .unwrap_or({
            // TODO: do this in a way that does not require a possible error
            0.0
        })
}

/// It rounds a `f64` with `p` significant figures:
/// ```
///     use ironcalc_base::number_format;
///     assert_eq!(number_format::to_precision(0.1+0.2, 15), 0.3);
///     assert_eq!(number_format::to_excel_precision_str(0.1+0.2), "0.3");
/// ```
/// This intends to be equivalent to the js: `${parseFloat(value.toPrecision(precision)})`
/// See ([ecma](https://tc39.es/ecma262/#sec-number.prototype.toprecision)).
pub fn to_excel_precision_str(value: f64) -> String {
    to_precision_str(value, 15)
}

pub fn to_excel_precision(value: f64, precision: usize) -> f64 {
    if !value.is_finite() || holds_precision(value, precision) {
        return value;
    }
    round_through_text(value, precision)
}

fn round_through_text(value: f64, precision: usize) -> f64 {
    let s = format!("{:.*e}", precision.saturating_sub(1), value);
    s.parse::<f64>().unwrap_or(value)
}

// Whether rounding a finite value to `precision` significant digits gives it back, without
// formatting it. True when its shortest round-trip decimal has at most that many digits and
// at most 15: half an ulp of a normal double is below half the gap between 15-digit
// decimals, so that decimal is the nearest one and it parses back to the value.
fn holds_precision(value: f64, precision: usize) -> bool {
    if value == 0.0 {
        return true;
    }
    if precision > 15 || value.abs() < f64::MIN_POSITIVE {
        return false;
    }
    let mut buffer = ryu::Buffer::new();
    let text = buffer.format_finite(value);
    let mantissa = text.split(['e', 'E']).next().unwrap_or(text);
    let mut first_nonzero = None;
    let mut last_nonzero = 0;
    for (position, digit) in mantissa.bytes().filter(u8::is_ascii_digit).enumerate() {
        if digit != b'0' {
            first_nonzero.get_or_insert(position);
            last_nonzero = position;
        }
    }
    first_nonzero.is_none_or(|first| last_nonzero - first < precision)
}

pub fn to_precision_str(value: f64, precision: usize) -> String {
    if !value.is_finite() {
        if value.is_infinite() {
            return "inf".to_string();
        } else {
            return "NaN".to_string();
        }
    }

    let s = format!("{:.*e}", precision.saturating_sub(1), value);
    let parsed = s.parse::<f64>().unwrap_or(value);

    // I would love to use the std library. There is not a speed concern here
    // problem is it doesn't do the right thing
    // Also ryu is my favorite _modern_ algorithm
    let mut buffer = ryu::Buffer::new();
    let text = buffer.format(parsed);
    // The above algorithm converts 2 to 2.0 regrettably
    if let Some(stripped) = text.strip_suffix(".0") {
        return stripped.to_string();
    }
    text.to_string()
}

pub fn format_number(value: f64, format_code: &str, locale: &str) -> Formatted {
    let locale = match get_locale(locale) {
        Ok(l) => l,
        Err(_) => {
            return Formatted {
                text: "#ERROR!".to_owned(),
                color: None,
                error: Some("Invalid locale".to_string()),
            }
        }
    };
    formatter::format::format_number(value, format_code, locale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn sample(state: &mut u64) -> f64 {
        let pick = next(state) % 6;
        let sign = if next(state) % 2 == 0 { 1.0 } else { -1.0 };
        let decimal = |state: &mut u64| {
            let digits = next(state) % 18;
            let mantissa = (next(state) % 10u64.pow(digits as u32 + 1)) as f64;
            mantissa / 10f64.powi((next(state) % 40) as i32 - 20)
        };
        match pick {
            0 => f64::from_bits(next(state)),
            1 => sign * decimal(state),
            2 => sign * (decimal(state) + decimal(state)),
            3 => sign * decimal(state) * decimal(state) / (decimal(state) + 1.0),
            4 => {
                let power = 10f64.powi((next(state) % 40) as i32 - 20);
                let ulps = (next(state) % 7) as i64 - 3;
                sign * f64::from_bits((power.to_bits() as i64 + ulps) as u64)
            }
            _ => sign * f64::from_bits(next(state) % (1u64 << 53)),
        }
    }

    #[test]
    fn rounding_shortcut_matches_rounding_through_text() {
        let mut state = 0x9E37_79B9_7F4A_7C15;
        let mut checked = 0;
        while checked < 60_000 {
            let value = sample(&mut state);
            if !value.is_finite() {
                continue;
            }
            for precision in 0..=17 {
                assert_eq!(
                    to_excel_precision(value, precision).to_bits(),
                    round_through_text(value, precision).to_bits(),
                    "to_excel_precision({value:e}, {precision})"
                );
                let through_text = to_precision_str(value, precision)
                    .parse::<f64>()
                    .unwrap_or(0.0);
                assert_eq!(
                    to_precision(value, precision).to_bits(),
                    through_text.to_bits(),
                    "to_precision({value:e}, {precision})"
                );
            }
            checked += 1;
        }
    }

    #[test]
    fn rounding_shortcut_keeps_exact_values() {
        for value in [
            0.0,
            -0.0,
            0.3,
            1e15,
            999_999_999_999_999.0,
            0.1 + 0.2,
            5e-324,
        ] {
            assert_eq!(
                to_excel_precision(value, 15).to_bits(),
                round_through_text(value, 15).to_bits()
            );
        }
        assert_eq!(to_excel_precision(0.1 + 0.2, 15), 0.3);
        assert_eq!(
            to_excel_precision(1_234_567_890_123_456.0, 15),
            1_234_567_890_123_460.0
        );
    }

    // The default table is indexed by `numFmtId`: `get_num_fmt(id, &[])` must return the
    // ECMA-376 §18.8.30 built-in code for that id.
    #[test]
    fn builtin_ids_map_to_the_ecma376_codes() {
        assert_eq!(get_num_fmt(0, &[]), "general");
        assert_eq!(get_num_fmt(5, &[]), "$#,##0_);($#,##0)");
        assert_eq!(get_num_fmt(8, &[]), "$#,##0.00_);[Red]($#,##0.00)");
        assert_eq!(get_num_fmt(11, &[]), "0.00E+00");
        assert_eq!(get_num_fmt(37, &[]), "#,##0_);(#,##0)");
        // id 39 is the one that produced `#VALUE!` in the wild (was "t0.00").
        assert_eq!(get_num_fmt(39, &[]), "#,##0.00_);(#,##0.00)");
        assert_eq!(
            get_num_fmt(44, &[]),
            "_(\"$\"* #,##0.00_);_(\"$\"* \\(#,##0.00\\);_(\"$\"* \"-\"??_);_(@_)"
        );
        assert_eq!(get_num_fmt(49, &[]), "@");
        // Reserved ids 23–36 resolve to "general".
        assert_eq!(get_num_fmt(25, &[]), "general");
        // Out-of-range falls back to "general" too.
        assert_eq!(get_num_fmt(500, &[]), "general");
    }

    // A workbook-defined `numFmt` still wins over the default table.
    #[test]
    fn workbook_defined_num_fmt_takes_precedence() {
        let defined = vec![NumFmt {
            num_fmt_id: 39,
            format_code: "FILE-OWN".to_string(),
        }];
        assert_eq!(get_num_fmt(39, &defined), "FILE-OWN");
    }

    // Every corrected built-in code must actually be formattable by IronCalc's own formatter
    // (a valid number must NOT come back as an error). This is what was broken: the old garbage
    // codes (e.g. "t0.00") made the formatter fail on valid numbers.
    //
    // Exception: id 47 (`mmss.0`, elapsed minutes:seconds.tenths) is the correct ECMA-376 code
    // but IronCalc's *formatter* does not yet parse that pattern — a separate formatter gap, not
    // a table bug (the old code at id 47 didn't render either). Kept correct in the table so a
    // future formatter fix makes it work; excluded from this render smoke-check.
    #[test]
    fn corrected_builtins_format_without_error() {
        let mut failures = Vec::new();
        for id in [
            5, 6, 7, 8, 11, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 48, 49,
        ] {
            let code = get_num_fmt(id, &[]);
            let out = format_number(1234.5, &code, "en");
            if out.error.is_some() || out.text.contains("#VALUE!") || out.text.contains("#ERROR!") {
                failures.push((id, code, out.text, out.error));
            }
        }
        assert!(
            failures.is_empty(),
            "codes the formatter rejected: {failures:?}"
        );
    }

    // The specific regression: id 39 formats 175000 as a grouped 2-decimal number, not `#VALUE!`.
    #[test]
    fn id_39_formats_currency_value() {
        let out = format_number(175000.0, &get_num_fmt(39, &[]), "en");
        assert!(out.error.is_none());
        assert!(out.text.contains("175,000.00"), "got {:?}", out.text);
    }
}
