use url::Url;

use crate::markdown_depth;
use crate::markdown_links::is_invisible_or_bidi;

const MAX_NESTING: usize = 16;
const MAX_LINK_BYTES: usize = 2048;
const REPOSITORY_PATH: &str = "/EmilioGiordano/Zenkai";

// The text of a release note is edited on GitHub and is not signed, so it is shown in a
// restricted mode: this cleans what the parser sees, and the pane renders HTML, images and
// links through `vetted_link` only. None when the text nests too deep to be shown.
pub fn clean(text: &str) -> Option<String> {
    let mut cleaned = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                cleaned.push('\n');
            }
            '\n' | '\t' => cleaned.push(character),
            _ if is_invisible_or_bidi(character) => {}
            _ => cleaned.push(character),
        }
    }
    (!markdown_depth::deeper_than(&cleaned, MAX_NESTING)).then_some(cleaned)
}

// The address a link may open, as the URL parser reads it: https, exactly github.com, no user,
// no port, no `..`, and inside this repository. The caller shows and opens the returned text,
// never the one it was given, so what the user confirms is what the browser gets.
pub fn vetted_link(raw: &str) -> Option<String> {
    if raw.len() > MAX_LINK_BYTES
        || raw
            .chars()
            .any(|character| character.is_whitespace() || is_invisible_or_bidi(character))
        || has_parent_segment(raw)
        || has_user_or_port(raw)
    {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let path = url.path();
    let in_repository = path == REPOSITORY_PATH
        || path
            .strip_prefix(REPOSITORY_PATH)
            .is_some_and(|rest| rest.starts_with('/'));
    let allowed = url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && in_repository;
    allowed.then(|| url.into())
}

// The parser resolves `..` before the path is checked, which would hide it; a link that
// contains one is refused as written, in any of the ways a path can spell it.
fn has_parent_segment(raw: &str) -> bool {
    let path = raw.split(['?', '#']).next().unwrap_or_default();
    path.split(['/', '\\'])
        .any(|segment| segment.to_ascii_lowercase().replace("%2e", ".") == "..")
}

// The parser drops a default port, so `:443` has to be caught in the text: the authority of
// an allowed link is the bare host.
fn has_user_or_port(raw: &str) -> bool {
    let after_scheme = raw.split_once(':').map_or(raw, |(_, rest)| rest);
    let authority = after_scheme
        .trim_start_matches(['/', '\\'])
        .split(['/', '\\', '?', '#'])
        .next()
        .unwrap_or_default();
    authority.contains([':', '@'])
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const REPOSITORY: &str = "https://github.com/EmilioGiordano/Zenkai";

    #[test]
    fn links_inside_the_repository_are_allowed() {
        for link in [
            REPOSITORY.to_owned(),
            format!("{REPOSITORY}/"),
            format!("{REPOSITORY}/releases/tag/v0.1.1"),
            format!("{REPOSITORY}/issues/12?q=a#top"),
            format!("{REPOSITORY}/compare/v0.1.0...v0.1.1"),
        ] {
            assert_eq!(vetted_link(&link).as_deref(), Some(link.as_str()), "{link}");
        }
    }

    #[test]
    fn the_address_returned_is_the_one_the_parser_reads() {
        assert_eq!(
            vetted_link("HTTPS://GitHub.com/EmilioGiordano/Zenkai/releases").as_deref(),
            Some("https://github.com/EmilioGiordano/Zenkai/releases")
        );
    }

    #[test]
    fn every_other_link_is_refused() {
        for link in [
            "",
            "#section",
            "releases",
            "http://github.com/EmilioGiordano/Zenkai",
            "ftp://github.com/EmilioGiordano/Zenkai",
            "javascript:alert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "mailto:a@b.c",
            "//github.com/EmilioGiordano/Zenkai",
            "https://github.com.evil.example/EmilioGiordano/Zenkai",
            "https://evil.example/EmilioGiordano/Zenkai",
            "https://www.github.com/EmilioGiordano/Zenkai",
            "https://api.github.com/EmilioGiordano/Zenkai",
            "https://github.com./EmilioGiordano/Zenkai",
            "https://github.com@evil.example/EmilioGiordano/Zenkai",
            "https://github.com:pw@evil.example/EmilioGiordano/Zenkai",
            "https://user@github.com/EmilioGiordano/Zenkai",
            "https://github.com:443/EmilioGiordano/Zenkai",
            "https://github.com:8443/EmilioGiordano/Zenkai",
            "https://github.com\\@evil.example/EmilioGiordano/Zenkai",
            "https://github.com",
            "https://github.com/",
            "https://github.com/EmilioGiordano",
            "https://github.com/EmilioGiordano/ZenkaiEvil",
            "https://github.com/EmilioGiordano/zenkai",
            "https://github.com/emiliogiordano/Zenkai",
            "https://github.com/Other/Zenkai",
            "https://github.com/EmilioGiordano/Zenkai%2Fa",
            "https://github.com/EmilioGiordano/Zenkai/../Other",
            "https://github.com/EmilioGiordano/Zenkai/%2e%2e/Other",
            "https://github.com/EmilioGiordano/Zenkai/.%2E/Other",
            "https://github.com/EmilioGiordano/Zenkai\\..\\Other",
            "https://github.com/EmilioGiordano/Zenkai/releases/..",
            "https://github.com/a/../EmilioGiordano/Zenkai",
            "https://github.com/EmilioGiordano/Zenkai\n/x",
            "https://github.com/EmilioGiordano/Zenkai/\u{202e}x",
            "https://gith\u{200b}ub.com/EmilioGiordano/Zenkai",
            "https://github.com/EmilioGiordano/Zenkai x",
        ] {
            assert_eq!(vetted_link(link), None, "{link:?}");
        }
    }

    #[test]
    fn a_dot_dot_in_the_query_or_fragment_is_not_a_path_segment() {
        assert!(vetted_link(&format!("{REPOSITORY}/issues?q=../x")).is_some());
        assert!(vetted_link(&format!("{REPOSITORY}#../x")).is_some());
    }

    #[test]
    fn a_link_past_the_length_limit_is_refused() {
        let long = format!("{REPOSITORY}/{}", "a".repeat(MAX_LINK_BYTES));
        assert_eq!(vetted_link(&long), None);
    }

    #[test]
    fn line_endings_become_line_feeds_and_hidden_characters_go() {
        assert_eq!(clean("a\r\nb\rc\nd\te").as_deref(), Some("a\nb\nc\nd\te"));
        assert_eq!(
            clean("a\u{202e}b\u{200b}c\u{0}d\u{7f}e\u{85}f\u{feff}").as_deref(),
            Some("abcdef")
        );
        assert_eq!(clean("").as_deref(), Some(""));
    }

    #[test]
    fn text_nested_past_the_limit_is_refused() {
        assert_eq!(clean(&">".repeat(MAX_NESTING + 1)), None);
        assert!(clean(&">".repeat(MAX_NESTING)).is_some());
        assert_eq!(clean(&"[".repeat(1_000_000)), None);
    }

    #[test]
    fn markup_is_left_for_the_renderer_to_show_as_text() {
        let text = "<script>x</script>\n\n![logo](C:\\a.png)\n\n[a](file:///x)\n";
        assert_eq!(clean(text).as_deref(), Some(text));
    }

    const FRAGMENTS: &[&str] = &[
        "https://",
        "http://",
        "https:",
        "//",
        "github.com",
        "GitHub.com",
        "evil.example",
        "@",
        ":",
        "443",
        "/",
        "\\",
        "/EmilioGiordano/Zenkai",
        "/EmilioGiordano",
        "/..",
        "/%2e%2e",
        "/.",
        "%2f",
        "?q=1",
        "#frag",
        " ",
        "\n",
        "\u{202e}",
        "é",
        "%",
    ];

    fn assembled() -> impl Strategy<Value = String> {
        prop::collection::vec(prop::sample::select(FRAGMENTS.to_vec()), 0..10)
            .prop_map(|parts| parts.concat())
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

        #[test]
        fn an_allowed_link_is_a_plain_https_address_inside_the_repository(raw in assembled()) {
            if let Some(allowed) = vetted_link(&raw) {
                let url = Url::parse(&allowed).unwrap();
                prop_assert_eq!(url.scheme(), "https");
                prop_assert_eq!(url.host_str(), Some("github.com"));
                prop_assert!(url.username().is_empty() && url.password().is_none());
                prop_assert_eq!(url.port(), None);
                prop_assert!(
                    url.path() == REPOSITORY_PATH
                        || url.path().starts_with("/EmilioGiordano/Zenkai/")
                );
                prop_assert!(url.path_segments().unwrap().all(|segment| segment != ".."));
                prop_assert!(!allowed.chars().any(|c| c.is_whitespace() || is_invisible_or_bidi(c)));
                prop_assert_eq!(vetted_link(&allowed).as_deref(), Some(allowed.as_str()));
            }
        }

        #[test]
        fn any_text_is_vetted_without_a_panic(raw in prop::collection::vec(any::<char>(), 0..80).prop_map(String::from_iter)) {
            if let Some(allowed) = vetted_link(&raw) {
                prop_assert!(allowed.starts_with("https://github.com/EmilioGiordano/Zenkai"));
            }
        }

        #[test]
        fn cleaned_text_has_no_control_or_direction_characters(text in prop::collection::vec(any::<char>(), 0..200).prop_map(String::from_iter)) {
            if let Some(cleaned) = clean(&text) {
                prop_assert!(cleaned
                    .chars()
                    .all(|c| c == '\n' || c == '\t' || !is_invisible_or_bidi(c)));
                prop_assert_eq!(clean(&cleaned).as_deref(), Some(cleaned.as_str()));
            }
        }
    }
}
