use std::fmt;

use serde::Deserialize;
use serde::de::{
    DeserializeSeed, Deserializer, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor,
};

const MAX_TAG_BYTES: usize = 24;
const MAX_COMPONENT_DIGITS: usize = 9;
const MAX_RELEASES: usize = 100;

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

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("the response is not a list of releases")]
pub struct NotAReleaseList;

// The list is read as a stream that keeps only the best stable version, never as a tree, and
// only its first MAX_RELEASES elements. An element that is not a release is skipped, so one
// odd release cannot hide a newer one.
pub fn newest_stable(body: &[u8]) -> Result<Option<Version>, NotAReleaseList> {
    let mut scan = Scan::default();
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let read = ReleaseList(&mut scan)
        .deserialize(&mut deserializer)
        .and_then(|()| deserializer.end());
    match read {
        Ok(()) => Ok(scan.newest),
        Err(_) if scan.capped => Ok(scan.newest),
        Err(_) => Err(NotAReleaseList),
    }
}

#[derive(Default)]
struct Scan {
    newest: Option<Version>,
    seen: usize,
    capped: bool,
}

struct ReleaseList<'a>(&'a mut Scan);

impl<'de> DeserializeSeed<'de> for ReleaseList<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for ReleaseList<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a list of releases")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(Element(stable)) = seq.next_element::<Element>()? {
            if self.0.seen == MAX_RELEASES {
                self.0.capped = true;
                return Err(A::Error::custom("more releases than are read"));
            }
            self.0.seen += 1;
            self.0.newest = self.0.newest.max(stable);
        }
        Ok(())
    }
}

// The version of a release that is neither a draft nor a prerelease, if its tag is valid.
struct Element(Option<Version>);

impl<'de> Deserialize<'de> for Element {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Element, D::Error> {
        deserializer.deserialize_any(ElementVisitor)
    }
}

struct ElementVisitor;

macro_rules! not_a_release {
    ($($method:ident($argument:ty)),+) => {
        $(
            fn $method<E>(self, _: $argument) -> Result<Element, E> {
                Ok(Element(None))
            }
        )+
    };
}

impl<'de> Visitor<'de> for ElementVisitor {
    type Value = Element;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a release")
    }

    not_a_release!(
        visit_bool(bool),
        visit_i64(i64),
        visit_u64(u64),
        visit_f64(f64),
        visit_str(&str)
    );

    fn visit_unit<E>(self) -> Result<Element, E> {
        Ok(Element(None))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Element, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Element(None))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Element, A::Error> {
        let mut tag = None;
        let mut draft = false;
        let mut prerelease = false;
        // A flag that is present but not a boolean is not trusted: the release is skipped.
        let mut flags_are_booleans = true;
        while let Some(field) = map.next_key::<Field>()? {
            match field {
                Field::TagName => {
                    if let Scalar::Text(version) = map.next_value()? {
                        tag = version;
                    }
                }
                Field::Draft | Field::Prerelease => {
                    let flag = match map.next_value()? {
                        Scalar::Flag(flag) => flag,
                        _ => {
                            flags_are_booleans = false;
                            false
                        }
                    };
                    if field == Field::Draft {
                        draft = flag;
                    } else {
                        prerelease = flag;
                    }
                }
                Field::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let stable = flags_are_booleans && !draft && !prerelease;
        Ok(Element(tag.filter(|_| stable)))
    }
}

#[derive(PartialEq, Eq)]
enum Field {
    TagName,
    Draft,
    Prerelease,
    Other,
}

impl<'de> Deserialize<'de> for Field {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Field, D::Error> {
        deserializer.deserialize_identifier(FieldVisitor)
    }
}

struct FieldVisitor;

impl Visitor<'_> for FieldVisitor {
    type Value = Field;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a field name")
    }

    fn visit_str<E>(self, name: &str) -> Result<Field, E> {
        Ok(match name {
            "tag_name" => Field::TagName,
            "draft" => Field::Draft,
            "prerelease" => Field::Prerelease,
            _ => Field::Other,
        })
    }
}

// The value of a field: a tag text is parsed on the spot, so a long string is never kept.
enum Scalar {
    Text(Option<Version>),
    Flag(bool),
    Other,
}

impl<'de> Deserialize<'de> for Scalar {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Scalar, D::Error> {
        deserializer.deserialize_any(ScalarVisitor)
    }
}

struct ScalarVisitor;

impl<'de> Visitor<'de> for ScalarVisitor {
    type Value = Scalar;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a value")
    }

    fn visit_str<E>(self, text: &str) -> Result<Scalar, E> {
        Ok(Scalar::Text(Version::parse_tag(text)))
    }

    fn visit_bool<E>(self, flag: bool) -> Result<Scalar, E> {
        Ok(Scalar::Flag(flag))
    }

    fn visit_i64<E>(self, _: i64) -> Result<Scalar, E> {
        Ok(Scalar::Other)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Scalar, E> {
        Ok(Scalar::Other)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Scalar, E> {
        Ok(Scalar::Other)
    }

    fn visit_unit<E>(self) -> Result<Scalar, E> {
        Ok(Scalar::Other)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Scalar, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Scalar::Other)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Scalar, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Scalar::Other)
    }
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
    fn a_flag_that_is_not_a_boolean_skips_the_release() {
        let body = br#"[
            {"tag_name":"v9.0.0","draft":"no"},
            {"tag_name":"v8.0.0","prerelease":null},
            {"tag_name":"v0.2.0","draft":false}
        ]"#;
        assert_eq!(newest_stable(body), Ok(Some(version(0, 2, 0))));
    }

    #[test]
    fn nested_values_in_unknown_fields_are_skipped() {
        let body = br#"[{"assets":[{"a":[1,2,{"b":null}]}],"body":"x","tag_name":"v0.3.0","author":{"id":1}}]"#;
        assert_eq!(newest_stable(body), Ok(Some(version(0, 3, 0))));
    }

    #[test]
    fn only_the_first_hundred_releases_are_read() {
        let mut elements: Vec<String> = (0..super::MAX_RELEASES)
            .map(|_| r#"{"tag_name":"v0.1.1"}"#.to_string())
            .collect();
        elements.push(r#"{"tag_name":"v9.9.9"}"#.to_string());
        let body = format!("[{}]", elements.join(","));
        assert_eq!(newest_stable(body.as_bytes()), Ok(Some(version(0, 1, 1))));
    }

    #[test]
    fn a_huge_tag_text_is_ignored() {
        let body = format!(r#"[{{"tag_name":"v{}"}}]"#, "1".repeat(100_000));
        assert_eq!(newest_stable(body.as_bytes()), Ok(None));
    }

    #[test]
    fn text_after_the_list_is_an_error() {
        assert_eq!(newest_stable(b"[] x"), Err(NotAReleaseList));
    }

    #[test]
    fn hostile_nesting_is_an_error_not_a_crash() {
        let body = format!("[{}]", "[".repeat(10_000));
        assert_eq!(newest_stable(body.as_bytes()), Err(NotAReleaseList));
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
