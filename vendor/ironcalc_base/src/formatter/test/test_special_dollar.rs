// [$$-en-US]#,##0.00_);[Red]([$$-en-US]#,##0.00)

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]

use crate::formatter::lexer::{Lexer, Token};

#[test]
fn dollar_en_us() {
    let mut lexer = Lexer::new("[$$-en-US]");
    let token = lexer.next_token();
    assert!(matches!(token, Token::Currency('$')));
}

#[test]
fn dollar_409() {
    let mut lexer = Lexer::new("[$$-409]");
    let token = lexer.next_token();
    assert!(matches!(token, Token::Currency('$')));
}

#[test]
fn locale_without_symbol_is_skipped() {
    let mut lexer = Lexer::new("[$-409]d");
    assert!(matches!(lexer.next_token(), Token::Day));
    assert!(matches!(lexer.next_token(), Token::EOF));
}

#[test]
fn unclosed_locale_without_symbol_is_an_error() {
    let mut lexer = Lexer::new("[$-409");
    assert!(matches!(lexer.next_token(), Token::ILLEGAL));
}

#[test]
fn many_locale_sections_do_not_overflow_the_stack() {
    let format = format!("{}d", "[$-409]".repeat(100_000));
    let mut lexer = Lexer::new(&format);
    assert!(matches!(lexer.next_token(), Token::Day));
}
