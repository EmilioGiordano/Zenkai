use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::values::{Stop, ValuesError};

const EVENTS_PER_STOP_CHECK: u32 = 4096;

// calamine 0.36.1 loops forever when an .ods content.xml ends inside a table, so only
// content.xml files whose elements all close are handed to it.
pub(crate) fn check_content_ends_cleanly(path: &Path, stop: &Stop<'_>) -> Result<(), ValuesError> {
    let file = File::open(path).map_err(|e| ValuesError::Unreadable(e.to_string()))?;
    let Ok(mut archive) = zip::ZipArchive::new(BufReader::new(file)) else {
        return Ok(());
    };
    let Ok(content) = archive.by_name("content.xml") else {
        return Ok(());
    };
    let mut reader = Reader::from_reader(BufReader::new(content));
    let mut buf = Vec::new();
    let mut open_elements = 0usize;
    let mut events = 0u32;
    loop {
        events = events.wrapping_add(1);
        if events % EVENTS_PER_STOP_CHECK == 0 {
            stop.check()?;
        }
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => open_elements += 1,
            Ok(Event::End(_)) => {
                open_elements = open_elements.checked_sub(1).ok_or_else(unbalanced)?;
            }
            Ok(Event::Eof) if open_elements == 0 => return Ok(()),
            Ok(Event::Eof) => return Err(unbalanced()),
            Ok(_) => {}
            Err(e) => return Err(ValuesError::Unreadable(format!("content.xml: {e}"))),
        }
        buf.clear();
    }
}

fn unbalanced() -> ValuesError {
    ValuesError::Unreadable("content.xml has elements that never close".to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::AtomicBool;

    use super::*;

    fn ods_with_content(content: &str) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(file.reopen().unwrap());
        zip.start_file("content.xml", zip::write::FileOptions::default())
            .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
        zip.finish().unwrap();
        file
    }

    fn check(content: &str) -> Result<(), ValuesError> {
        let file = ods_with_content(content);
        let cancel = AtomicBool::new(false);
        check_content_ends_cleanly(file.path(), &Stop::new(&cancel))
    }

    #[test]
    fn a_table_that_closes_passes() {
        check("<doc><table><row/></table></doc>").unwrap();
    }

    #[test]
    fn content_cut_inside_a_table_is_refused() {
        assert!(check("<doc><table><row/>").is_err());
    }

    #[test]
    fn an_end_tag_that_matches_nothing_is_refused() {
        assert!(check("<doc></table></doc>").is_err());
    }
}
