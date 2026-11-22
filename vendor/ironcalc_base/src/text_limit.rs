use crate::{
    calc_result::CalcResult,
    expressions::{token::Error, types::CellReferenceIndex},
};

/// Excel holds at most 32,767 characters in a text. `&` and the text functions whose result
/// would be longer return `#VALUE!` instead of building it.
pub(crate) const MAX_TEXT_LENGTH: usize = 32_767;

/// The length of `text` as Excel counts it, in UTF-16 code units: a character outside the
/// Basic Multilingual Plane counts twice.
pub(crate) fn text_length(text: &str) -> usize {
    text.encode_utf16().count()
}

pub(crate) fn text_too_long(cell: CellReferenceIndex) -> CalcResult {
    CalcResult::new_error(
        Error::VALUE,
        cell,
        format!("Text longer than {MAX_TEXT_LENGTH} characters"),
    )
}

#[derive(Debug, PartialEq)]
pub(crate) struct TextTooLong;

#[derive(Default)]
pub(crate) struct LimitedText {
    text: String,
    length: usize,
    too_long: bool,
}

impl LimitedText {
    // Once a piece does not fit the text stays too long and later pieces are dropped, so a
    // caller can still evaluate its other arguments and give the error one of them holds.
    pub(crate) fn push(&mut self, piece: &str) {
        if self.too_long {
            return;
        }
        let length = self.length + text_length(piece);
        if length > MAX_TEXT_LENGTH {
            self.too_long = true;
            self.text = String::new();
            return;
        }
        self.text.push_str(piece);
        self.length = length;
    }

    pub(crate) fn is_too_long(&self) -> bool {
        self.too_long
    }

    pub(crate) fn into_string(self) -> Result<String, TextTooLong> {
        if self.too_long {
            Err(TextTooLong)
        } else {
            Ok(self.text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_counts_utf16_code_units() {
        assert_eq!(text_length("abc"), 3);
        assert_eq!(text_length("é"), 1);
        assert_eq!(text_length("€"), 1);
        assert_eq!(text_length("😀"), 2);
    }

    #[test]
    fn limited_text_takes_pieces_up_to_the_limit_only() {
        let mut text = LimitedText::default();
        text.push(&"x".repeat(MAX_TEXT_LENGTH - 1));
        text.push("é");
        text.push("");
        let text = text.into_string();
        assert_eq!(text.as_deref().map(text_length), Ok(MAX_TEXT_LENGTH));
        assert!(text.is_ok_and(|text| text.ends_with('é')));
    }

    #[test]
    fn limited_text_stays_too_long_once_a_piece_does_not_fit() {
        let mut text = LimitedText::default();
        text.push(&"x".repeat(MAX_TEXT_LENGTH - 1));
        assert!(!text.is_too_long());
        text.push("😀");
        text.push("é");
        assert!(text.is_too_long());
        assert_eq!(text.into_string(), Err(TextTooLong));
    }
}
