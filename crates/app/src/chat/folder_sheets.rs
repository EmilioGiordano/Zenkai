use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use zenkai_engine::{ListedSheet, sheet_names_of};

use super::mention_candidates::MAX_SHEETS_PER_BOOK;
use super::mention_catalog::Catalog;
use super::name_guard::{MAX_SHEET_NAME_CHARS, can_hold_external_sheet, is_safe_name};

pub(super) const MAX_FILES_PER_QUERY: usize = 8;
const CACHE_ENTRIES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileKey {
    path: PathBuf,
    modified: Option<SystemTime>,
    size: u64,
}

impl FileKey {
    fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path)
            .inspect_err(|error| {
                tracing::debug!(%error, path = %path.display(), "could not stat a workbook");
            })
            .ok()?;
        Some(Self {
            path: path.to_path_buf(),
            modified: metadata.modified().ok(),
            size: metadata.len(),
        })
    }
}

// Sheet names by file identity, so an edited file is read again. The oldest entry leaves first.
#[derive(Clone, Debug, Default)]
pub(super) struct SheetNameCache {
    entries: VecDeque<(FileKey, Vec<String>)>,
}

impl SheetNameCache {
    fn get(&self, key: &FileKey) -> Option<&Vec<String>> {
        self.entries
            .iter()
            .find(|(known, _)| known == key)
            .map(|(_, names)| names)
    }

    pub(super) fn insert(&mut self, key: FileKey, names: Vec<String>) {
        self.entries.retain(|(known, _)| known.path != key.path);
        self.entries.push_back((key, names));
        if self.entries.len() > CACHE_ENTRIES {
            self.entries.pop_front();
        }
    }
}

pub(super) struct Fetched {
    pub file: String,
    pub key: FileKey,
    pub names: Vec<String>,
}

// Folder-only workbooks whose name matches, best first: a file name that starts with the query,
// then the shorter path. An empty query lists no sheets, so opening the picker reads no file.
pub(super) fn best_matches(query: &str, catalog: &Catalog) -> Vec<String> {
    let wanted = query.trim_matches('\'').to_lowercase();
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<&String> = catalog
        .folder_only()
        .filter(|file| can_hold_external_sheet(file) && file.to_lowercase().contains(&wanted))
        .collect();
    hits.sort_by_key(|file| {
        let name = file.rsplit('/').next().unwrap_or(file);
        (!name.to_lowercase().starts_with(&wanted), file.len())
    });
    hits.into_iter()
        .take(MAX_FILES_PER_QUERY)
        .cloned()
        .collect()
}

pub(super) fn visible_safe_names(listed: Vec<ListedSheet>) -> Vec<String> {
    listed
        .into_iter()
        .filter(|sheet| {
            !sheet.hidden
                && sheet.name.chars().take(MAX_SHEET_NAME_CHARS + 1).count() <= MAX_SHEET_NAME_CHARS
                && is_safe_name(&sheet.name)
        })
        .map(|sheet| sheet.name)
        .take(MAX_SHEETS_PER_BOOK)
        .collect()
}

// Reads files, so it runs off the UI thread. An unreadable file yields no sheets and is not
// read again until it changes.
pub(super) fn fetch(folder: &Path, files: Vec<String>, cache: &SheetNameCache) -> Vec<Fetched> {
    files
        .into_iter()
        .filter_map(|file| {
            let path = folder.join(&file);
            let key = FileKey::of(&path)?;
            let names = match cache.get(&key) {
                Some(names) => names.clone(),
                None => match sheet_names_of(&path) {
                    Ok(listed) => visible_safe_names(listed),
                    Err(error) => {
                        tracing::debug!(%error, file, "could not list the sheets of a workbook");
                        Vec::new()
                    }
                },
            };
            Some(Fetched { file, key, names })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::mention_catalog::sample::catalog;
    use super::*;

    fn listed(name: &str, hidden: bool) -> ListedSheet {
        ListedSheet {
            name: name.to_string(),
            hidden,
        }
    }

    fn key(path: &str, size: u64, seconds: u64) -> FileKey {
        FileKey {
            path: PathBuf::from(path),
            modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(seconds)),
            size,
        }
    }

    #[test]
    fn hidden_and_unsafe_sheets_are_skipped_and_the_list_is_capped() {
        let mut sheets = vec![
            listed("Data", false),
            listed("Secret", true),
            listed("evil\nname", false),
            listed("rtl\u{202E}x", false),
            listed(&"x".repeat(5000), false),
            listed(&"y".repeat(MAX_SHEET_NAME_CHARS), false),
        ];
        sheets.extend((0..30).map(|n| listed(&format!("s{n}"), false)));
        let names = visible_safe_names(sheets);
        assert_eq!(names.len(), MAX_SHEETS_PER_BOOK);
        assert_eq!(
            &names[..2],
            ["Data", "y".repeat(MAX_SHEET_NAME_CHARS).as_str()]
        );
        assert!(
            names
                .iter()
                .all(|name| name.chars().count() <= MAX_SHEET_NAME_CHARS)
        );
    }

    #[test]
    fn only_folder_workbooks_that_match_are_read_and_at_most_a_few() {
        let mut catalog = catalog();
        assert_eq!(best_matches("archive", &catalog), ["sales-archive.xlsx"]);
        assert!(best_matches("q3", &catalog).is_empty());
        assert!(best_matches("", &catalog).is_empty());
        catalog.folder = (0..20).map(|n| format!("plan {n}.xlsx")).collect();
        catalog.folder.push("plan.xlsx".to_string());
        catalog.folder.push("a [b] plan.xlsx".to_string());
        let found = best_matches("plan", &catalog);
        assert_eq!(found.len(), MAX_FILES_PER_QUERY);
        assert_eq!(found[0], "plan.xlsx");
        assert!(found.iter().all(|file| !file.contains('[')));
    }

    #[test]
    fn a_changed_modified_time_or_size_is_a_different_cache_key() {
        let mut cache = SheetNameCache::default();
        cache.insert(key("a.xlsx", 10, 1), vec!["One".to_string()]);
        assert!(cache.get(&key("a.xlsx", 10, 1)).is_some());
        assert!(cache.get(&key("a.xlsx", 10, 2)).is_none());
        assert!(cache.get(&key("a.xlsx", 11, 1)).is_none());
    }

    #[test]
    fn the_cache_is_bounded_and_keeps_one_entry_per_file() {
        let mut cache = SheetNameCache::default();
        for n in 0..(CACHE_ENTRIES + 10) {
            cache.insert(key(&format!("f{n}.xlsx"), 1, 1), Vec::new());
        }
        assert_eq!(cache.entries.len(), CACHE_ENTRIES);
        assert!(cache.get(&key("f0.xlsx", 1, 1)).is_none());
        assert!(cache.get(&key("f73.xlsx", 1, 1)).is_some());
        cache.insert(key("f73.xlsx", 2, 1), Vec::new());
        assert_eq!(cache.entries.len(), CACHE_ENTRIES);
        assert!(cache.get(&key("f73.xlsx", 1, 1)).is_none());
    }
}
