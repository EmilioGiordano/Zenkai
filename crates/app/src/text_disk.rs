use std::path::Path;

// What a tab says about its file on disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiskNotice {
    #[default]
    None,
    ChangedWithEdits,
    Deleted,
}

// A job writing the tab's text: a save to the file or an autosave to the recovery copy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Writing {
    #[default]
    Idle,
    Saving,
    Autosaving,
}

// How the file of a tab restored from a recovery copy stands against the edits in the copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveredFile {
    // The text never had a file, so none can have changed or been deleted.
    Untitled,
    Same,
    Moved,
    Missing,
}

impl RecoveredFile {
    pub fn judge(path: Option<&Path>, moved_on: bool, exists: bool) -> RecoveredFile {
        match (path, moved_on, exists) {
            (None, ..) => RecoveredFile::Untitled,
            (Some(_), false, _) => RecoveredFile::Same,
            (Some(_), true, true) => RecoveredFile::Moved,
            (Some(_), true, false) => RecoveredFile::Missing,
        }
    }

    pub fn notice(self) -> DiskNotice {
        match self {
            RecoveredFile::Untitled | RecoveredFile::Same => DiskNotice::None,
            RecoveredFile::Moved => DiskNotice::ChangedWithEdits,
            RecoveredFile::Missing => DiskNotice::Deleted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recovered_text_without_a_file_never_claims_the_file_was_deleted() {
        let judged = |path: Option<&str>, moved, exists| {
            RecoveredFile::judge(path.map(Path::new), moved, exists).notice()
        };
        assert_eq!(judged(None, true, false), DiskNotice::None);
        assert_eq!(judged(Some("a.md"), false, true), DiskNotice::None);
        assert_eq!(
            judged(Some("a.md"), true, true),
            DiskNotice::ChangedWithEdits
        );
        assert_eq!(judged(Some("a.md"), true, false), DiskNotice::Deleted);
    }

    #[test]
    fn a_file_that_exists_is_never_announced_as_deleted() {
        assert_eq!(
            RecoveredFile::judge(Some(Path::new("a.md")), true, true),
            RecoveredFile::Moved
        );
    }
}
