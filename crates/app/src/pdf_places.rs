use std::path::{Path, PathBuf};

use crate::files::same_path;
use crate::pdf::PdfPosition;
use crate::session::PdfPlace;

pub const MAX_PLACES: usize = 50;

// Where the user was reading each recent PDF, so one closed and opened again, even after a
// restart, comes back to the same page and zoom. The least recently read are forgotten.
#[derive(Default)]
pub struct PdfPlaces {
    entries: Vec<(PathBuf, PdfPosition)>,
}

impl PdfPlaces {
    pub fn from_records(records: &[PdfPlace]) -> PdfPlaces {
        let start = records.len().saturating_sub(MAX_PLACES);
        PdfPlaces {
            entries: records[start..]
                .iter()
                .map(|record| (PathBuf::from(&record.path), record.position))
                .collect(),
        }
    }

    pub fn records(&self) -> Vec<PdfPlace> {
        self.entries
            .iter()
            .map(|(path, position)| PdfPlace {
                path: path.to_string_lossy().into_owned(),
                position: *position,
            })
            .collect()
    }

    pub fn position_of(&self, path: &Path) -> Option<PdfPosition> {
        self.entries
            .iter()
            .find(|(known, _)| same_path(known, path))
            .map(|(_, position)| *position)
    }

    pub fn remember(&mut self, path: &Path, position: PdfPosition) {
        self.entries.retain(|(known, _)| !same_path(known, path));
        self.entries.push((path.to_path_buf(), position));
        if self.entries.len() > MAX_PLACES {
            self.entries.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::layout::ZoomMode;

    fn at(page: u32) -> PdfPosition {
        PdfPosition {
            page,
            zoom: ZoomMode::FitPage,
        }
    }

    #[test]
    fn a_remembered_place_is_found_again() {
        let mut places = PdfPlaces::default();
        places.remember(Path::new("C:\\docs\\a.pdf"), at(4));
        assert_eq!(
            places.position_of(Path::new("C:\\docs\\a.pdf")),
            Some(at(4))
        );
        assert_eq!(places.position_of(Path::new("C:\\docs\\b.pdf")), None);
    }

    #[test]
    fn remembering_a_file_again_replaces_its_place() {
        let mut places = PdfPlaces::default();
        places.remember(Path::new("a.pdf"), at(1));
        places.remember(Path::new("b.pdf"), at(2));
        places.remember(Path::new("a.pdf"), at(9));
        assert_eq!(places.position_of(Path::new("a.pdf")), Some(at(9)));
        assert_eq!(places.records().len(), 2);
        assert_eq!(places.records()[1].path, "a.pdf");
    }

    #[test]
    fn only_the_most_recent_places_are_kept() {
        let mut places = PdfPlaces::default();
        for number in 0..MAX_PLACES + 5 {
            places.remember(Path::new(&format!("{number}.pdf")), at(1));
        }
        assert_eq!(places.records().len(), MAX_PLACES);
        assert_eq!(places.position_of(Path::new("0.pdf")), None);
        assert!(places.position_of(Path::new("54.pdf")).is_some());
    }

    #[test]
    fn the_places_survive_the_records_of_the_session() {
        let mut places = PdfPlaces::default();
        places.remember(Path::new("a.pdf"), at(3));
        places.remember(Path::new("b.pdf"), at(7));
        let back = PdfPlaces::from_records(&places.records());
        assert_eq!(back.position_of(Path::new("a.pdf")), Some(at(3)));
        assert_eq!(back.position_of(Path::new("b.pdf")), Some(at(7)));
    }

    #[test]
    fn a_session_with_too_many_places_keeps_the_last_ones() {
        let records: Vec<PdfPlace> = (0..MAX_PLACES + 10)
            .map(|number| PdfPlace {
                path: format!("{number}.pdf"),
                position: at(1),
            })
            .collect();
        let places = PdfPlaces::from_records(&records);
        assert_eq!(places.records().len(), MAX_PLACES);
        assert_eq!(places.position_of(Path::new("9.pdf")), None);
        assert!(places.position_of(Path::new("10.pdf")).is_some());
    }
}
