use crate::arguments::Version;
use crate::folders::Folders;

// Left in `.update\notice` for the app it opens, which shows it, records a restored version so it
// is not offered again on its own, and deletes the file. The app parses exactly these words, one
// notice per line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    Restored(Option<Version>),
    Interrupted,
    NotApplied(Version),
}

impl Notice {
    pub fn text(&self) -> String {
        match self {
            Notice::Restored(Some(version)) => format!("restored {version}"),
            Notice::Restored(None) => "restored".to_owned(),
            Notice::Interrupted => "interrupted".to_owned(),
            Notice::NotApplied(version) => format!("not-applied {version}"),
        }
    }
}

pub fn leave(folders: &Folders, notices: &[Notice]) {
    let text = notices
        .iter()
        .map(Notice::text)
        .collect::<Vec<_>>()
        .join("\n");
    tracing::info!(notices = %text, "leaving notices for the app");
    if let Err(error) = std::fs::write(folders.notice(), text) {
        tracing::error!(%error, "could not leave the notices for the app");
    }
}

#[cfg(test)]
mod tests {
    use super::Notice;
    use crate::arguments::Version;

    #[test]
    fn each_notice_has_the_words_the_app_reads() {
        let version = || Version::parse("0.2.0").unwrap();
        assert_eq!(Notice::Restored(Some(version())).text(), "restored 0.2.0");
        assert_eq!(Notice::Restored(None).text(), "restored");
        assert_eq!(Notice::Interrupted.text(), "interrupted");
        assert_eq!(Notice::NotApplied(version()).text(), "not-applied 0.2.0");
    }
}
