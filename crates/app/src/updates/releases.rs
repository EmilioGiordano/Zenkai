use std::fmt;

use serde::de::{
    DeserializeSeed, Deserializer, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor,
};

use super::notes::{Day, MAX_NOTE_BYTES, Note, Notes};
use super::version::Version;

const MAX_RELEASES: usize = 100;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("the response is not a list of releases")]
pub struct NotAReleaseList;

// `newest` is the best stable version in the list; `notes` keeps the versions above the
// installed one, which are the ones the What's new tab shows.
#[derive(Debug, PartialEq, Eq)]
pub struct Releases {
    pub newest: Option<Version>,
    pub notes: Notes,
}

// A stream over the first MAX_RELEASES elements; one element that is not a release is
// skipped so it cannot hide a newer one.
pub fn parse(body: &[u8], installed: Version) -> Result<Releases, NotAReleaseList> {
    let mut scan = Scan {
        installed,
        newest: None,
        found: Vec::new(),
        seen: 0,
        capped: false,
    };
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let read = ReleaseList(&mut scan)
        .deserialize(&mut deserializer)
        .and_then(|()| deserializer.end());
    match read {
        Ok(()) => Ok(scan.finish()),
        Err(_) if scan.capped => Ok(scan.finish()),
        Err(_) => Err(NotAReleaseList),
    }
}

struct Scan {
    installed: Version,
    newest: Option<Version>,
    found: Vec<Note>,
    seen: usize,
    capped: bool,
}

impl Scan {
    fn take(&mut self, note: Note) {
        self.newest = self.newest.max(Some(note.version));
        if note.version > self.installed {
            self.found.push(note);
        }
    }

    fn finish(self) -> Releases {
        Releases {
            newest: self.newest,
            notes: Notes::from_releases(self.found),
        }
    }
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
        while let Some(Element(stable)) = seq.next_element_seed(ElementSeed)? {
            if self.0.seen == MAX_RELEASES {
                self.0.capped = true;
                return Err(A::Error::custom("more releases than are read"));
            }
            self.0.seen += 1;
            if let Some(note) = stable {
                self.0.take(note);
            }
        }
        Ok(())
    }
}

// The stable release an element describes, if it describes one.
struct Element(Option<Note>);

struct ElementSeed;

impl<'de> DeserializeSeed<'de> for ElementSeed {
    type Value = Element;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Element, D::Error> {
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
        let mut published = None;
        let mut body = None;
        let mut draft = false;
        let mut prerelease = false;
        // A flag that is present but not a boolean is not trusted: the release is skipped.
        let mut flags_are_booleans = true;
        while let Some(field) = map.next_key::<Field>()? {
            match field {
                Field::TagName => {
                    if let Scalar::Tag(version) = map.next_value_seed(Value(Want::Tag))? {
                        tag = version;
                    }
                }
                Field::PublishedAt => {
                    if let Scalar::Date(day) = map.next_value_seed(Value(Want::Date))? {
                        published = day;
                    }
                }
                Field::Body => {
                    if let Scalar::Body(text) = map.next_value_seed(Value(Want::Body))? {
                        body = text;
                    }
                }
                Field::Draft | Field::Prerelease => {
                    let flag = match map.next_value_seed(Value(Want::Flag))? {
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
        Ok(Element(tag.filter(|_| stable).map(|version| Note {
            version,
            published,
            body,
        })))
    }
}

#[derive(PartialEq, Eq)]
enum Field {
    TagName,
    PublishedAt,
    Body,
    Draft,
    Prerelease,
    Other,
}

impl<'de> serde::Deserialize<'de> for Field {
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
            "published_at" => Field::PublishedAt,
            "body" => Field::Body,
            "draft" => Field::Draft,
            "prerelease" => Field::Prerelease,
            _ => Field::Other,
        })
    }
}

// What a text field is read as, decided by the field it came from: a text is turned into
// what is kept of it on the spot, so a long string that is not kept is never copied.
#[derive(Clone, Copy)]
enum Want {
    Tag,
    Date,
    Body,
    Flag,
}

enum Scalar {
    Tag(Option<Version>),
    Date(Option<Day>),
    Body(Option<String>),
    Flag(bool),
    Other,
}

struct Value(Want);

impl<'de> DeserializeSeed<'de> for Value {
    type Value = Scalar;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Scalar, D::Error> {
        deserializer.deserialize_any(ScalarVisitor(self.0))
    }
}

struct ScalarVisitor(Want);

// A release with no text, or with a text past the limit, has no note; its version still
// counts.
fn kept_body(text: &str) -> Option<String> {
    (!text.trim().is_empty() && text.len() <= MAX_NOTE_BYTES).then(|| text.to_owned())
}

impl<'de> Visitor<'de> for ScalarVisitor {
    type Value = Scalar;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a value")
    }

    fn visit_str<E>(self, text: &str) -> Result<Scalar, E> {
        Ok(match self.0 {
            Want::Tag => Scalar::Tag(Version::parse_tag(text)),
            Want::Date => Scalar::Date(Day::parse(text)),
            Want::Body => Scalar::Body(kept_body(text)),
            Want::Flag => Scalar::Other,
        })
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

#[cfg(test)]
mod tests {
    use super::super::notes::{MAX_NOTE_BYTES, MAX_NOTES};
    use super::super::version::Version;
    use super::{MAX_RELEASES, NotAReleaseList, parse};

    fn version(text: &str) -> Version {
        Version::parse_bare(text).unwrap()
    }

    fn newest(body: &[u8]) -> Result<Option<Version>, NotAReleaseList> {
        parse(body, version("0.0.0")).map(|releases| releases.newest)
    }

    fn listed(body: &[u8], installed: &str) -> Vec<(String, Option<String>)> {
        parse(body, version(installed))
            .unwrap()
            .notes
            .newer_than(version("0.0.0"))
            .map(|note| (note.version.to_string(), note.body.clone()))
            .collect()
    }

    #[test]
    fn drafts_and_prereleases_are_ignored() {
        let body = br#"[
            {"tag_name":"v9.0.0","draft":true,"prerelease":false},
            {"tag_name":"v8.0.0","draft":false,"prerelease":true},
            {"tag_name":"v0.2.0","draft":false,"prerelease":false}
        ]"#;
        assert_eq!(newest(body), Ok(Some(version("0.2.0"))));
    }

    #[test]
    fn the_newest_is_found_whatever_the_order() {
        let body = br#"[{"tag_name":"v0.1.1"},{"tag_name":"v0.3.0"},{"tag_name":"v0.2.5"}]"#;
        assert_eq!(newest(body), Ok(Some(version("0.3.0"))));
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
        assert_eq!(newest(body), Ok(Some(version("0.1.1"))));
    }

    #[test]
    fn a_flag_that_is_not_a_boolean_skips_the_release() {
        let body = br#"[
            {"tag_name":"v9.0.0","draft":"no"},
            {"tag_name":"v8.0.0","prerelease":null},
            {"tag_name":"v0.2.0","draft":false}
        ]"#;
        assert_eq!(newest(body), Ok(Some(version("0.2.0"))));
    }

    #[test]
    fn nested_values_in_unknown_fields_are_skipped() {
        let body = br#"[{"assets":[{"a":[1,2,{"b":null}]}],"body":"x","tag_name":"v0.3.0","author":{"id":1}}]"#;
        assert_eq!(newest(body), Ok(Some(version("0.3.0"))));
    }

    #[test]
    fn only_the_first_hundred_releases_are_read() {
        let mut elements: Vec<String> = (0..MAX_RELEASES)
            .map(|_| r#"{"tag_name":"v0.1.1"}"#.to_string())
            .collect();
        elements.push(r#"{"tag_name":"v9.9.9"}"#.to_string());
        let body = format!("[{}]", elements.join(","));
        assert_eq!(newest(body.as_bytes()), Ok(Some(version("0.1.1"))));
    }

    #[test]
    fn a_huge_tag_text_is_ignored() {
        let body = format!(r#"[{{"tag_name":"v{}"}}]"#, "1".repeat(100_000));
        assert_eq!(newest(body.as_bytes()), Ok(None));
    }

    #[test]
    fn text_after_the_list_is_an_error() {
        assert_eq!(newest(b"[] x"), Err(NotAReleaseList));
    }

    #[test]
    fn hostile_nesting_is_an_error_not_a_crash() {
        let body = format!("[{}]", "[".repeat(10_000));
        assert_eq!(newest(body.as_bytes()), Err(NotAReleaseList));
    }

    #[test]
    fn an_empty_list_has_no_newest() {
        assert_eq!(newest(b"[]"), Ok(None));
    }

    #[test]
    fn a_response_that_is_not_a_list_is_an_error() {
        for body in [&b""[..], b"{}", b"null", b"<html>", b"[1,", b"\xff\xfe"] {
            assert_eq!(newest(body), Err(NotAReleaseList));
        }
    }

    #[test]
    fn the_notes_are_those_above_the_installed_version_newest_first() {
        let body = br#"[
            {"tag_name":"v0.1.1","published_at":"2026-11-20T09:00:00Z","body":"one"},
            {"tag_name":"v0.1.3","published_at":"2026-12-04T09:00:00Z","body":"three"},
            {"tag_name":"v0.1.0","body":"installed"},
            {"tag_name":"v0.1.2","body":"two"}
        ]"#;
        assert_eq!(
            listed(body, "0.1.0"),
            [
                ("0.1.3".to_owned(), Some("three".to_owned())),
                ("0.1.2".to_owned(), Some("two".to_owned())),
                ("0.1.1".to_owned(), Some("one".to_owned())),
            ]
        );
        let releases = parse(body, version("0.1.0")).unwrap();
        let first = releases.notes.newer_than(version("0.0.0")).next().unwrap();
        assert_eq!(first.published.unwrap().to_string(), "2026-12-04");
    }

    #[test]
    fn nothing_is_kept_when_the_installed_version_is_the_newest() {
        let body = br#"[{"tag_name":"v0.1.0","body":"installed"}]"#;
        assert!(listed(body, "0.1.0").is_empty());
        assert_eq!(newest(body), Ok(Some(version("0.1.0"))));
    }

    #[test]
    fn drafts_and_prereleases_leave_no_note() {
        let body = br#"[
            {"tag_name":"v0.2.0","draft":true,"body":"draft"},
            {"tag_name":"v0.1.9","prerelease":true,"body":"pre"},
            {"tag_name":"v0.1.1","body":"stable"}
        ]"#;
        assert_eq!(
            listed(body, "0.1.0"),
            [("0.1.1".to_owned(), Some("stable".to_owned()))]
        );
    }

    #[test]
    fn a_note_over_the_size_limit_is_skipped_but_its_version_still_counts() {
        let big = "x".repeat(MAX_NOTE_BYTES + 1);
        let exact = "y".repeat(MAX_NOTE_BYTES);
        let body = format!(
            r#"[{{"tag_name":"v0.1.2","body":"{big}"}},{{"tag_name":"v0.1.1","body":"{exact}"}}]"#
        );
        let found = listed(body.as_bytes(), "0.1.0");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], ("0.1.2".to_owned(), None));
        assert_eq!(found[1].1.as_deref(), Some(exact.as_str()));
        assert_eq!(newest(body.as_bytes()), Ok(Some(version("0.1.2"))));
    }

    #[test]
    fn a_release_without_text_has_a_version_and_no_note() {
        let body = br#"[
            {"tag_name":"v0.1.4","body":null},
            {"tag_name":"v0.1.3","body":""},
            {"tag_name":"v0.1.2","body":" \n\t"},
            {"tag_name":"v0.1.1"},
            {"tag_name":"v0.1.5","body":12}
        ]"#;
        let found = listed(body, "0.1.0");
        assert_eq!(found.len(), 5);
        assert!(found.iter().all(|(_, text)| text.is_none()));
    }

    #[test]
    fn a_date_that_is_not_one_is_left_out() {
        let body = br#"[{"tag_name":"v0.1.1","published_at":"soon","body":"a"}]"#;
        let releases = parse(body, version("0.1.0")).unwrap();
        let note = releases.notes.newer_than(version("0.0.0")).next().unwrap();
        assert_eq!(note.published, None);
        assert_eq!(note.body.as_deref(), Some("a"));
    }

    #[test]
    fn escapes_in_the_text_are_decoded_and_left_for_the_filter() {
        let body = br#"[{"tag_name":"v0.1.1","body":"a\r\nb \u202e <b>c</b> \u0000"}]"#;
        let found = listed(body, "0.1.0");
        assert_eq!(
            found[0].1.as_deref(),
            Some("a\r\nb \u{202e} <b>c</b> \u{0}")
        );
    }

    #[test]
    fn no_more_than_twenty_notes_are_kept() {
        let elements: Vec<String> = (1..=40)
            .map(|patch| format!(r#"{{"tag_name":"v0.1.{patch}","body":"n"}}"#))
            .collect();
        let body = format!("[{}]", elements.join(","));
        let found = listed(body.as_bytes(), "0.1.0");
        assert_eq!(found.len(), MAX_NOTES);
        assert_eq!(found[0].0, "0.1.40");
    }

    #[test]
    fn a_repeated_version_keeps_the_first_listed() {
        let body = br#"[
            {"tag_name":"v0.1.1","body":"first"},
            {"tag_name":"v0.1.1","body":"second"}
        ]"#;
        assert_eq!(
            listed(body, "0.1.0"),
            [("0.1.1".to_owned(), Some("first".to_owned()))]
        );
    }
}
