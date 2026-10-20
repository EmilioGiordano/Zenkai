use std::path::{Component, Path, PathBuf};

pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_IMAGE_LINKS: usize = 256;
const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

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

// `CON`, `NUL`, `COM1` and the like name devices on Windows whatever the extension, in any
// case and in any folder of the path; opening one can block or reach hardware.
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

fn names_a_device(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => is_reserved_device_name(name),
        _ => false,
    })
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
    if is_remote_or_device(&full) || names_a_device(&full) || !openable {
        LinkTarget::Refused
    } else {
        LinkTarget::File(full)
    }
}

fn has_image_extension(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy().to_ascii_lowercase();
        IMAGE_EXTENSIONS.contains(&extension.as_str())
    })
}

// The targets of `![alt](target)` in the text, a bounded number, in one forward pass.
pub fn image_urls(text: &str) -> Vec<&str> {
    let mut urls = Vec::new();
    let mut rest = text;
    while urls.len() < MAX_IMAGE_LINKS {
        let Some(start) = rest.find("![") else {
            break;
        };
        let after = &rest[start + 2..];
        let Some(label_end) = after.find("](") else {
            break;
        };
        let target = &after[label_end + 2..];
        let Some(target_end) = target.find(')') else {
            break;
        };
        urls.push(target[..target_end].trim());
        rest = &target[target_end..];
    }
    urls
}

// Reads the disk, so it runs off the UI thread. The image must be a small picture of a kind
// that cannot carry script, and its real location, after symlinks and junctions, must be
// inside the real folder of the document. The returned path is the one to load.
pub fn vet_image(path: &Path, folder: &Path) -> Option<PathBuf> {
    if !has_image_extension(path) {
        return None;
    }
    let real = path.canonicalize().ok()?;
    let real_folder = folder.canonicalize().ok()?;
    if !real.starts_with(&real_folder) || !has_image_extension(&real) {
        return None;
    }
    let metadata = std::fs::metadata(&real).ok()?;
    (metadata.is_file() && metadata.len() <= MAX_IMAGE_BYTES).then_some(real)
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
    (safe && has_image_extension(&full) && !is_remote_or_device(&full) && !names_a_device(&full))
        .then_some(full)
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
            "vector.svg",
            "page.html",
            "noext",
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
    fn only_picture_formats_without_script_are_images() {
        for name in ["a.png", "a.JPG", "a.jpeg", "a.gif", "a.webp", "a.bmp"] {
            assert!(image_path(name, Some(folder())).is_some(), "{name}");
        }
        for name in ["a.svg", "a.svgz", "a.ico", "a.png.exe", "a.pdf"] {
            assert!(image_path(name, Some(folder())).is_none(), "{name}");
        }
    }

    #[test]
    fn image_targets_are_listed_in_order_and_a_missing_closer_ends_the_scan() {
        let text = "![a](one.png) text ![b](two.png \"t\") ![c](three.png";
        assert_eq!(image_urls(text), ["one.png", "two.png \"t\""]);
        assert!(image_urls(&"![".repeat(2_000_000)).is_empty());
        assert_eq!(image_urls(&"![x](a.png)".repeat(1000)).len(), 256);
    }

    #[test]
    fn a_vetted_image_is_a_small_real_picture_inside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("docs");
        std::fs::create_dir_all(folder.join("img")).unwrap();
        let ok = folder.join("img").join("a.png");
        std::fs::write(&ok, b"png").unwrap();
        let real = vet_image(&ok, &folder).unwrap();
        assert!(real.ends_with("a.png"));
        let svg = folder.join("a.svg");
        std::fs::write(&svg, b"<svg/>").unwrap();
        assert_eq!(vet_image(&svg, &folder), None);
        let big = folder.join("big.png");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_IMAGE_BYTES + 1)
            .unwrap();
        assert_eq!(vet_image(&big, &folder), None);
        let edge = folder.join("edge.png");
        std::fs::File::create(&edge)
            .unwrap()
            .set_len(MAX_IMAGE_BYTES)
            .unwrap();
        assert!(vet_image(&edge, &folder).is_some());
        assert_eq!(vet_image(&folder.join("missing.png"), &folder), None);
        let outside = dir.path().join("outside.png");
        std::fs::write(&outside, b"png").unwrap();
        assert_eq!(vet_image(&outside, &folder), None);
    }

    #[test]
    fn a_link_that_leaves_the_folder_by_a_symlink_is_not_an_image() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("docs");
        std::fs::create_dir_all(&folder).unwrap();
        let secret = dir.path().join("secret.png");
        std::fs::write(&secret, b"png").unwrap();
        let link = folder.join("looks-local.png");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&secret, &link);
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&secret, &link);
        if linked.is_err() {
            return;
        }
        assert_eq!(vet_image(&link, &folder), None);
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
