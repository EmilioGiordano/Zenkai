use std::fmt;

use serde::Deserialize;

const MAX_TAG_BYTES: usize = 24;
const MAX_COMPONENT_DIGITS: usize = 9;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Version {
    pub fn installed() -> Option<Version> {
        Version::parse_bare(env!("CARGO_PKG_VERSION"))
    }

    pub fn parse_tag(tag: &str) -> Option<Version> {
        if tag.len() > MAX_TAG_BYTES {
            return None;
        }
        Version::parse_bare(tag.strip_prefix('v')?)
    }

    pub fn parse_bare(text: &str) -> Option<Version> {
        let mut parts = text.split('.');
        let version = Version {
            major: component(parts.next()?)?,
            minor: component(parts.next()?)?,
            patch: component(parts.next()?)?,
        };
        parts.next().is_none().then_some(version)
    }
}

// Digits only and no leading zero, so "+1", "01" and non-ASCII digits never parse.
fn component(text: &str) -> Option<u32> {
    let plain = !text.is_empty()
        && text.len() <= MAX_COMPONENT_DIGITS
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    if plain { text.parse().ok() } else { None }
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: Option<String>,
    draft: Option<bool>,
    prerelease: Option<bool>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("the response is not a list of releases")]
pub struct NotAReleaseList;

// A malformed element is skipped instead of failing the list, so one odd release can never
// hide a newer one.
pub fn newest_stable(body: &[u8]) -> Result<Option<Version>, NotAReleaseList> {
    let elements: Vec<serde_json::Value> =
        serde_json::from_slice(body).map_err(|_| NotAReleaseList)?;
    Ok(elements
        .into_iter()
        .filter_map(|element| serde_json::from_value::<RawRelease>(element).ok())
        .filter(|release| release.draft != Some(true) && release.prerelease != Some(true))
        .filter_map(|release| Version::parse_tag(&release.tag_name?))
        .max())
}

pub fn newer_than(newest: Option<Version>, installed: Version) -> Option<Version> {
    newest.filter(|newest| *newest > installed)
}

#[cfg(test)]
mod tests {
    use super::{NotAReleaseList, Version, newer_than, newest_stable};

    fn version(major: u32, minor: u32, patch: u32) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    #[test]
    fn a_strict_tag_parses() {
        assert_eq!(Version::parse_tag("v0.1.1"), Some(version(0, 1, 1)));
        assert_eq!(Version::parse_tag("v10.20.30"), Some(version(10, 20, 30)));
    }

    #[test]
    fn hostile_tags_do_not_parse() {
        for tag in [
            "",
            "v",
            "0.1.1",
            "V0.1.1",
            "v1",
            "v1.2",
            "v1.2.3.4",
            "v1.2.3-rc1",
            "v1.2.3+build",
            "v01.2.3",
            "v1.02.3",
            "v+1.2.3",
            "v-1.2.3",
            "v1.2.3 ",
            " v1.2.3",
            "v1..3",
            "v1.2.",
            "v1.2.3\n",
            "v1.2.\u{0663}",
            "v99999999999.0.0",
            "v4294967296.0.0",
            "v1.2.3\0",
        ] {
            assert_eq!(Version::parse_tag(tag), None, "{tag:?}");
        }
    }

    #[test]
    fn a_tag_past_the_length_limit_is_ignored() {
        let long = format!("v1.2.{}", "0".repeat(100));
        assert_eq!(Version::parse_tag(&long), None);
        assert_eq!(Version::parse_tag(&"9".repeat(1_000_000)), None);
    }

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert!(version(0, 10, 0) > version(0, 9, 0));
        assert!(version(1, 0, 0) > version(0, 99, 99));
        assert!(version(0, 1, 2) > version(0, 1, 1));
    }

    #[test]
    fn the_installed_version_parses() {
        assert!(Version::installed().is_some());
    }

    #[test]
    fn a_version_prints_without_the_prefix() {
        assert_eq!(version(0, 1, 1).to_string(), "0.1.1");
    }

    #[test]
    fn only_a_strictly_greater_version_counts() {
        let installed = version(0, 1, 0);
        assert_eq!(
            newer_than(Some(version(0, 1, 1)), installed),
            Some(version(0, 1, 1))
        );
        assert_eq!(newer_than(Some(installed), installed), None);
        assert_eq!(newer_than(Some(version(0, 0, 9)), installed), None);
        assert_eq!(newer_than(None, installed), None);
    }

    #[test]
    fn drafts_and_prereleases_are_ignored() {
        let body = br#"[
            {"tag_name":"v9.0.0","draft":true,"prerelease":false},
            {"tag_name":"v8.0.0","draft":false,"prerelease":true},
            {"tag_name":"v0.2.0","draft":false,"prerelease":false}
        ]"#;
        assert_eq!(newest_stable(body), Ok(Some(version(0, 2, 0))));
    }

    #[test]
    fn the_newest_is_found_whatever_the_order() {
        let body = br#"[{"tag_name":"v0.1.1"},{"tag_name":"v0.3.0"},{"tag_name":"v0.2.5"}]"#;
        assert_eq!(newest_stable(body), Ok(Some(version(0, 3, 0))));
    }

    #[test]
    fn odd_elements_are_skipped_and_do_not_hide_an_update() {
        let body = br#"[
            42,
            null,
            {"tag_name":123},
            {"tag_name":"latest"},
            {"tag_name":"v1.0.0-beta"},
            {"name":"no tag"},
            {"tag_name":"v0.1.1"}
        ]"#;
        assert_eq!(newest_stable(body), Ok(Some(version(0, 1, 1))));
    }

    #[test]
    fn an_empty_list_has_no_newest() {
        assert_eq!(newest_stable(b"[]"), Ok(None));
    }

    #[test]
    fn a_response_that_is_not_a_list_is_an_error() {
        for body in [&b""[..], b"{}", b"null", b"<html>", b"[1,", b"\xff\xfe"] {
            assert_eq!(newest_stable(body), Err(NotAReleaseList));
        }
    }
}
