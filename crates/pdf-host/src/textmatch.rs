use crate::wire::CharSpan;

// One char to one char, so a match keeps the indices of the page's char list.
fn lowercase(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

// Matches of `query` in one page's text, as char ranges. Case is ignored; a char is compared
// on its own so the indices stay those of the page's char list.
pub fn find_in_page(chars: &[char], query: &str) -> Vec<CharSpan> {
    let needle: Vec<char> = query.chars().map(lowercase).collect();
    if needle.is_empty() || needle.len() > chars.len() {
        return Vec::new();
    }
    let lowered: Vec<char> = chars.iter().map(|c| lowercase(*c)).collect();
    let mut found = Vec::new();
    let mut start = 0;
    while start + needle.len() <= lowered.len() {
        if lowered[start..start + needle.len()] == needle[..] {
            found.push(CharSpan {
                start,
                len: needle.len(),
            });
            start += needle.len();
        } else {
            start += 1;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn matches_ignore_case_and_do_not_overlap() {
        let spans = find_in_page(&chars("Aaa aA"), "aa");
        assert_eq!(
            spans,
            [CharSpan { start: 0, len: 2 }, CharSpan { start: 4, len: 2 }]
        );
    }

    #[test]
    fn an_empty_or_longer_query_matches_nothing() {
        assert!(find_in_page(&chars("abc"), "").is_empty());
        assert!(find_in_page(&chars("ab"), "abc").is_empty());
        assert!(find_in_page(&[], "a").is_empty());
    }
}
