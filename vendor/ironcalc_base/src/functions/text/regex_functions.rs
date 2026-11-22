#[cfg(not(target_arch = "wasm32"))]
use regex::Regex;
#[cfg(target_arch = "wasm32")]
use regex_lite::Regex;

use crate::{
    calc_result::CalcResult,
    expressions::{
        parser::{ArrayNode, Node},
        token::Error,
        types::CellReferenceIndex,
    },
    model::Model,
    text_limit::{text_too_long, LimitedText, TextTooLong},
};

#[derive(Debug, PartialEq)]
enum ReplacementPiece<'r> {
    Text(&'r str),
    Group(usize),
}

// Reads `replacement` as `Captures::expand` of the regex crates does: `$$` is `$`, `$name`
// (letters, digits and `_`) and `${name}` name a group by number or name, a group the pattern
// lacks is left out, and any other `$` is itself.
fn replacement_pieces<'r>(re: &Regex, mut replacement: &'r str) -> Vec<ReplacementPiece<'r>> {
    let mut pieces = Vec::new();
    while let Some(dollar) = replacement.find('$') {
        pieces.push(ReplacementPiece::Text(&replacement[..dollar]));
        let after = &replacement[dollar + 1..];
        if let Some(rest) = after.strip_prefix('$') {
            pieces.push(ReplacementPiece::Text("$"));
            replacement = rest;
            continue;
        }
        let reference = match after.strip_prefix('{') {
            Some(braced) => braced
                .find('}')
                .map(|close| (&braced[..close], &braced[close + 1..])),
            None => {
                let name_length = after
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                    .count();
                if name_length == 0 {
                    None
                } else {
                    Some(after.split_at(name_length))
                }
            }
        };
        match reference {
            Some((name, rest)) => {
                let index = match name.parse::<usize>() {
                    Ok(index) => Some(index),
                    Err(_) => re.capture_names().position(|group| group == Some(name)),
                };
                if let Some(index) = index {
                    pieces.push(ReplacementPiece::Group(index));
                }
                replacement = rest;
            }
            None => {
                pieces.push(ReplacementPiece::Text("$"));
                replacement = after;
            }
        }
    }
    pieces.push(ReplacementPiece::Text(replacement));
    pieces
}

// Gives what `re.replace_all` gives, or stops at Excel's text limit.
fn replace_all_within_limit(
    re: &Regex,
    text: &str,
    replacement: &str,
) -> Result<String, TextTooLong> {
    let pieces = replacement_pieces(re, replacement);
    let mut result = LimitedText::default();
    let mut last_end = 0;
    for captures in re.captures_iter(text) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        result.push(&text[last_end..whole.start()]);
        for piece in &pieces {
            match piece {
                ReplacementPiece::Text(piece) => result.push(piece),
                ReplacementPiece::Group(index) => {
                    if let Some(group) = captures.get(*index) {
                        result.push(group.as_str());
                    }
                }
            }
        }
        last_end = whole.end();
        if result.is_too_long() {
            break;
        }
    }
    result.push(&text[last_end..]);
    result.into_string()
}

impl<'a> Model<'a> {
    /// REGEXEXTRACT(text, regular_expression, [return_all])
    ///
    /// Returns the first substring of `text` that matches `regular_expression`.
    /// If the pattern contains capture groups, returns the content of the first group.
    /// When the optional `return_all` argument is 1, returns a horizontal array of
    /// every match (one match per column) instead of only the first one.
    /// Returns #N/A if there is no match, #VALUE! if the regex is invalid.
    pub(crate) fn fn_regexextract(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> CalcResult {
        if args.len() < 2 || args.len() > 3 {
            return CalcResult::new_args_number_error(cell);
        }
        let text = match self.get_string(&args[0], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let pattern = match self.get_string(&args[1], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let return_all = if args.len() == 3 {
            match self.get_number(&args[2], cell) {
                Ok(n) => n != 0.0,
                Err(e) => return e,
            }
        } else {
            false
        };
        let re = match Regex::new(&pattern) {
            Ok(r) => r,
            Err(_) => {
                return CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "Invalid regular expression".to_string(),
                )
            }
        };
        if return_all {
            let row: Vec<ArrayNode> = re
                .find_iter(&text)
                .map(|m| ArrayNode::String(m.as_str().to_string()))
                .collect();
            if row.is_empty() {
                return CalcResult::new_error(Error::NA, cell, "No match found".to_string());
            }
            CalcResult::Array(vec![row])
        } else {
            match re.captures(&text) {
                None => CalcResult::new_error(Error::NA, cell, "No match found".to_string()),
                Some(caps) => {
                    // If there is at least one explicit capture group, return group 1.
                    // Otherwise return the full match (group 0).
                    let matched = if caps.len() > 1 {
                        caps.get(1).map_or("", |m| m.as_str())
                    } else {
                        caps.get(0).map_or("", |m| m.as_str())
                    };
                    CalcResult::String(matched.to_string())
                }
            }
        }
    }

    /// REGEXREPLACE(text, regular_expression, replacement)
    ///
    /// Replaces every substring of `text` that matches `regular_expression` with
    /// `replacement`. Returns #VALUE! if the regex is invalid.
    pub(crate) fn fn_regexreplace(
        &mut self,
        args: &[Node],
        cell: CellReferenceIndex,
    ) -> CalcResult {
        if args.len() != 3 {
            return CalcResult::new_args_number_error(cell);
        }
        let text = match self.get_string(&args[0], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let pattern = match self.get_string(&args[1], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let replacement = match self.get_string(&args[2], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let re = match Regex::new(&pattern) {
            Ok(r) => r,
            Err(_) => {
                return CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "Invalid regular expression".to_string(),
                )
            }
        };
        match replace_all_within_limit(&re, &text, &replacement) {
            Ok(replaced) => CalcResult::String(replaced),
            Err(TextTooLong) => text_too_long(cell),
        }
    }

    /// REGEXTEST(text, regular_expression)
    ///
    /// Returns `TRUE` if `regular_expression` matches anywhere in `text`, `FALSE`
    /// otherwise. Returns #VALUE! if the regex is invalid.
    pub(crate) fn fn_regextest(&mut self, args: &[Node], cell: CellReferenceIndex) -> CalcResult {
        if args.len() != 2 {
            return CalcResult::new_args_number_error(cell);
        }
        let text = match self.get_string(&args[0], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let pattern = match self.get_string(&args[1], cell) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let re = match Regex::new(&pattern) {
            Ok(r) => r,
            Err(_) => {
                return CalcResult::new_error(
                    Error::VALUE,
                    cell,
                    "Invalid regular expression".to_string(),
                )
            }
        };
        CalcResult::Boolean(re.is_match(&text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Replacements made of `$`, braces, group names and numbers, against patterns with
    // numbered, named and optional groups, give what the regex crate gives.
    #[test]
    fn replace_all_within_limit_agrees_with_replace_all() {
        let patterns = [
            "a",
            "(a)(b)?",
            "(?P<word>[a-c]+)(?P<n>[0-9])?",
            "x*",
            "(é)|(z)",
        ];
        let texts = ["", "abc", "aab9 cab1 b", "xxaxx", "zéz é"];
        let symbols = [
            "$", "$", "{", "}", "0", "1", "2", "9", "a", "n", "word", "_", "+", " ", "é", "x",
        ];
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut random = move |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        for pattern in patterns {
            let re = Regex::new(pattern).unwrap();
            for _ in 0..400 {
                let length = random(9);
                let replacement: String = (0..length)
                    .map(|_| symbols[random(symbols.len())])
                    .collect();
                for text in texts {
                    assert_eq!(
                        replace_all_within_limit(&re, text, &replacement).as_deref(),
                        Ok(&*re.replace_all(text, replacement.as_str())),
                        "pattern {pattern:?}, text {text:?}, replacement {replacement:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn replacement_pieces_read_references_as_the_regex_crate_does() {
        let re = Regex::new("(?P<first>a)(b)").unwrap();
        assert_eq!(
            replacement_pieces(&re, "x$1${first}$$y$first$z${3}$1a${"),
            vec![
                ReplacementPiece::Text("x"),
                ReplacementPiece::Group(1),
                ReplacementPiece::Text(""),
                ReplacementPiece::Group(1),
                ReplacementPiece::Text(""),
                ReplacementPiece::Text("$"),
                ReplacementPiece::Text("y"),
                ReplacementPiece::Group(1),
                ReplacementPiece::Text(""),
                ReplacementPiece::Text(""),
                ReplacementPiece::Group(3),
                ReplacementPiece::Text(""),
                ReplacementPiece::Text(""),
                ReplacementPiece::Text("$"),
                ReplacementPiece::Text("{"),
            ]
        );
    }
}
