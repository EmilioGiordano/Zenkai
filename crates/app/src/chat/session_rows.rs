use chrono::DateTime;
use zenkai_agent::chat::history::History;
use zenkai_agent::chat::state::AgentState;
use zenkai_agent::settings::AgentId;
use zenkai_i18n::t;

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Today,
    ThisWeek,
    Earlier,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Today => t!("chat.sessions.today"),
            Group::ThisWeek => t!("chat.sessions.this_week"),
            Group::Earlier => t!("chat.sessions.earlier"),
        }
    }
}

// Where a row came from decides what picking it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Agent { session: String },
    Local,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub source: Source,
    pub title: String,
    pub meta: String,
    pub age: String,
    pub group: Group,
    pub agent: AgentId,
}

pub fn group_of(age_seconds: u64) -> Group {
    if age_seconds < DAY {
        Group::Today
    } else if age_seconds < 7 * DAY {
        Group::ThisWeek
    } else {
        Group::Earlier
    }
}

pub fn age_label(age_seconds: u64) -> String {
    match age_seconds {
        s if s < MINUTE => t!("chat.age.now").to_string(),
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        s => format!("{}d", s / DAY),
    }
}

fn row(
    source: Source,
    title: &str,
    meta: String,
    started: Option<u64>,
    now: u64,
    agent: &AgentId,
) -> Row {
    let age = started.map_or(u64::MAX, |started| now.saturating_sub(started));
    Row {
        source,
        title: if title.is_empty() {
            t!("chat.sessions.untitled")
        } else {
            title
        }
        .to_string(),
        meta,
        age: if started.is_some() {
            age_label(age)
        } else {
            String::new()
        },
        group: group_of(age),
        agent: agent.clone(),
    }
}

// The agent's own list when it can list sessions, otherwise Zenkai's local metadata.
pub fn rows(
    agent: &AgentId,
    agent_name: &str,
    state: &AgentState,
    history: &History,
    now: u64,
    query: &str,
) -> Vec<Row> {
    let mut rows: Vec<Row> = if state.abilities.list_sessions {
        state
            .past_sessions
            .iter()
            .map(|past| {
                let started = past
                    .updated_at
                    .as_deref()
                    .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                    .and_then(|time| u64::try_from(time.timestamp()).ok());
                row(
                    Source::Agent {
                        session: past.id.clone(),
                    },
                    past.title.as_deref().unwrap_or(""),
                    agent_name.to_string(),
                    started,
                    now,
                    agent,
                )
            })
            .collect()
    } else {
        history
            .for_agent(agent)
            .map(|record| {
                row(
                    Source::Local,
                    &record.title,
                    format!("{agent_name}, {}", record.workbook),
                    Some(record.started_unix_seconds),
                    now,
                    agent,
                )
            })
            .collect()
    };
    let query = query.trim().to_lowercase();
    if !query.is_empty() {
        rows.retain(|row| {
            row.title.to_lowercase().contains(&query) || row.meta.to_lowercase().contains(&query)
        });
    }
    rows.sort_by_key(|row| row.group);
    rows
}

#[cfg(test)]
mod tests {
    use zenkai_agent::chat::state::{Abilities, PastSession, StateChange};

    use super::*;

    fn agent() -> AgentId {
        AgentId::new("claude")
    }

    #[test]
    fn ages_read_short_and_group_by_recency() {
        assert_eq!(age_label(30), "now");
        assert_eq!(age_label(12 * MINUTE), "12m");
        assert_eq!(age_label(5 * HOUR), "5h");
        assert_eq!(age_label(3 * DAY), "3d");
        assert_eq!(group_of(5 * HOUR), Group::Today);
        assert_eq!(group_of(3 * DAY), Group::ThisWeek);
        assert_eq!(group_of(30 * DAY), Group::Earlier);
    }

    #[test]
    fn an_agent_that_can_list_sessions_supplies_the_rows() {
        let mut state = AgentState::default();
        state.apply(StateChange::Abilities(Abilities {
            list_sessions: true,
            load_session: true,
        }));
        state.apply(StateChange::PastSessions(vec![
            PastSession {
                id: "s1".to_string(),
                title: Some("Fix the dates".to_string()),
                updated_at: Some("2026-10-09T10:00:00Z".to_string()),
            },
            PastSession {
                id: "s2".to_string(),
                title: None,
                updated_at: None,
            },
        ]));
        let mut history = History::default();
        history.push(agent(), "ignored local", "w", 1);
        let now = DateTime::parse_from_rfc3339("2026-10-09T10:12:00Z")
            .unwrap()
            .timestamp() as u64;
        let found = rows(&agent(), "Claude", &state, &history, now, "");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].title, "Fix the dates");
        assert_eq!(found[0].age, "12m");
        assert_eq!(found[0].group, Group::Today);
        assert_eq!(
            found[0].source,
            Source::Agent {
                session: "s1".to_string()
            }
        );
        assert_eq!(found[1].title, "Untitled");
        assert_eq!(found[1].group, Group::Earlier);
    }

    #[test]
    fn without_listing_the_local_history_stands_in_for_this_agent_only() {
        let mut history = History::default();
        history.push(agent(), "Chart of sales", "ventas.xlsx", 1_000);
        history.push(AgentId::new("gemini"), "Other agent", "w", 1_000);
        let found = rows(
            &agent(),
            "Claude",
            &AgentState::default(),
            &history,
            1_600,
            "",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source, Source::Local);
        assert_eq!(found[0].meta, "Claude, ventas.xlsx");
        assert_eq!(found[0].age, "10m");
    }

    #[test]
    fn the_search_matches_title_or_workbook_and_groups_come_newest_first() {
        let mut history = History::default();
        history.push(agent(), "Old work", "ventas.xlsx", 0);
        history.push(agent(), "Fresh work", "gastos.xlsx", 10 * DAY);
        let now = 10 * DAY + 60;
        let all = rows(
            &agent(),
            "Claude",
            &AgentState::default(),
            &history,
            now,
            "",
        );
        assert_eq!(all[0].title, "Fresh work");
        let by_workbook = rows(
            &agent(),
            "Claude",
            &AgentState::default(),
            &history,
            now,
            "VENTAS",
        );
        assert_eq!(by_workbook.len(), 1);
        assert_eq!(by_workbook[0].title, "Old work");
    }
}
