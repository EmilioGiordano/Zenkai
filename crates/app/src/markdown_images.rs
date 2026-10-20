use std::path::{Component, Path, PathBuf};

use crate::files::is_remote_or_device;
use crate::markdown_links::{is_network_style, local_path, names_a_device, scheme_of};

pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_IMAGE_LINKS: usize = 256;
const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

fn has_image_extension(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy().to_ascii_lowercase();
        IMAGE_EXTENSIONS.contains(&extension.as_str())
    })
}

// The targets a document may load images from: those of `![alt](target)` and those of link
// reference definitions (`[label]: target`), which `![alt][label]` points to. A definition
// that no image uses is listed too; it is only ever checked, never loaded.
pub fn image_urls(text: &str) -> Vec<&str> {
    let mut urls = inline_image_urls(text);
    let room = MAX_IMAGE_LINKS.saturating_sub(urls.len());
    urls.extend(definition_urls(text).take(room));
    urls
}

fn definition_urls(text: &str) -> impl Iterator<Item = &str> {
    text.split(['\n', '\r']).filter_map(|line| {
        let rest = line.trim_start_matches(' ').strip_prefix('[')?;
        let (_, after) = rest.split_once("]:")?;
        let target = after.trim_start().split_whitespace().next()?;
        Some(target.trim_matches(['<', '>']))
    })
}

fn inline_image_urls(text: &str) -> Vec<&str> {
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
    fn reference_style_images_are_listed_through_their_definitions() {
        let text = "![logo][brand] and ![x][]\n\n[brand]: img/logo.png \"Brand\"\n  [x]: <img/x.png>\r[note]: https://example.com/page\n";
        let urls = image_urls(text);
        assert_eq!(
            urls,
            ["img/logo.png", "img/x.png", "https://example.com/page"]
        );
        let mixed = "![a](one.png)\n[r]: two.png\n";
        assert_eq!(image_urls(mixed), ["one.png", "two.png"]);
        assert!(image_urls(&"[r]: a.png\n".repeat(1000)).len() <= 256);
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
}
