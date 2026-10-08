use crate::error::{ColumnProblem, TextField};
use crate::text::is_format_character;

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
    let allows_line_breaks = field == TextField::ListValue;
    let unwanted_control = |symbol: char| {
        symbol.is_control() && !(allows_line_breaks && (symbol == '\t' || symbol == '\n'))
    };
    if text.chars().any(unwanted_control) {
        return Err(ColumnProblem::ControlCharacter { field });
    }
    if text.chars().any(is_format_character) {
        return Err(ColumnProblem::InvisibleCharacter { field });
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
    fn headers_are_single_line() {
        for text in ["a\tb", "a\nb"] {
            assert_eq!(
                check_text(text, TextField::Header, 10),
                Err(ColumnProblem::ControlCharacter {
                    field: TextField::Header
                })
            );
        }
    }

    #[test]
    fn invisible_format_characters_fail_in_every_field() {
        for field in [
            TextField::Header,
            TextField::ListValue,
            TextField::Pattern,
            TextField::Domain,
        ] {
            for text in [
                "evil\u{202e}txt",
                "a\u{200b}b",
                "\u{feff}x",
                "\u{2066}x",
                "\u{e0041}",
            ] {
                assert_eq!(
                    check_text(text, field, 20),
                    Err(ColumnProblem::InvisibleCharacter { field }),
                    "{text:?}"
                );
            }
        }
        assert_eq!(check_text("Teléfono ñandú", TextField::Header, 20), Ok(()));
    }

    #[test]
    fn control_characters_fail_except_tab_and_newline_in_list_values() {
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
