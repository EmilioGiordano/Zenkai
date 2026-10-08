use crate::error::ColumnProblem;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placeholders {
    LettersAndDigits,
    DigitsOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    Letter,
    Digit,
    Literal(char),
}

impl Token {
    fn base(self) -> u128 {
        match self {
            Token::Letter => 26,
            Token::Digit => 10,
            Token::Literal(_) => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pattern {
    tokens: Vec<Token>,
}

impl Pattern {
    pub(crate) fn parse(text: &str, placeholders: Placeholders) -> Result<Pattern, ColumnProblem> {
        let mut tokens = Vec::with_capacity(text.len());
        let mut chars = text.chars();
        while let Some(symbol) = chars.next() {
            let token = match (symbol, placeholders) {
                ('\\', _) => Token::Literal(chars.next().ok_or(ColumnProblem::DanglingEscape)?),
                ('#', _) => Token::Digit,
                ('A', Placeholders::LettersAndDigits) => Token::Letter,
                (other, _) => Token::Literal(other),
            };
            tokens.push(token);
        }
        if !tokens.iter().any(|token| token.base() > 1) {
            return Err(ColumnProblem::NoPlaceholders);
        }
        Ok(Pattern { tokens })
    }

    pub(crate) fn domain(&self) -> u128 {
        self.tokens
            .iter()
            .fold(1u128, |size, token| size.saturating_mul(token.base()))
    }

    pub(crate) fn value_at(&self, index: u128) -> String {
        let mut remaining = index;
        let mut chars: Vec<char> = self
            .tokens
            .iter()
            .rev()
            .map(|token| {
                let digit = remaining % token.base();
                remaining /= token.base();
                match token {
                    Token::Letter => char::from(b'A' + digit as u8),
                    Token::Digit => char::from(b'0' + digit as u8),
                    Token::Literal(symbol) => *symbol,
                }
            })
            .collect();
        chars.reverse();
        chars.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_literals_and_escapes() {
        let pattern = Pattern::parse(r"AAA-##\#\A", Placeholders::LettersAndDigits).unwrap();
        assert_eq!(pattern.domain(), 26 * 26 * 26 * 100);
        assert_eq!(pattern.value_at(0), "AAA-00#A");
        assert_eq!(pattern.value_at(pattern.domain() - 1), "ZZZ-99#A");
        assert_eq!(pattern.value_at(101), "AAB-01#A");
    }

    #[test]
    fn phone_patterns_keep_letters_literal() {
        let pattern = Pattern::parse("+54 9 11 ####-####", Placeholders::DigitsOnly).unwrap();
        assert_eq!(pattern.domain(), 100_000_000);
        assert_eq!(pattern.value_at(12_345_678), "+54 9 11 1234-5678");
        let with_letter = Pattern::parse("A ##", Placeholders::DigitsOnly).unwrap();
        assert_eq!(with_letter.value_at(7), "A 07");
    }

    #[test]
    fn rejects_dangling_escape_and_constant_patterns() {
        let parse = |text| Pattern::parse(text, Placeholders::LettersAndDigits);
        assert_eq!(parse(r"AB\"), Err(ColumnProblem::DanglingEscape));
        assert_eq!(parse(r"\A\#-x"), Err(ColumnProblem::NoPlaceholders));
        assert_eq!(parse(""), Err(ColumnProblem::NoPlaceholders));
    }

    #[test]
    fn huge_patterns_saturate_instead_of_overflowing() {
        let pattern = Pattern::parse(&"#".repeat(60), Placeholders::DigitsOnly).unwrap();
        assert_eq!(pattern.domain(), u128::MAX);
        assert_eq!(pattern.value_at(5).len(), 60);
    }
}
