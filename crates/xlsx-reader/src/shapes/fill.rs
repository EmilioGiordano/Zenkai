use super::write_unsigned;

// The first byte of a fill key; a lexer key starts with 0 or 1, so the two never meet.
const FILL: u8 = 2;
// Marks a relative-row cell reference in the key, and a range written bottom up.
const CELL: u8 = 1;
const SWAPPED: u8 = 2;

const LAST_COLUMN: i32 = 16_384;
const LAST_ROW: i32 = 1_048_576;

// A key for a formula filled down a column that costs a byte scan instead of IronCalc's
// lexer: the column, then the text with the row of every relative cell reference written
// as its offset from the cell. False for any text whose reading by the lexer this scan
// does not mirror exactly, such as rows alone (`3:5`), structured references or
// characters past ASCII outside quotes; those take the lexer key.
pub(super) fn write_fill_key(
    key: &mut Vec<u8>,
    text: &str,
    row: i32,
    column: i32,
    array: bool,
) -> bool {
    key.clear();
    key.push(FILL + u8::from(array));
    write_unsigned(key, u64::from(column.unsigned_abs()));
    let bytes = text.as_bytes();
    if bytes.contains(&CELL) || bytes.contains(&SWAPPED) {
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
            let kind = if matches!(bytes.get(at), Some(b'(' | b'!')) {
                Word::Other
            } else {
                word_kind(word)
            };
            match kind {
                Word::Invalid => return false,
                // The offset goes before the letters: a varint ends itself, and the
                // letters end at the next byte, which is never part of a word.
                Word::Cell {
                    letters,
                    row: cell_row,
                    relative_row: true,
                } => {
                    key.push(CELL);
                    let offset = i64::from(cell_row) - i64::from(row);
                    write_unsigned(key, ((offset << 1) ^ (offset >> 63)) as u64);
                    key.extend_from_slice(letters.as_bytes());
                }
                _ => key.extend_from_slice(word.as_bytes()),
            }
            if after_colon {
                match (previous, kind) {
                    // IronCalc puts the lower row first, so which end that is depends on
                    // the cell when only one row is relative.
                    (Word::Cell { row: first, .. }, Word::Cell { row: last, .. }) => {
                        if first > last {
                            key.push(SWAPPED);
                        }
                    }
                    (Word::Column, Word::Column) => {}
                    _ => return false,
                }
            }
            after_colon = false;
            previous = kind;
            continue;
        }
        if !byte.is_ascii() || byte == b'[' || byte == b']' {
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

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$' | b'\\' | b'?')
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Word<'w> {
    // A cell reference: its column as written and its row.
    Cell {
        letters: &'w str,
        row: i32,
        relative_row: bool,
    },
    // A column alone, as in `A:C`.
    Column,
    // Looks like a reference IronCalc may read otherwise, such as a column past XFD.
    Invalid,
    Other,
}

fn word_kind(word: &str) -> Word<'_> {
    let bytes = word.as_bytes();
    let column_absolute = bytes.first() == Some(&b'$');
    let letters_start = usize::from(column_absolute);
    let letters_end = letters_start
        + bytes[letters_start..]
            .iter()
            .take_while(|b| b.is_ascii_alphabetic())
            .count();
    let row_absolute = bytes.get(letters_end) == Some(&b'$');
    let digits_start = letters_end + usize::from(row_absolute);
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
        return if row_absolute {
            Word::Invalid
        } else if letters.len() <= 3 && column_number(letters) <= LAST_COLUMN {
            Word::Column
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
    if column_number(letters) > LAST_COLUMN || !(1..=LAST_ROW).contains(&row) {
        return Word::Invalid;
    }
    Word::Cell {
        letters: &word[..letters_end],
        row,
        relative_row: !row_absolute,
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

    fn key(text: &str, row: i32) -> Option<Vec<u8>> {
        let mut key = Vec::new();
        write_fill_key(&mut key, text, row, 3, false).then_some(key)
    }

    #[test]
    fn fill_keys_shift_only_the_rows_of_relative_cells() {
        let second = key("IF(N2=\"A1\",E2*2,$B$1+LOG10(4)+A$7+SUM(A2:B3))", 2);
        let third = key("IF(N3=\"A1\",E3*2,$B$1+LOG10(4)+A$7+SUM(A3:B4))", 3);
        assert!(second.is_some());
        assert_eq!(second, third);
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
            "A\u{1}1",
        ] {
            assert_eq!(key(refused, 2), None, "{refused}");
        }
    }
}
