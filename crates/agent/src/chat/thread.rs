use std::fmt;
use std::path::PathBuf;

use zenkai_i18n::t;

const MAX_DETAIL_CHARS: usize = 600;
const MAX_TITLE_CHARS: usize = 200;
const MAX_MESSAGE_CHARS: usize = 500_000;
const MAX_ENTRIES: usize = 2_000;

fn trimmed_notice() -> &'static str {
    t!("chat.trimmed")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MessageId(u64);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ToolCallId(String);

impl ToolCallId {
    pub fn new(id: impl Into<String>) -> ToolCallId {
        ToolCallId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    WaitingForPermission,
    InProgress,
    Completed,
    Failed,
    Rejected,
    Canceled,
}

impl ToolStatus {
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            ToolStatus::Completed
                | ToolStatus::Failed
                | ToolStatus::Rejected
                | ToolStatus::Canceled
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            ToolStatus::Pending => t!("chat.tool.starting"),
            ToolStatus::WaitingForPermission => t!("chat.tool.waiting"),
            ToolStatus::InProgress => t!("chat.tool.running"),
            ToolStatus::Completed => t!("chat.tool.done"),
            ToolStatus::Failed => t!("chat.tool.failed"),
            ToolStatus::Rejected => t!("chat.tool.denied"),
            ToolStatus::Canceled => t!("chat.tool.stopped"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCard {
    pub id: ToolCallId,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolStatus,
    pub detail: String,
    // The raw input or command the agent gave for the call, when it gave one.
    pub input: String,
}

// A file the agent made in its working folder, shown with a button that opens it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCard {
    pub name: String,
    pub place: String,
    pub path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    User(String),
    Assistant { id: MessageId, text: String },
    Tool(ToolCard),
    Worked { seconds: u64 },
    Notice { kind: NoticeKind, text: String },
    File(FileCard),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentUpdate {
    Message(String),
    UserText(String),
    Thought,
    ToolStarted(ToolCard),
    ToolChanged {
        id: ToolCallId,
        title: Option<String>,
        status: Option<ToolStatus>,
        detail: Option<String>,
        input: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnEnd {
    Finished,
    TokenLimit,
    Refused,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TurnState {
    #[default]
    Idle,
    Running,
    Cancelling,
}

// What the screen must do after an update: grow one message's text, or redraw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Appended { message: MessageId, added: String },
    Changed,
    Nothing,
}

#[derive(Clone, Debug, Default)]
pub struct Thread {
    entries: Vec<Entry>,
    turn: TurnState,
    // The assistant message the next chunk continues; a tool call or the end of the turn closes it.
    open_message: Option<usize>,
    next_message: u64,
    // Replayed user text arrives in chunks; they join until something else arrives.
    open_user: bool,
}

pub fn worked_label(seconds: u64) -> String {
    let seconds_text = |count: u64| t!("chat.unit.second", count = count);
    let minutes_text = |count: u64| t!("chat.unit.minute", count = count);
    let time = if seconds < 60 {
        seconds_text(seconds)
    } else if seconds.is_multiple_of(60) {
        minutes_text(seconds / 60)
    } else {
        format!(
            "{} {}",
            minutes_text(seconds / 60),
            seconds_text(seconds % 60)
        )
    };
    t!("chat.worked", time = time)
}

fn capped(text: String, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text;
    }
    let mut shortened: String = text.chars().take(limit).collect();
    shortened.push('…');
    shortened
}

impl Thread {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn turn(&self) -> TurnState {
        self.turn
    }

    pub fn message_text(&self, id: MessageId) -> Option<&str> {
        self.entries.iter().find_map(|entry| match entry {
            Entry::Assistant { id: found, text } if *found == id => Some(text.as_str()),
            _ => None,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    // False while the agent is still answering the previous message.
    pub fn submit(&mut self, text: String) -> bool {
        if self.turn != TurnState::Idle || text.trim().is_empty() {
            return false;
        }
        self.open_message = None;
        self.push_entry(Entry::User(text));
        self.turn = TurnState::Running;
        true
    }

    pub fn request_cancel(&mut self) {
        if self.turn == TurnState::Running {
            self.turn = TurnState::Cancelling;
        }
    }

    pub fn apply(&mut self, update: AgentUpdate) -> Effect {
        if !matches!(update, AgentUpdate::UserText(_)) {
            self.open_user = false;
        }
        match update {
            AgentUpdate::UserText(text) => {
                self.open_message = None;
                match self.entries.last_mut() {
                    Some(Entry::User(last)) if self.open_user => last.push_str(&text),
                    _ => self.push_entry(Entry::User(text)),
                }
                self.open_user = true;
                Effect::Changed
            }
            AgentUpdate::Message(text) => self.append_text(text),
            AgentUpdate::Thought => Effect::Nothing,
            AgentUpdate::ToolStarted(mut card) => {
                self.open_message = None;
                card.title = capped(card.title, MAX_TITLE_CHARS);
                card.detail = capped(card.detail, MAX_DETAIL_CHARS);
                card.input = capped(card.input, MAX_DETAIL_CHARS);
                match self.tool_index(&card.id) {
                    Some(index) => self.entries[index] = Entry::Tool(card),
                    None => self.push_entry(Entry::Tool(card)),
                }
                Effect::Changed
            }
            AgentUpdate::ToolChanged {
                id,
                title,
                status,
                detail,
                input,
            } => {
                let Some(index) = self.tool_index(&id) else {
                    return Effect::Nothing;
                };
                let Entry::Tool(card) = &mut self.entries[index] else {
                    return Effect::Nothing;
                };
                if let Some(title) = title {
                    card.title = capped(title, MAX_TITLE_CHARS);
                }
                // A call that is waiting for the user stays waiting until the answer arrives.
                if let Some(status) = status
                    && card.status != ToolStatus::WaitingForPermission
                {
                    card.status = status;
                }
                if let Some(detail) = detail {
                    card.detail = capped(detail, MAX_DETAIL_CHARS);
                }
                if let Some(input) = input {
                    card.input = capped(input, MAX_DETAIL_CHARS);
                }
                Effect::Changed
            }
        }
    }

    // A flooding agent cannot grow the transcript without limit: the oldest entries go, behind one
    // notice that says so.
    fn push_entry(&mut self, entry: Entry) {
        self.entries.push(entry);
        while self.entries.len() > MAX_ENTRIES {
            let has_notice = matches!(
                self.entries.first(),
                Some(Entry::Notice { text, .. }) if text == trimmed_notice()
            );
            if !has_notice {
                self.entries.insert(
                    0,
                    Entry::Notice {
                        kind: NoticeKind::Info,
                        text: trimmed_notice().to_string(),
                    },
                );
                self.open_message = self.open_message.map(|index| index + 1);
            }
            self.entries.remove(1);
            self.open_message = match self.open_message {
                Some(1) | None => None,
                Some(index) => Some(index - 1),
            };
        }
    }

    fn append_text(&mut self, text: String) -> Effect {
        if text.is_empty() {
            return Effect::Nothing;
        }
        if let Some(index) = self.open_message
            && let Some(Entry::Assistant { id, text: existing }) = self.entries.get_mut(index)
        {
            if existing.len() + text.len() > MAX_MESSAGE_CHARS {
                return Effect::Nothing;
            }
            existing.push_str(&text);
            return Effect::Appended {
                message: *id,
                added: text,
            };
        }
        let message = MessageId(self.next_message);
        self.next_message += 1;
        self.push_entry(Entry::Assistant {
            id: message,
            text: text.clone(),
        });
        self.open_message = Some(self.entries.len() - 1);
        Effect::Appended {
            message,
            added: text,
        }
    }

    fn tool_index(&self, id: &ToolCallId) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| matches!(entry, Entry::Tool(card) if &card.id == id))
    }

    // The agent asked the user about a call; the call is created if its first update has
    // not arrived yet.
    pub fn waiting_for_permission(&mut self, id: &ToolCallId, title: &str) {
        match self.tool_index(id) {
            Some(index) => {
                if let Entry::Tool(card) = &mut self.entries[index] {
                    card.status = ToolStatus::WaitingForPermission;
                }
            }
            None => {
                self.open_message = None;
                self.push_entry(Entry::Tool(ToolCard {
                    id: id.clone(),
                    title: title.to_string(),
                    kind: ToolKind::Other,
                    status: ToolStatus::WaitingForPermission,
                    detail: String::new(),
                    input: String::new(),
                }));
            }
        }
    }

    pub fn permission_answered(&mut self, id: &ToolCallId, allowed: bool) {
        if let Some(index) = self.tool_index(id)
            && let Entry::Tool(card) = &mut self.entries[index]
            && card.status == ToolStatus::WaitingForPermission
        {
            card.status = if allowed {
                ToolStatus::InProgress
            } else {
                ToolStatus::Rejected
            };
        }
    }

    pub fn end_turn(&mut self, end: TurnEnd, worked_seconds: u64) {
        let unfinished = match end {
            TurnEnd::Cancelled => Some(ToolStatus::Canceled),
            TurnEnd::Failed(_) => Some(ToolStatus::Failed),
            _ => None,
        };
        if let Some(status) = unfinished {
            for entry in &mut self.entries {
                if let Entry::Tool(card) = entry
                    && !card.status.is_finished()
                {
                    card.status = status;
                }
            }
        }
        if worked_seconds >= 1 {
            self.insert_worked_divider(worked_seconds);
        }
        match end {
            TurnEnd::Finished => {}
            TurnEnd::TokenLimit => self.notice(NoticeKind::Info, t!("chat.turn.length_limit")),
            TurnEnd::Refused => self.notice(NoticeKind::Info, t!("chat.turn.refused")),
            TurnEnd::Cancelled => self.notice(NoticeKind::Info, t!("chat.turn.cancelled")),
            TurnEnd::Failed(reason) => self.notice(NoticeKind::Error, &reason),
        }
        self.open_message = None;
        self.turn = TurnState::Idle;
    }

    // As in the design: the divider sits above the agent's closing text for the turn.
    fn insert_worked_divider(&mut self, seconds: u64) {
        let turn_start = self
            .entries
            .iter()
            .rposition(|entry| matches!(entry, Entry::User(_)))
            .map_or(0, |index| index + 1);
        let closing = self.entries[turn_start..]
            .iter()
            .rposition(|entry| matches!(entry, Entry::Assistant { .. }))
            .map(|offset| turn_start + offset);
        let divider = Entry::Worked { seconds };
        match closing {
            Some(index) if index + 1 == self.entries.len() => {
                self.entries.insert(index, divider);
            }
            _ => self.push_entry(divider),
        }
    }

    pub fn notice(&mut self, kind: NoticeKind, text: &str) {
        self.push_entry(Entry::Notice {
            kind,
            text: text.to_string(),
        });
    }

    // Text the agent streams after the card starts a new message below it.
    pub fn file_created(&mut self, card: FileCard) {
        self.open_message = None;
        self.push_entry(Entry::File(card));
    }

    pub fn clear(&mut self) {
        *self = Thread::default();
    }
}

impl fmt::Display for ToolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ToolKind::Read => t!("chat.kind.read"),
            ToolKind::Edit => t!("chat.kind.edit"),
            ToolKind::Delete => t!("chat.kind.delete"),
            ToolKind::Move => t!("chat.kind.move"),
            ToolKind::Search => t!("chat.kind.search"),
            ToolKind::Execute => t!("chat.kind.execute"),
            ToolKind::Think => t!("chat.kind.think"),
            ToolKind::Fetch => t!("chat.kind.fetch"),
            ToolKind::Other => t!("chat.kind.tool"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, status: ToolStatus) -> ToolCard {
        ToolCard {
            id: ToolCallId::new(id),
            title: format!("call {id}"),
            kind: ToolKind::Other,
            status,
            detail: String::new(),
            input: String::new(),
        }
    }

    fn running() -> Thread {
        let mut thread = Thread::default();
        assert!(thread.submit("hello".to_string()));
        thread
    }

    #[test]
    fn a_message_is_refused_while_the_agent_works_or_when_blank() {
        let mut thread = Thread::default();
        assert!(!thread.submit("   ".to_string()));
        assert!(thread.submit("one".to_string()));
        assert!(!thread.submit("two".to_string()));
        thread.end_turn(TurnEnd::Finished, 0);
        assert!(thread.submit("two".to_string()));
    }

    #[test]
    fn chunks_grow_one_message_and_report_where_the_new_text_starts() {
        let mut thread = running();
        assert_eq!(
            thread.apply(AgentUpdate::Message("Hel".to_string())),
            Effect::Appended {
                message: MessageId(0),
                added: "Hel".to_string()
            }
        );
        assert_eq!(
            thread.apply(AgentUpdate::Message("lo".to_string())),
            Effect::Appended {
                message: MessageId(0),
                added: "lo".to_string()
            }
        );
        assert_eq!(thread.message_text(MessageId(0)), Some("Hello"));
        assert_eq!(
            thread.apply(AgentUpdate::Message(String::new())),
            Effect::Nothing
        );
    }

    #[test]
    fn text_after_a_tool_call_starts_a_new_message() {
        let mut thread = running();
        thread.apply(AgentUpdate::Message("Reading.".to_string()));
        thread.apply(AgentUpdate::ToolStarted(card("t1", ToolStatus::InProgress)));
        let effect = thread.apply(AgentUpdate::Message("Done.".to_string()));
        assert_eq!(
            effect,
            Effect::Appended {
                message: MessageId(1),
                added: "Done.".to_string()
            }
        );
        assert_eq!(thread.entries().len(), 4);
    }

    #[test]
    fn a_tool_call_moves_through_its_statuses() {
        let mut thread = running();
        thread.apply(AgentUpdate::ToolStarted(card("t1", ToolStatus::Pending)));
        let change = |status, detail: Option<&str>| AgentUpdate::ToolChanged {
            id: ToolCallId::new("t1"),
            title: None,
            status: Some(status),
            detail: detail.map(str::to_string),
            input: None,
        };
        thread.apply(change(ToolStatus::InProgress, None));
        thread.apply(change(ToolStatus::Completed, Some("12 cells")));
        let Entry::Tool(done) = &thread.entries()[1] else {
            panic!("expected a tool call");
        };
        assert_eq!(done.status, ToolStatus::Completed);
        assert_eq!(done.detail, "12 cells");
        assert_eq!(done.title, "call t1");
    }

    #[test]
    fn an_update_for_an_unknown_call_changes_nothing() {
        let mut thread = running();
        let effect = thread.apply(AgentUpdate::ToolChanged {
            id: ToolCallId::new("ghost"),
            title: Some("x".to_string()),
            status: Some(ToolStatus::Completed),
            detail: None,
            input: None,
        });
        assert_eq!(effect, Effect::Nothing);
        assert_eq!(thread.entries().len(), 1);
    }

    #[test]
    fn a_repeated_start_replaces_the_call_instead_of_duplicating_it() {
        let mut thread = running();
        thread.apply(AgentUpdate::ToolStarted(card("t1", ToolStatus::Pending)));
        thread.apply(AgentUpdate::ToolStarted(card("t1", ToolStatus::InProgress)));
        assert_eq!(thread.entries().len(), 2);
    }

    #[test]
    fn a_call_waiting_for_the_user_ignores_agent_status_updates_until_answered() {
        let mut thread = running();
        thread.apply(AgentUpdate::ToolStarted(card("t1", ToolStatus::Pending)));
        thread.waiting_for_permission(&ToolCallId::new("t1"), "ignored");
        thread.apply(AgentUpdate::ToolChanged {
            id: ToolCallId::new("t1"),
            title: None,
            status: Some(ToolStatus::InProgress),
            detail: None,
            input: None,
        });
        let Entry::Tool(waiting) = &thread.entries()[1] else {
            panic!("expected a tool call");
        };
        assert_eq!(waiting.status, ToolStatus::WaitingForPermission);
        thread.permission_answered(&ToolCallId::new("t1"), false);
        let Entry::Tool(denied) = &thread.entries()[1] else {
            panic!("expected a tool call");
        };
        assert_eq!(denied.status, ToolStatus::Rejected);
    }

    #[test]
    fn a_permission_for_a_call_not_seen_yet_creates_the_card() {
        let mut thread = running();
        thread.waiting_for_permission(&ToolCallId::new("new"), "Write cells");
        let Entry::Tool(created) = &thread.entries()[1] else {
            panic!("expected a tool call");
        };
        assert_eq!(created.title, "Write cells");
        assert_eq!(created.status, ToolStatus::WaitingForPermission);
        thread.permission_answered(&ToolCallId::new("new"), true);
        let Entry::Tool(allowed) = &thread.entries()[1] else {
            panic!("expected a tool call");
        };
        assert_eq!(allowed.status, ToolStatus::InProgress);
    }

    #[test]
    fn stopping_marks_open_calls_stopped_and_keeps_finished_ones() {
        let mut thread = running();
        thread.apply(AgentUpdate::ToolStarted(card(
            "done",
            ToolStatus::Completed,
        )));
        thread.apply(AgentUpdate::ToolStarted(card(
            "open",
            ToolStatus::InProgress,
        )));
        thread.waiting_for_permission(&ToolCallId::new("open"), "");
        thread.request_cancel();
        assert_eq!(thread.turn(), TurnState::Cancelling);
        thread.end_turn(TurnEnd::Cancelled, 0);
        let statuses: Vec<ToolStatus> = thread
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                Entry::Tool(card) => Some(card.status),
                _ => None,
            })
            .collect();
        assert_eq!(statuses, [ToolStatus::Completed, ToolStatus::Canceled]);
        assert_eq!(thread.turn(), TurnState::Idle);
        assert!(matches!(
            thread.entries().last(),
            Some(Entry::Notice {
                kind: NoticeKind::Info,
                ..
            })
        ));
    }

    #[test]
    fn a_failed_turn_shows_the_reason_and_fails_open_calls() {
        let mut thread = running();
        thread.apply(AgentUpdate::ToolStarted(card(
            "open",
            ToolStatus::InProgress,
        )));
        thread.end_turn(TurnEnd::Failed("the agent exited".to_string()), 0);
        let Entry::Tool(failed) = &thread.entries()[1] else {
            panic!("expected a tool call");
        };
        assert_eq!(failed.status, ToolStatus::Failed);
        assert_eq!(
            thread.entries().last(),
            Some(&Entry::Notice {
                kind: NoticeKind::Error,
                text: "the agent exited".to_string()
            })
        );
    }

    #[test]
    fn the_worked_divider_sits_above_the_closing_text_of_the_turn() {
        let mut thread = running();
        thread.apply(AgentUpdate::Message("Reading.".to_string()));
        thread.apply(AgentUpdate::ToolStarted(card("t1", ToolStatus::Completed)));
        thread.apply(AgentUpdate::Message("Result.".to_string()));
        thread.end_turn(TurnEnd::Finished, 6);
        assert_eq!(thread.entries()[3], Entry::Worked { seconds: 6 });
        assert_eq!(thread.message_text(MessageId(1)), Some("Result."));
        assert!(matches!(thread.entries()[4], Entry::Assistant { .. }));
    }

    #[test]
    fn a_turn_under_one_second_has_no_divider_and_an_earlier_turn_is_untouched() {
        let mut thread = running();
        thread.apply(AgentUpdate::Message("First.".to_string()));
        thread.end_turn(TurnEnd::Finished, 2);
        assert!(thread.submit("again".to_string()));
        thread.apply(AgentUpdate::Message("Second.".to_string()));
        thread.end_turn(TurnEnd::Finished, 0);
        let dividers = thread
            .entries()
            .iter()
            .filter(|entry| matches!(entry, Entry::Worked { .. }))
            .count();
        assert_eq!(dividers, 1);
        assert_eq!(thread.entries()[1], Entry::Worked { seconds: 2 });
    }

    #[test]
    fn durations_read_naturally_and_never_say_1_seconds() {
        assert_eq!(worked_label(1), "Worked 1 second");
        assert_eq!(worked_label(6), "Worked 6 seconds");
        assert_eq!(worked_label(60), "Worked 1 minute");
        assert_eq!(worked_label(125), "Worked 2 minutes 5 seconds");
        assert_eq!(worked_label(61), "Worked 1 minute 1 second");
    }

    #[test]
    fn long_tool_details_are_cut() {
        let mut thread = running();
        thread.apply(AgentUpdate::ToolStarted(card("t1", ToolStatus::Pending)));
        thread.apply(AgentUpdate::ToolChanged {
            id: ToolCallId::new("t1"),
            title: None,
            status: None,
            detail: Some("x".repeat(5_000)),
            input: None,
        });
        let Entry::Tool(shown) = &thread.entries()[1] else {
            panic!("expected a tool call");
        };
        assert_eq!(shown.detail.chars().count(), MAX_DETAIL_CHARS + 1);
    }

    #[test]
    fn replayed_history_rebuilds_both_sides_without_starting_a_turn() {
        let mut thread = Thread::default();
        thread.apply(AgentUpdate::UserText("Hel".to_string()));
        thread.apply(AgentUpdate::UserText("lo".to_string()));
        thread.apply(AgentUpdate::Message("Hi.".to_string()));
        thread.apply(AgentUpdate::UserText("Again".to_string()));
        assert_eq!(thread.turn(), TurnState::Idle);
        assert_eq!(thread.entries().len(), 3);
        assert_eq!(thread.entries()[0], Entry::User("Hello".to_string()));
        assert_eq!(thread.entries()[2], Entry::User("Again".to_string()));
    }

    #[test]
    fn titles_and_inputs_are_capped_whether_a_call_starts_or_changes() {
        let mut thread = running();
        let mut long = card("t1", ToolStatus::Pending);
        long.title = "t".repeat(1_000);
        long.input = "i".repeat(5_000);
        thread.apply(AgentUpdate::ToolStarted(long));
        thread.apply(AgentUpdate::ToolChanged {
            id: ToolCallId::new("t1"),
            title: Some("u".repeat(1_000)),
            status: None,
            detail: None,
            input: Some("j".repeat(5_000)),
        });
        let Entry::Tool(shown) = &thread.entries()[1] else {
            panic!("expected a tool call");
        };
        assert_eq!(shown.title.chars().count(), MAX_TITLE_CHARS + 1);
        assert_eq!(shown.input.chars().count(), MAX_DETAIL_CHARS + 1);
    }

    #[test]
    fn one_message_stops_growing_at_its_cap() {
        let mut thread = running();
        thread.apply(AgentUpdate::Message("a".repeat(MAX_MESSAGE_CHARS - 1)));
        let effect = thread.apply(AgentUpdate::Message("bbbb".to_string()));
        assert_eq!(effect, Effect::Nothing);
    }

    #[test]
    fn the_transcript_drops_its_oldest_entries_behind_one_notice() {
        let mut thread = running();
        for number in 0..(MAX_ENTRIES + 50) {
            thread.apply(AgentUpdate::ToolStarted(card(
                &format!("t{number}"),
                ToolStatus::Completed,
            )));
        }
        assert_eq!(thread.entries().len(), MAX_ENTRIES);
        assert!(matches!(
            &thread.entries()[0],
            Entry::Notice { text, .. } if text == trimmed_notice()
        ));
        let notices = thread
            .entries()
            .iter()
            .filter(|entry| matches!(entry, Entry::Notice { .. }))
            .count();
        assert_eq!(notices, 1);
        let effect = thread.apply(AgentUpdate::Message("still works".to_string()));
        assert!(matches!(effect, Effect::Appended { .. }));
    }

    #[test]
    fn a_new_conversation_forgets_everything() {
        let mut thread = running();
        thread.apply(AgentUpdate::Message("x".to_string()));
        thread.clear();
        assert!(thread.is_empty());
        assert_eq!(thread.turn(), TurnState::Idle);
        assert!(thread.submit("fresh".to_string()));
    }
}
