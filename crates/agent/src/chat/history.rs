use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::settings::AgentId;

const MAX_RECORDS: usize = 50;
// Saves come from background tasks; one at a time keeps the temp file and the rename together.
static SAVING: Mutex<()> = Mutex::new(());
const MAX_TITLE_CHARS: usize = 80;

// Metadata only: no message text is kept. It stands in for the agent's own session list when
// the agent cannot list or load sessions, and a conversation resumed from it starts fresh on
// the agent side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub agent: AgentId,
    pub title: String,
    pub started_unix_seconds: u64,
    pub workbook: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("could not read the conversation history: {0}")]
    Read(std::io::Error),
    #[error("the conversation history file is damaged: {0}")]
    Damaged(serde_json::Error),
    #[error("could not write the conversation history: {0}")]
    Write(std::io::Error),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    records: Vec<Record>,
}

fn title_of(first_message: &str) -> String {
    let line = first_message.lines().next().unwrap_or("").trim();
    line.chars().take(MAX_TITLE_CHARS).collect()
}

impl History {
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    // Newest first, for the agent that is active.
    pub fn for_agent<'a>(&'a self, agent: &'a AgentId) -> impl Iterator<Item = &'a Record> {
        self.records
            .iter()
            .filter(move |record| &record.agent == agent)
    }

    pub fn push(&mut self, agent: AgentId, first_message: &str, workbook: &str, now: u64) {
        let title = title_of(first_message);
        if title.is_empty() {
            return;
        }
        self.records.insert(
            0,
            Record {
                agent,
                title,
                started_unix_seconds: now,
                workbook: workbook.chars().take(MAX_TITLE_CHARS).collect(),
            },
        );
        self.records.truncate(MAX_RECORDS);
    }

    pub fn load(path: &Path) -> Result<History, HistoryError> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(HistoryError::Damaged),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(History::default()),
            Err(error) => Err(HistoryError::Read(error)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), HistoryError> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| HistoryError::Write(std::io::Error::other(error)))?;
        let _saving = SAVING
            .lock()
            .map_err(|_| HistoryError::Write(std::io::Error::other("poisoned lock")))?;
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(HistoryError::Write)?;
        }
        // Written beside the target and renamed, so a crash never leaves half a file.
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text).map_err(HistoryError::Write)?;
        std::fs::rename(&temporary, path).map_err(HistoryError::Write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_keep_the_first_line_of_the_first_message_and_nothing_else() {
        let mut history = History::default();
        history.push(
            AgentId::new("a"),
            "  Fix the dates\nsecret second line",
            "Ventas",
            10,
        );
        let record = &history.records()[0];
        assert_eq!(record.title, "Fix the dates");
        assert!(!serde_json::to_string(&history).unwrap().contains("secret"));
    }

    #[test]
    fn newest_comes_first_and_the_list_is_capped() {
        let mut history = History::default();
        for number in 0..60 {
            history.push(AgentId::new("a"), &format!("m{number}"), "w", number);
        }
        assert_eq!(history.records().len(), MAX_RECORDS);
        assert_eq!(history.records()[0].title, "m59");
    }

    #[test]
    fn a_blank_message_is_not_recorded_and_agents_are_listed_apart() {
        let mut history = History::default();
        history.push(AgentId::new("a"), "   ", "w", 1);
        history.push(AgentId::new("a"), "one", "w", 2);
        history.push(AgentId::new("b"), "two", "w", 3);
        assert_eq!(history.records().len(), 2);
        let ids = AgentId::new("a");
        assert_eq!(history.for_agent(&ids).count(), 1);
    }

    #[test]
    fn history_survives_a_round_trip_and_a_missing_file_is_empty() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("sub").join("history.json");
        assert_eq!(History::load(&path).unwrap(), History::default());
        let mut history = History::default();
        history.push(AgentId::new("a"), "hello", "w", 5);
        history.save(&path).unwrap();
        assert_eq!(History::load(&path).unwrap(), history);
    }

    #[test]
    fn a_damaged_file_is_reported_not_replaced_by_an_empty_history() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("history.json");
        std::fs::write(&path, "{ nope").unwrap();
        assert!(matches!(
            History::load(&path),
            Err(HistoryError::Damaged(_))
        ));
    }
}
