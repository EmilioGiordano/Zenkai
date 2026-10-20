use std::path::{Component, Path, PathBuf};

use crate::files::{is_openable, is_remote_or_device};
use crate::text_document::DocumentKind;

#[derive(Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Anchor,
    Web(String),
    File(PathBuf),
    Refused,
}

const SHOWN_TARGET_CHARS: usize = 80;

fn is_invisible_or_bidi(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{061c}'
                | '\u{feff}'
        )
}

// What a prompt or a notice shows of a link: no control, invisible or direction-changing
// characters that could disguise it, and an ellipsis where it was cut.
pub fn shown_target(url: &str) -> String {
    let mut shown: String = url
        .chars()
        .filter(|character| !is_invisible_or_bidi(*character))
        .take(SHOWN_TARGET_CHARS + 1)
        .collect();
    if shown.chars().count() > SHOWN_TARGET_CHARS {
        shown = shown.chars().take(SHOWN_TARGET_CHARS).collect();
        shown.push('\u{2026}');
    }
    shown
}

// The host a web link goes to, without the user name some links put in front of it to look
// like another site.
pub fn host_of(url: &str) -> String {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme
        .split(['/', '?', '#', '\\'])
        .next()
        .unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    host.chars()
        .filter(|character| !is_invisible_or_bidi(*character))
        .collect()
}

pub(crate) fn scheme_of(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    let drive = scheme.len() == 1 && rest.starts_with(['/', '\\']);
    (valid && !drive).then_some(scheme)
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                at += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| text.to_owned())
}

pub(crate) fn local_path(url: &str) -> Option<PathBuf> {
    let without_fragment = url.split(['#', '?']).next().unwrap_or_default();
    let decoded = percent_decoded(without_fragment);
    (!decoded.is_empty())
        .then(|| PathBuf::from(decoded.replace('/', std::path::MAIN_SEPARATOR_STR)))
}

// `CON`, `NUL`, `COM1` and the like name devices on Windows whatever the extension or stream
// suffix, in any case and in any folder of the path; opening one can block or reach hardware.
// A `:` inside a name is an alternate data stream and is refused outright.
fn is_reserved_device_name(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|rest| {
            matches!(rest, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                || matches!(rest, "\u{b9}" | "\u{b2}" | "\u{b3}")
        })
    };
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || numbered("COM")
        || numbered("LPT")
}

pub(crate) fn names_a_device(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => {
            is_reserved_device_name(name) || name.to_string_lossy().contains(':')
        }
        _ => false,
    })
}

pub(crate) fn is_network_style(url: &str) -> bool {
    url.starts_with("//") || url.starts_with("\\\\")
}

// A link in a document is untrusted text: only http and https leave the app (after the user
// agrees), and only workbooks and Markdown files open inside it.
pub fn classify_link(url: &str, folder: Option<&Path>) -> LinkTarget {
    let url = url.trim();
    if url.starts_with('#') {
        return LinkTarget::Anchor;
    }
    if let Some(scheme) = scheme_of(url) {
        return if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
            LinkTarget::Web(url.to_owned())
        } else {
            LinkTarget::Refused
        };
    }
    if is_network_style(url) {
        return LinkTarget::Refused;
    }
    let Some(path) = local_path(url) else {
        return LinkTarget::Anchor;
    };
    let full = if path.is_absolute() || path.has_root() {
        path
    } else if let Some(folder) = folder {
        folder.join(path)
    } else {
        return LinkTarget::Refused;
    };
    let openable = is_openable(&full) || DocumentKind::of(&full) == DocumentKind::Markdown;
    if is_remote_or_device(&full) || names_a_device(&full) || !openable {
        LinkTarget::Refused
    } else {
        LinkTarget::File(full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_images::image_path;

    fn folder() -> &'static Path {
        Path::new("docs")
    }

    #[test]
    fn web_links_are_only_http_and_https() {
        assert_eq!(
            classify_link("https://example.com/a?b=1", None),
            LinkTarget::Web("https://example.com/a?b=1".into())
        );
        assert!(matches!(
            classify_link("HTTP://x.y", None),
            LinkTarget::Web(_)
        ));
        for refused in [
            "javascript:alert(1)",
            "file:///C:/x.xlsx",
            "ms-msdt:/id",
            "mailto:a@b.c",
            "ftp://x.y/a",
            "data:text/html;base64,AAAA",
            "//host/share/a.xlsx",
            r"\\host\share\a.xlsx",
        ] {
            assert_eq!(
                classify_link(refused, Some(folder())),
                LinkTarget::Refused,
                "{refused}"
            );
        }
    }

    #[test]
    fn a_relative_link_to_a_workbook_or_markdown_opens_next_to_the_document() {
        assert_eq!(
            classify_link("data/q3.xlsx", Some(folder())),
            LinkTarget::File(folder().join("data").join("q3.xlsx"))
        );
        assert_eq!(
            classify_link("notes%20two.md#intro", Some(folder())),
            LinkTarget::File(folder().join("notes two.md"))
        );
        assert_eq!(
            classify_link("../sibling/plan.markdown", Some(folder())),
            LinkTarget::File(folder().join("..").join("sibling").join("plan.markdown"))
        );
    }

    #[test]
    fn other_file_types_and_unsaved_documents_do_not_open() {
        assert_eq!(
            classify_link("run.exe", Some(folder())),
            LinkTarget::Refused
        );
        assert_eq!(
            classify_link("script.bat", Some(folder())),
            LinkTarget::Refused
        );
        assert_eq!(classify_link("data.xlsx", None), LinkTarget::Refused);
    }

    #[test]
    fn an_anchor_stays_in_the_document() {
        assert_eq!(
            classify_link("#section", Some(folder())),
            LinkTarget::Anchor
        );
        assert_eq!(classify_link("", Some(folder())), LinkTarget::Anchor);
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_path_is_a_path_not_a_scheme_and_a_share_is_refused() {
        assert_eq!(
            classify_link(r"C:\data\q3.xlsx", None),
            LinkTarget::File(PathBuf::from(r"C:\data\q3.xlsx"))
        );
        assert_eq!(
            classify_link(r"\\host\share\q3.xlsx", None),
            LinkTarget::Refused
        );
    }

    #[test]
    fn a_shown_link_loses_what_could_disguise_it_and_marks_where_it_was_cut() {
        assert_eq!(shown_target("https://a.b/c"), "https://a.b/c");
        assert_eq!(
            shown_target("https://a.b/\u{202e}gpj.exe\u{200b}\n\t"),
            "https://a.b/gpj.exe"
        );
        let long = format!("https://a.b/{}", "x".repeat(200));
        let shown = shown_target(&long);
        assert_eq!(shown.chars().count(), SHOWN_TARGET_CHARS + 1);
        assert!(shown.ends_with('\u{2026}'));
        let exact = "x".repeat(SHOWN_TARGET_CHARS);
        assert_eq!(shown_target(&exact), exact);
    }

    #[test]
    fn the_host_is_what_follows_any_user_name() {
        assert_eq!(host_of("https://example.com/a?b#c"), "example.com");
        assert_eq!(host_of("https://example.com:8443/a"), "example.com:8443");
        assert_eq!(
            host_of("https://bank.com@evil.example/login"),
            "evil.example"
        );
        assert_eq!(host_of("https://evil.example\\@bank.com/"), "evil.example");
        assert_eq!(host_of("https://ex\u{202e}ample.com/"), "example.com");
        assert_eq!(host_of("https://example.com"), "example.com");
    }

    #[test]
    fn device_names_are_refused_in_any_case_extension_and_folder() {
        for name in [
            "CON",
            "con.md",
            "Nul.xlsx",
            "aux.txt.md",
            "PRN",
            "com1.md",
            "LPT9.xlsx",
            "COM\u{b9}.md",
            "sub/CON/plan.md",
            "nul .md",
            "CONIN$.md",
            "CON:x",
            "NUL:x.png",
            "notes/CON:stream.md",
            "notes/plan.md:hidden",
            "img/a.png:stream",
            "sub/aux :s.md",
        ] {
            assert_eq!(
                classify_link(name, Some(folder())),
                LinkTarget::Refused,
                "{name}"
            );
            assert_eq!(image_path(name, Some(folder())), None, "{name}");
        }
        for fine in [
            "console.md",
            "com10.md",
            "com0.md",
            "lpt.md",
            "conn/plan.md",
        ] {
            assert!(
                matches!(classify_link(fine, Some(folder())), LinkTarget::File(_)),
                "{fine}"
            );
        }
        assert!(image_path("icons/aux.png", Some(folder())).is_none());
        assert!(image_path("icons/auxiliary.png", Some(folder())).is_some());
    }

    #[test]
    fn percent_decoding_keeps_malformed_escapes_and_invalid_utf8_as_they_are() {
        assert_eq!(percent_decoded("a%20b"), "a b");
        assert_eq!(percent_decoded("100%"), "100%");
        assert_eq!(percent_decoded("%zz"), "%zz");
        assert_eq!(percent_decoded("%FF"), "%FF");
        assert_eq!(percent_decoded("%C3%A9"), "é");
    }
}
