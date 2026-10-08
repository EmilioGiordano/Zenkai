use crate::error::{ColumnProblem, TextField};

// Excel's limit on a table column name.
pub(crate) const MAX_HEADER_CHARS: usize = 255;
// Same bound as a header: a list value is a short label, not a document.
pub(crate) const MAX_OPTION_CHARS: usize = 255;
// Ten times the longest real-world code or phone format; keeps every value far below a cell.
pub(crate) const MAX_PATTERN_CHARS: usize = 256;
// A list longer than this is data to import, not choices to pick from.
pub(crate) const MAX_OPTIONS: usize = 10_000;
// Far above any realistic set of mail providers.
pub(crate) const MAX_DOMAINS: usize = 100;
// The DNS limit on a full domain name.
pub(crate) const MAX_DOMAIN_CHARS: usize = 253;

pub(crate) fn check_text(text: &str, field: TextField, limit: usize) -> Result<(), ColumnProblem> {
    let chars = text.chars().count();
    if chars > limit {
        return Err(ColumnProblem::TextTooLong {
            field,
            chars,
            limit,
        });
    }
    if text
        .chars()
        .any(|symbol| symbol.is_control() && symbol != '\t' && symbol != '\n')
    {
        return Err(ColumnProblem::ControlCharacter { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_at_the_limit_passes_and_one_more_char_fails() {
        let at_limit = "ñ".repeat(MAX_HEADER_CHARS);
        assert_eq!(
            check_text(&at_limit, TextField::Header, MAX_HEADER_CHARS),
            Ok(())
        );
        assert_eq!(
            check_text(&format!("{at_limit}a"), TextField::Header, MAX_HEADER_CHARS),
            Err(ColumnProblem::TextTooLong {
                field: TextField::Header,
                chars: MAX_HEADER_CHARS + 1,
                limit: MAX_HEADER_CHARS
            })
        );
    }

    #[test]
    fn control_characters_fail_except_tab_and_newline() {
        assert_eq!(check_text("a\tb\nc", TextField::ListValue, 10), Ok(()));
        for text in ["a\rb", "\u{0}", "a\u{1b}[2J", "\u{7f}", "\u{85}"] {
            assert_eq!(
                check_text(text, TextField::ListValue, 10),
                Err(ColumnProblem::ControlCharacter {
                    field: TextField::ListValue
                }),
                "{text:?}"
            );
        }
    }
}
