// Excel's `_xHHHH_` escapes for characters XML 1.0 cannot carry, decoded as IronCalc does.
pub(crate) fn decode_xlsx_escapes(s: &str) -> String {
    if !s.contains("_x") {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut result = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 6 < bytes.len()
            && bytes[i] == b'_'
            && bytes[i + 1] == b'x'
            && bytes[i + 6] == b'_'
            && let Some(hex) = s.get(i + 2..i + 6)
            && hex.chars().all(|c| c.is_ascii_hexdigit())
            && let Some(c) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        {
            result.push(c);
            i += 7;
            continue;
        }
        match s.get(i..).and_then(|rest| rest.chars().next()) {
            Some(c) => {
                result.push(c);
                i += c.len_utf8();
            }
            None => break,
        }
    }
    result
}

// An xsd:boolean attribute read as IronCalc reads it: trimmed, case-insensitive, and false
// when absent or unrecognised.
pub(crate) fn parse_bool_false(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        Some(value) => value == "1" || value.eq_ignore_ascii_case("true"),
        None => false,
    }
}
