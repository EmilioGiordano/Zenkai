use super::write_offset;

// The first byte of a fill key; a lexer key starts with 0 or 1, so the two never meet.
const FILL: u8 = 2;
// Tags of the key, which a formula never holds: a cell reference with which of its row
// (bit 0) and column (bit 1) are absolute, a column of a column range with whether it is
// absolute, and the ends of a range that IronCalc swaps.
const CELL: u8 = 0x10;
const COLUMN: u8 = 0x14;
const SWAPPED_ROWS: u8 = 0x16;
const SWAPPED_COLUMNS: u8 = 0x17;

const LAST_COLUMN: i32 = 16_384;
const LAST_ROW: i32 = 1_048_576;

// A key for a filled formula that costs a byte scan instead of IronCalc's lexer: the text
// with every reference written as a tag and its row and column, each relative one as its
// offset from the cell, as the lexer key writes them. False for any text whose reading
// by the lexer this scan does not mirror exactly, such as rows alone (`3:5`), structured
// references or characters past ASCII outside quotes; those take the lexer key.
pub(super) fn write_fill_key(
    key: &mut Vec<u8>,
    text: &str,
    row: i32,
    column: i32,
    array: bool,
) -> bool {
    key.clear();
    key.push(FILL + u8::from(array));
    let bytes = text.as_bytes();
    if bytes.iter().any(|b| (CELL..=SWAPPED_COLUMNS).contains(b)) {
        return false;
    }
    let mut previous = Word::Other;
    let mut after_colon = false;
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte == b'"' || byte == b'\'' {
            let start = at;
            at += 1;
            loop {
                match bytes.get(at) {
                    None => return false,
                    Some(&b) if b == byte && bytes.get(at + 1) == Some(&byte) => at += 2,
                    Some(&b) if b == byte => break,
                    Some(_) => at += 1,
                }
            }
            at += 1;
            key.extend_from_slice(&bytes[start..at]);
            previous = Word::Other;
            continue;
        }
        if is_word_byte(byte) {
            let start = at;
            while at < bytes.len() && is_word_byte(bytes[at]) {
                at += 1;
            }
            let word = &text[start..at];
            let next = bytes.get(at).copied();
            let kind = if !matches!(next, Some(b'(' | b'!')) {
                word_kind(word)
            } else if word.contains('$') {
                // The lexer reads `A$1(` as a reference before the parenthesis.
                Word::Invalid
            } else {
                Word::Other
            };
            match kind {
                Word::Invalid => return false,
                Word::Cell {
                    row: cell_row,
                    column: cell_column,
                    absolute_row,
                    absolute_column,
                } => {
                    key.push(CELL | u8::from(absolute_row) | u8::from(absolute_column) << 1);
                    write_offset(key, cell_row, absolute_row, row);
                    write_offset(key, cell_column, absolute_column, column);
                }
                // Outside a range, letters alone are a name such as `AB`, not a column.
                Word::Column {
                    column: range_column,
                    absolute,
                } if after_colon || next == Some(b':') => {
                    key.push(COLUMN | u8::from(absolute));
                    write_offset(key, range_column, absolute, column);
                }
                Word::Column { absolute: true, .. } => return false,
                Word::Column { .. } | Word::Other => key.extend_from_slice(word.as_bytes()),
            }
            if after_colon {
                match (previous, kind) {
                    // IronCalc puts the lower row and column first, so which end that is
                    // depends on the cell when only one of them is relative.
                    (
                        Word::Cell {
                            row: top,
                            column: left,
                            ..
                        },
                        Word::Cell {
                            row: bottom,
                            column: right,
                            ..
                        },
                    ) => {
                        if top > bottom {
                            key.push(SWAPPED_ROWS);
                        }
                        if left > right {
                            key.push(SWAPPED_COLUMNS);
                        }
                    }
                    (Word::Column { column: left, .. }, Word::Column { column: right, .. }) => {
                        if left > right {
                            key.push(SWAPPED_COLUMNS);
                        }
                    }
                    _ => return false,
                }
            }
            after_colon = false;
            previous = kind;
            continue;
        }
        // The lexer ends a reference at `\` or `?` and the parser drops what follows, so
        // `Q1\x` reads as the cell Q1.
        if !byte.is_ascii() || matches!(byte, b'[' | b']' | b'\\' | b'?') {
            return false;
        }
        if byte == b':' {
            if at == 0 || !is_word_byte(bytes[at - 1]) {
                return false;
            }
            after_colon = true;
        } else if after_colon {
            return false;
        } else {
            previous = Word::Other;
        }
        key.push(byte);
        at += 1;
    }
    !after_colon
}

// The bytes of IronCalc's identifiers, and the `$` of absolute references.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$')
}

// The lexer reads a number from the start of `1A1` and then the reference A1.
fn is_number(word: &str) -> bool {
    let (mantissa, exponent) = word.split_once(['e', 'E']).unwrap_or((word, ""));
    mantissa.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && exponent.bytes().all(|b| b.is_ascii_digit())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Word {
    Cell {
        row: i32,
        column: i32,
        absolute_row: bool,
        absolute_column: bool,
    },
    // Letters alone, a column if a colon joins it to another, as in `A:C`.
    Column {
        column: i32,
        absolute: bool,
    },
    // Looks like a reference IronCalc may read otherwise, such as a column past XFD.
    Invalid,
    Other,
}

fn word_kind(word: &str) -> Word {
    let bytes = word.as_bytes();
    if bytes[0].is_ascii_digit() || bytes[0] == b'.' {
        return if is_number(word) {
            Word::Other
        } else {
            Word::Invalid
        };
    }
    let absolute_column = bytes[0] == b'$';
    let letters_start = usize::from(absolute_column);
    let letters_end = letters_start
        + bytes[letters_start..]
            .iter()
            .take_while(|b| b.is_ascii_alphabetic())
            .count();
    let absolute_row = bytes.get(letters_end) == Some(&b'$');
    let digits_start = letters_end + usize::from(absolute_row);
    let digits_end = digits_start
        + bytes[digits_start..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
    let letters = &word[letters_start..letters_end];
    let dollar = bytes.contains(&b'$');
    if digits_end != bytes.len() || letters.is_empty() {
        return if dollar { Word::Invalid } else { Word::Other };
    }
    if digits_start == digits_end {
        return if absolute_row {
            Word::Invalid
        } else if letters.len() <= 3 && column_number(letters) <= LAST_COLUMN {
            Word::Column {
                column: column_number(letters),
                absolute: absolute_column,
            }
        } else if dollar {
            Word::Invalid
        } else {
            Word::Other
        };
    }
    if letters.len() > 3 {
        return if dollar { Word::Invalid } else { Word::Other };
    }
    let row = word[digits_start..].parse::<i32>().unwrap_or(0);
    let column = column_number(letters);
    if column > LAST_COLUMN || !(1..=LAST_ROW).contains(&row) {
        return Word::Invalid;
    }
    Word::Cell {
        row,
        column,
        absolute_row,
        absolute_column,
    }
}

fn column_number(letters: &str) -> i32 {
    letters.bytes().fold(0, |number, letter| {
        number * 26 + i32::from(letter.to_ascii_uppercase() - b'A') + 1
    })
}

#[cfg(test)]
mod tests {
    use super::write_fill_key;

    fn key_at(text: &str, row: i32, column: i32) -> Option<Vec<u8>> {
        let mut key = Vec::new();
        write_fill_key(&mut key, text, row, column, false).then_some(key)
    }

    fn key(text: &str, row: i32) -> Option<Vec<u8>> {
        key_at(text, row, 3)
    }

    #[test]
    fn fill_keys_shift_the_relative_rows_and_columns_of_references() {
        let second = key("IF(N2=\"A1\",E2*2,$B$1+LOG10(4)+A$7+SUM(A2:B3))", 2);
        let third = key("IF(N3=\"A1\",E3*2,$B$1+LOG10(4)+A$7+SUM(A3:B4))", 3);
        assert!(second.is_some());
        assert_eq!(second, third);
        assert_eq!(
            second,
            key_at("IF(O2=\"A1\",F2*2,$B$1+LOG10(4)+B$7+SUM(B2:C3))", 2, 4)
        );
        for other in [
            "IF(N3=\"A1\",E3*3,$B$1+LOG10(4)+A$7+SUM(A3:B4))",
            "IF(N3=\"A2\",E3*2,$B$1+LOG10(4)+A$7+SUM(A3:B4))",
            "IF(N3=\"A1\",E3*2,$B$1+LOG10(4)+A$8+SUM(A3:B4))",
            "IF(N3=\"A1\",F3*2,$B$1+LOG10(4)+A$7+SUM(A3:B4))",
        ] {
            assert_ne!(second, key(other, 3), "{other}");
        }
        assert_ne!(key("SUM($A$5:A4)", 4), key("SUM($A$5:A6)", 6));
        assert_eq!(key("SUM($A$5:A6)", 6), key("SUM($A$5:A7)", 7));
        assert_ne!(key_at("SUM($E1:D1)", 1, 3), key_at("SUM($E1:F1)", 1, 5));
        assert_eq!(key_at("SUM($E1:F1)", 1, 5), key_at("SUM($E1:G1)", 1, 6));
        assert_eq!(key_at("SUM(A:B)", 1, 3), key_at("SUM(B:C)", 1, 4));
        assert_ne!(key_at("SUM(A:A)", 1, 3), key_at("SUM(B:B)", 1, 3));
        assert_ne!(key_at("SUM($C:B)", 1, 3), key_at("SUM($C:D)", 1, 5));
        assert_ne!(key_at("AB+1", 1, 3), key_at("AC+1", 1, 4));
        let mut array = Vec::new();
        assert!(write_fill_key(&mut array, "A2", 2, 3, true));
        assert_ne!(Some(array), key("A2", 2));
    }

    #[test]
    fn fill_keys_refuse_what_the_scan_cannot_read_like_the_lexer() {
        for refused in [
            "SUM(3:5)",
            "SUM(A1:B)",
            "Table1[Col]",
            "A\u{f1}o1+1",
            "XFE1",
            "\"open",
            "SUM(A1:'Other'!B2)",
            "SUM($5:$7)",
            "A\u{10}1",
            "$A+1",
            "Q1\\x",
            "A1?",
            "1A1",
            "B$2(1)",
        ] {
            assert_eq!(key(refused, 2), None, "{refused}");
        }
    }
}
