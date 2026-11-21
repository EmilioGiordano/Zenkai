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

/// The piece that would take a [`LimitedText`] past [`MAX_TEXT_LENGTH`].
#[derive(Debug, PartialEq)]
pub(crate) struct TextTooLong;

/// A text built piece by piece that never grows past [`MAX_TEXT_LENGTH`].
#[derive(Default)]
pub(crate) struct LimitedText {
    text: String,
    length: usize,
}

impl LimitedText {
    /// Appends `piece`, or leaves the text as it was when the result would be too long.
    pub(crate) fn push(&mut self, piece: &str) -> Result<(), TextTooLong> {
        let length = self.length + text_length(piece);
        if length > MAX_TEXT_LENGTH {
            return Err(TextTooLong);
        }
        self.text.push_str(piece);
        self.length = length;
        Ok(())
    }

    pub(crate) fn into_string(self) -> String {
        self.text
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
        assert_eq!(text.push(&"x".repeat(MAX_TEXT_LENGTH - 1)), Ok(()));
        assert_eq!(text.push("😀"), Err(TextTooLong));
        assert_eq!(text.push("é"), Ok(()));
        assert_eq!(text.push(""), Ok(()));
        assert_eq!(text.push("x"), Err(TextTooLong));
        let text = text.into_string();
        assert_eq!(text_length(&text), MAX_TEXT_LENGTH);
        assert!(text.ends_with('é'));
    }
}
