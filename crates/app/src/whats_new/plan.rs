use super::links::{NoteLink, links_of};
use crate::markdown_restricted;
use crate::updates::{Day, Note, Notes, Status, Version};

pub struct SectionPlan {
    pub version: Version,
    pub published: Option<Day>,
    pub text: Option<String>,
    pub links: Vec<NoteLink>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Checking,
    CheckFailed,
}

pub enum Body {
    Sections {
        sections: Vec<SectionPlan>,
        older_exist: bool,
    },
    UpToDate,
    NoNotes,
    Nothing,
}

// What the tab shows, worked out off the UI thread: the restricted filter and the search for
// links read every note in full.
pub struct Plan {
    pub banner: Option<Version>,
    pub line: Option<Line>,
    pub body: Body,
}

pub fn plan(status: Status, notes: &Notes, installed: Version) -> Plan {
    let sections: Vec<SectionPlan> = notes.newer_than(installed).map(section).collect();
    let banner = match status {
        Status::Available(version) => Some(version),
        _ => None,
    };
    let line = match status {
        Status::Checking => Some(Line::Checking),
        Status::Failed if !sections.is_empty() => Some(Line::CheckFailed),
        _ => None,
    };
    let body = if !sections.is_empty() {
        Body::Sections {
            older_exist: notes.truncated_above(installed),
            sections,
        }
    } else {
        match status {
            Status::UpToDate { .. } => Body::UpToDate,
            Status::Checking => Body::Nothing,
            Status::Unchecked | Status::Available(_) | Status::Failed => Body::NoNotes,
        }
    };
    Plan { banner, line, body }
}

// A note that is empty, too deeply nested or past the size limit has no text, and its
// heading says so; its release page is still there.
fn section(note: &Note) -> SectionPlan {
    let checked = note
        .body
        .as_deref()
        .and_then(markdown_restricted::check)
        .filter(|checked| !checked.text.trim().is_empty());
    SectionPlan {
        version: note.version,
        published: note.published,
        links: links_of(note.version, checked.as_ref().map(|checked| &checked.tree)),
        text: checked.map(|checked| checked.text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::MAX_NOTES;

    fn version(text: &str) -> Version {
        Version::parse_bare(text).unwrap()
    }

    fn note(text: &str, body: Option<&str>) -> Note {
        Note {
            version: version(text),
            published: Day::parse("2026-12-04"),
            body: body.map(str::to_owned),
        }
    }

    fn installed() -> Version {
        version("0.1.0")
    }

    fn sections(plan: &Plan) -> &[SectionPlan] {
        match &plan.body {
            Body::Sections { sections, .. } => sections,
            _ => &[],
        }
    }

    #[test]
    fn the_notes_above_the_installed_version_show_newest_first_with_a_banner() {
        let notes = Notes::from_releases(vec![
            note("0.1.0", Some("installed")),
            note("0.1.1", Some("one")),
            note("0.1.2", Some("two")),
        ]);
        let plan = plan(Status::Available(version("0.1.2")), &notes, installed());
        assert_eq!(plan.banner, Some(version("0.1.2")));
        assert_eq!(plan.line, None);
        let listed: Vec<String> = sections(&plan)
            .iter()
            .map(|section| section.version.to_string())
            .collect();
        assert_eq!(listed, ["0.1.2", "0.1.1"]);
        assert_eq!(sections(&plan)[0].text.as_deref(), Some("two"));
    }

    #[test]
    fn a_version_whose_note_cannot_be_shown_keeps_its_heading_and_release_page() {
        let deep = ">".repeat(40);
        let notes = Notes::from_releases(vec![
            note("0.1.3", None),
            note("0.1.2", Some("  \n")),
            note("0.1.1", Some(&deep)),
        ]);
        let plan = plan(Status::Available(version("0.1.3")), &notes, installed());
        assert_eq!(sections(&plan).len(), 3);
        for section in sections(&plan) {
            assert_eq!(section.text, None);
            assert_eq!(section.links.len(), 1);
        }
    }

    #[test]
    fn the_text_of_a_note_is_cleaned_before_it_is_shown() {
        let notes = Notes::from_releases(vec![note("0.1.1", Some("a\u{202e}b\r\nc"))]);
        let plan = plan(Status::Available(version("0.1.1")), &notes, installed());
        assert_eq!(sections(&plan)[0].text.as_deref(), Some("ab\nc"));
    }

    #[test]
    fn a_failed_check_with_saved_notes_says_so_above_them() {
        let notes = Notes::from_releases(vec![note("0.1.1", Some("one"))]);
        let plan = plan(Status::Failed, &notes, installed());
        assert_eq!(plan.line, Some(Line::CheckFailed));
        assert_eq!(sections(&plan).len(), 1);
        assert_eq!(plan.banner, None);
    }

    #[test]
    fn a_running_check_is_named_whatever_else_is_shown() {
        let plan = plan(Status::Checking, &Notes::default(), installed());
        assert_eq!(plan.line, Some(Line::Checking));
        assert!(matches!(plan.body, Body::Nothing));
        let notes = Notes::from_releases(vec![note("0.1.1", Some("one"))]);
        let plan = super::plan(Status::Checking, &notes, installed());
        assert_eq!(plan.line, Some(Line::Checking));
        assert_eq!(sections(&plan).len(), 1);
    }

    #[test]
    fn nothing_newer_is_up_to_date_and_no_cache_is_no_notes() {
        let empty = Notes::default();
        assert!(matches!(
            plan(Status::UpToDate { at: 5 }, &empty, installed()).body,
            Body::UpToDate
        ));
        for status in [
            Status::Unchecked,
            Status::Failed,
            Status::Available(version("0.2.0")),
        ] {
            let plan = plan(status, &empty, installed());
            assert!(matches!(plan.body, Body::NoNotes));
            assert_eq!(plan.line, None);
        }
    }

    #[test]
    fn older_versions_are_named_only_when_some_were_left_out() {
        let made = |count: usize| {
            Notes::from_releases(
                (1..=count)
                    .map(|patch| note(&format!("0.1.{patch}"), Some("n")))
                    .collect(),
            )
        };
        let older = |count: usize, installed: Version| {
            let plan = plan(
                Status::Available(version("0.1.30")),
                &made(count),
                installed,
            );
            matches!(
                plan.body,
                Body::Sections {
                    older_exist: true,
                    ..
                }
            )
        };
        assert!(older(MAX_NOTES + 1, installed()));
        assert!(!older(MAX_NOTES, installed()));
        assert!(!older(1, installed()));
        assert!(!older(MAX_NOTES + 1, version("0.1.2")));
    }
}
