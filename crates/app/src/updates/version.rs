use std::fmt;

const MAX_TAG_BYTES: usize = 24;
// The launcher's `--apply` takes 1 to 5 digits per part; a longer version could never be applied.
const MAX_COMPONENT_DIGITS: usize = 5;

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

pub fn newer_than(newest: Option<Version>, installed: Version) -> Option<Version> {
    newest.filter(|newest| *newest > installed)
}

#[cfg(test)]
mod tests {
    use super::{Version, newer_than};

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
    fn a_part_has_at_most_five_digits_as_the_launcher_accepts() {
        assert_eq!(
            Version::parse_bare("99999.99999.99999"),
            Some(version(99_999, 99_999, 99_999))
        );
        for text in ["100000.0.0", "0.100000.0", "0.0.100000", "123456789.0.0"] {
            assert_eq!(Version::parse_bare(text), None, "{text}");
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
}
