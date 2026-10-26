use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::state::ContextUsage;
use crate::settings::AgentId;

const MAX_RECORDS: usize = 50;
const MAX_TITLE_CHARS: usize = 80;
const MAX_SESSIONS: usize = 500;

// Metadata only: no message text is kept. It stands in for the agent's own session list when
// the agent cannot list or load sessions, and a conversation resumed from it starts fresh on
// the agent side.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub agent: AgentId,
    pub title: String,
    pub started_unix_seconds: u64,
    pub workbook: String,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub context: Option<ContextUsage>,
}

pub type RecordId = (AgentId, String, u64, String);

impl Record {
    // What makes two records the same conversation; the session id and the context are
    // annotations added later, never identity.
    pub fn identity(&self) -> RecordId {
        (
            self.agent.clone(),
            self.title.clone(),
            self.started_unix_seconds,
            self.workbook.clone(),
        )
    }
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
    #[serde(default)]
    sessions: Vec<String>,
}

fn title_of(first_message: &str) -> String {
    let line = first_message.lines().next().unwrap_or("").trim();
    line.chars().take(MAX_TITLE_CHARS).collect()
}

impl History {
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn for_agent<'a>(&'a self, agent: &'a AgentId) -> impl Iterator<Item = &'a Record> {
        self.records
            .iter()
            .filter(move |record| &record.agent == agent)
    }

    pub fn push(&mut self, agent: AgentId, first_message: &str, workbook: &str, now: u64) -> bool {
        let title = title_of(first_message);
        if title.is_empty() {
            return false;
        }
        self.records.insert(
            0,
            Record {
                agent,
                title,
                started_unix_seconds: now,
                workbook: workbook.chars().take(MAX_TITLE_CHARS).collect(),
                session: None,
                context: None,
            },
        );
        self.records.truncate(MAX_RECORDS);
        true
    }

    pub fn newest_for(&self, agent: &AgentId) -> Option<&Record> {
        self.records.iter().find(|record| &record.agent == agent)
    }

    pub fn attach_session_to(&mut self, identity: &RecordId, session: &str) -> Option<Record> {
        let record = self
            .records
            .iter_mut()
            .find(|record| record.session.is_none() && record.identity() == *identity)?;
        record.session = Some(session.to_string());
        Some(record.clone())
    }

    pub fn note_context(&mut self, session: &str, usage: ContextUsage) -> Option<Record> {
        let record = self
            .records
            .iter_mut()
            .find(|record| record.session.as_deref() == Some(session))?;
        record.context = Some(usage);
        Some(record.clone())
    }

    pub fn record_for_session(&self, session: &str) -> Option<&Record> {
        self.records
            .iter()
            .find(|record| record.session.as_deref() == Some(session))
    }

    pub fn record_size(record: &Record) -> u64 {
        serde_json::to_vec(record)
            .map(|bytes| bytes.len() as u64)
            .unwrap_or(0)
    }

    pub fn remember_session(&mut self, id: &str) {
        if self.sessions.iter().any(|known| known == id) {
            return;
        }
        self.sessions.insert(0, id.to_string());
        self.sessions.truncate(MAX_SESSIONS);
    }

    pub fn started_sessions(&self) -> BTreeSet<String> {
        self.sessions.iter().cloned().collect()
    }

    pub fn load(path: &Path) -> Result<History, HistoryError> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(HistoryError::Damaged),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(History::default()),
            Err(error) => Err(HistoryError::Read(error)),
        }
    }

    // A damaged file is kept as .bak for the user and replaced by an empty history, so the
    // next save never overwrites the only copy.
    pub fn load_or_quarantine(path: &Path) -> Result<History, HistoryError> {
        match History::load(path) {
            Err(HistoryError::Damaged(error)) => {
                tracing::warn!(%error, "conversation history is damaged; keeping it as .bak");
                let aside = zenkai_types::aside::free_aside_name(path, "bak").ok_or_else(|| {
                    HistoryError::Write(std::io::Error::other("no free name for the .bak copy"))
                })?;
                std::fs::rename(path, aside).map_err(HistoryError::Write)?;
                Ok(History::default())
            }
            other => other,
        }
    }

    // Two records with different session ids are different conversations sharing
    // metadata, so both are kept.
    pub fn merge(&mut self, loaded: History) {
        for record in loaded.records {
            match self.records.iter_mut().find(|known| {
                known.identity() == record.identity()
                    && (known.session == record.session
                        || known.session.is_none()
                        || record.session.is_none())
            }) {
                Some(known) => {
                    if known.session.is_none() {
                        known.session = record.session.clone();
                    }
                    if known.context.is_none() {
                        known.context = record.context;
                    }
                }
                None => self.records.push(record),
            }
        }
        self.records
            .sort_by_key(|record| std::cmp::Reverse(record.started_unix_seconds));
        self.records.truncate(MAX_RECORDS);
        for id in loaded.sessions {
            if !self.sessions.contains(&id) {
                self.sessions.push(id);
            }
        }
        self.sessions.truncate(MAX_SESSIONS);
    }

    pub fn save(&self, path: &Path) -> Result<(), HistoryError> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| HistoryError::Write(std::io::Error::other(error)))?;
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
    fn a_damaged_file_is_kept_as_a_backup_and_the_history_starts_empty() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("history.json");
        std::fs::write(&path, "{ nope").unwrap();
        assert_eq!(
            History::load_or_quarantine(&path).unwrap(),
            History::default()
        );
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.bak")).unwrap(),
            "{ nope"
        );
    }

    #[test]
    fn a_second_damage_keeps_the_first_quarantined_copy() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("history.json");
        std::fs::write(&path, "{ first").unwrap();
        History::load_or_quarantine(&path).unwrap();
        std::fs::write(&path, "{ second").unwrap();
        History::load_or_quarantine(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(folder.path().join("history.json.bak")).unwrap(),
            "{ first"
        );
        assert_eq!(
            std::fs::read_to_string(folder.path().join("history.json.1.bak")).unwrap(),
            "{ second"
        );
    }

    #[test]
    fn a_failed_set_aside_reports_the_error_and_leaves_the_damaged_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("history.json");
        std::fs::write(&path, "{ damaged").unwrap();
        let taken = zenkai_types::aside::ASIDE_ATTEMPTS;
        for attempt in 0..taken {
            let name = match attempt {
                0 => "history.json.bak".to_string(),
                _ => format!("history.json.{attempt}.bak"),
            };
            std::fs::write(folder.path().join(name), "earlier").unwrap();
        }
        assert!(matches!(
            History::load_or_quarantine(&path),
            Err(HistoryError::Write(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ damaged");
    }

    #[test]
    fn loaded_records_merge_with_newer_ones_without_duplicates() {
        let mut older = History::default();
        older.push(AgentId::new("a"), "old", "w", 1);
        let mut current = History::default();
        current.push(AgentId::new("a"), "new", "w", 9);
        current.merge(older.clone());
        current.merge(older);
        let titles: Vec<&str> = current.records().iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["new", "old"]);
    }

    #[test]
    fn started_sessions_are_kept_once_and_merge_with_the_loaded_ones() {
        let mut older = History::default();
        older.remember_session("old");
        let mut current = History::default();
        current.remember_session("new");
        current.remember_session("new");
        current.merge(older);
        let started = current.started_sessions();
        assert_eq!(started.len(), 2);
        assert!(started.contains("old") && started.contains("new"));
    }

    #[test]
    fn a_history_written_before_sessions_were_kept_still_loads() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("history.json");
        std::fs::write(&path, r#"{"records": []}"#).unwrap();
        assert!(History::load(&path).unwrap().started_sessions().is_empty());
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

    #[test]
    fn context_is_stored_with_the_session_and_survives_a_round_trip() {
        let mut history = History::default();
        history.push(AgentId::new("a"), "hello", "w", 5);
        let usage = ContextUsage {
            used: 1_000,
            size: 8_000,
        };
        assert!(history.note_context("s1", usage).is_none());
        let identity = history.records()[0].identity();
        history.attach_session_to(&identity, "s1").unwrap();
        assert_eq!(
            history.note_context("s1", usage).unwrap().context,
            Some(usage)
        );
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("history.json");
        history.save(&path).unwrap();
        let loaded = History::load(&path).unwrap();
        let record = &loaded.records()[0];
        assert_eq!(record.session.as_deref(), Some("s1"));
        assert_eq!(record.context, Some(usage));
        assert_eq!(
            loaded.record_for_session("s1").unwrap().context,
            Some(usage)
        );
    }

    #[test]
    fn a_history_written_before_context_and_session_still_loads() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("history.json");
        std::fs::write(
            &path,
            r#"{"records": [{"agent": "a", "title": "old", "started_unix_seconds": 1, "workbook": "w"}], "sessions": []}"#,
        )
        .unwrap();
        let loaded = History::load(&path).unwrap();
        let record = &loaded.records()[0];
        assert_eq!(record.session, None);
        assert_eq!(record.context, None);
        assert!(loaded.record_for_session("s1").is_none());
    }

    #[test]
    fn session_ids_link_their_own_record_when_starts_arrive_out_of_order() {
        let mut history = History::default();
        history.push(AgentId::new("a"), "first", "w", 1);
        let first = history.records()[0].identity();
        history.push(AgentId::new("a"), "second", "w", 2);
        let second = history.records()[0].identity();
        history.attach_session_to(&second, "s2").unwrap();
        history.attach_session_to(&first, "s1").unwrap();
        assert_eq!(history.record_for_session("s2").unwrap().title, "second");
        assert_eq!(history.record_for_session("s1").unwrap().title, "first");
        assert!(history.attach_session_to(&first, "s3").is_none());
    }

    #[test]
    fn merging_fills_the_annotations_it_lacks_without_duplicating() {
        let mut current = History::default();
        current.push(AgentId::new("a"), "live", "w", 1);
        let live = current.records()[0].identity();
        current.attach_session_to(&live, "s1").unwrap();
        let usage = ContextUsage {
            used: 500,
            size: 1_000,
        };
        let mut older = History::default();
        older.push(AgentId::new("a"), "live", "w", 1);
        let live = older.records()[0].identity();
        older.attach_session_to(&live, "s1").unwrap();
        older.note_context("s1", usage);
        current.merge(older);
        assert_eq!(current.records().len(), 1);
        let record = &current.records()[0];
        assert_eq!(record.session.as_deref(), Some("s1"));
        assert_eq!(record.context, Some(usage));
        current.merge(History::default());
        assert_eq!(current.records().len(), 1);
    }

    #[test]
    fn merging_keeps_records_with_different_session_ids() {
        let mut current = History::default();
        current.push(AgentId::new("a"), "same", "w", 1);
        let live = current.records()[0].identity();
        current.attach_session_to(&live, "s1").unwrap();
        let mut older = History::default();
        older.push(AgentId::new("a"), "same", "w", 1);
        let live = older.records()[0].identity();
        older.attach_session_to(&live, "s2").unwrap();
        current.merge(older);
        assert_eq!(current.records().len(), 2);
        assert_eq!(current.record_for_session("s1").unwrap().title, "same");
        assert_eq!(current.record_for_session("s2").unwrap().title, "same");
    }

    #[test]
    fn the_size_is_the_bytes_of_the_serialized_entry() {
        let mut history = History::default();
        history.push(AgentId::new("a"), "short", "w", 1);
        history.push(AgentId::new("a"), "a much longer title", "w", 2);
        let small = History::record_size(&history.records()[1]);
        let big = History::record_size(&history.records()[0]);
        assert!(small > 0);
        assert!(big > small);
        assert_eq!(
            big,
            serde_json::to_string(&history.records()[0]).unwrap().len() as u64
        );
    }
}
