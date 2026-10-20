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

fn scheme_of(url: &str) -> Option<&str> {
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

fn local_path(url: &str) -> Option<PathBuf> {
    let without_fragment = url.split(['#', '?']).next().unwrap_or_default();
    let decoded = percent_decoded(without_fragment);
    (!decoded.is_empty())
        .then(|| PathBuf::from(decoded.replace('/', std::path::MAIN_SEPARATOR_STR)))
}

fn is_network_style(url: &str) -> bool {
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
    if is_remote_or_device(&full) || !openable {
        LinkTarget::Refused
    } else {
        LinkTarget::File(full)
    }
}

// Only a file inside the document's own folder: remote addresses, absolute paths, network
// shares and `..` escapes never load, so opening a document cannot reach out or read elsewhere.
pub fn image_path(url: &str, folder: Option<&Path>) -> Option<PathBuf> {
    let url = url.trim();
    if scheme_of(url).is_some() || is_network_style(url) {
        return None;
    }
    let relative = local_path(url)?;
    let safe = relative
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    let full = folder?.join(relative);
    (safe && !is_remote_or_device(&full)).then_some(full)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn images_load_only_from_inside_the_documents_folder() {
        assert_eq!(
            image_path("img/a.png", Some(folder())),
            Some(folder().join("img").join("a.png"))
        );
        assert_eq!(
            image_path("./a%20b.png", Some(folder())),
            Some(folder().join(".").join("a b.png"))
        );
        for refused in [
            "https://x.y/a.png",
            "http://x.y/a.png",
            "//x.y/a.png",
            "../secret.png",
            "img/../../secret.png",
            "data:image/png;base64,AAAA",
            "file:///C:/a.png",
            r"\\host\share\a.png",
            "/etc/a.png",
        ] {
            assert_eq!(image_path(refused, Some(folder())), None, "{refused}");
        }
        assert_eq!(image_path("a.png", None), None);
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
