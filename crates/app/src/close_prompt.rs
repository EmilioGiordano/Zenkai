use std::collections::VecDeque;
use std::time::Duration;

use zenkai_types::WorkbookId;

use crate::documents::Unsaved;

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Age {
    Now,
    Minutes(u64),
    Hours(u64),
    Days(u64),
}

impl Age {
    pub fn of(elapsed: Duration) -> Age {
        let seconds = elapsed.as_secs();
        match seconds {
            0..MINUTE => Age::Now,
            MINUTE..HOUR => Age::Minutes(seconds / MINUTE),
            HOUR..DAY => Age::Hours(seconds / HOUR),
            _ => Age::Days(seconds / DAY),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CloseGoal {
    #[default]
    Quit,
    // The window goes to the tray and every workbook stays in memory.
    #[cfg(windows)]
    Hide,
}

impl CloseGoal {
    pub fn keeps_running(self) -> bool {
        self != CloseGoal::Quit
    }
}

// What closing the app does with the workbooks the user left unsaved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClosePlan {
    pub discard: Vec<WorkbookId>,
    pub start_empty: bool,
    pub goal: CloseGoal,
}

impl ClosePlan {
    pub fn keeps(&self, id: WorkbookId) -> bool {
        !self.discard.contains(&id)
    }
}

// The rows of the dialog that closes the app, with the choices made so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choices {
    items: Vec<Unsaved>,
    selected: Vec<bool>,
    pub reopen: bool,
    pub reopen_offered: bool,
    pub goal: CloseGoal,
}

impl Choices {
    pub fn new(items: Vec<Unsaved>, reopen_offered: bool) -> Choices {
        Choices {
            selected: vec![true; items.len()],
            items,
            reopen: reopen_offered,
            reopen_offered,
            goal: CloseGoal::Quit,
        }
    }

    pub fn closing_to(self, goal: CloseGoal) -> Choices {
        Choices { goal, ..self }
    }

    pub fn items(&self) -> &[Unsaved] {
        &self.items
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.get(index).copied().unwrap_or(false)
    }

    pub fn toggle(&mut self, index: usize) {
        if let Some(selected) = self.selected.get_mut(index) {
            *selected = !*selected;
        }
    }

    pub fn selected_count(&self) -> usize {
        self.selected.iter().filter(|selected| **selected).count()
    }

    pub fn all_selected(&self) -> bool {
        self.selected.iter().all(|selected| *selected)
    }

    pub fn toggle_all(&mut self) {
        let on = !self.all_selected();
        self.selected.fill(on);
    }

    pub fn ids_to_save(&self) -> Vec<WorkbookId> {
        self.ids_where(true)
    }

    pub fn ids_to_discard(&self) -> Vec<WorkbookId> {
        self.ids_where(false)
    }

    pub fn all_ids(&self) -> Vec<WorkbookId> {
        self.items.iter().map(|item| item.id).collect()
    }

    fn ids_where(&self, selected: bool) -> Vec<WorkbookId> {
        self.items
            .iter()
            .zip(&self.selected)
            .filter(|(_, chosen)| **chosen == selected)
            .map(|(item, _)| item.id)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Tab(WorkbookId),
    App,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    Save(WorkbookId),
    Done,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Settled {
    Ignored,
    Continue,
    Aborted,
}

// Saves the workbooks one at a time and only lets the close go on if every one was saved.
#[derive(Debug)]
pub struct Departure {
    pub purpose: Purpose,
    pub plan: ClosePlan,
    queue: VecDeque<WorkbookId>,
    current: Option<WorkbookId>,
}

impl Departure {
    pub fn closing_tab(id: WorkbookId) -> Departure {
        Departure {
            purpose: Purpose::Tab(id),
            plan: ClosePlan::default(),
            queue: VecDeque::from([id]),
            current: None,
        }
    }

    pub fn closing_app(save: Vec<WorkbookId>, plan: ClosePlan) -> Departure {
        Departure {
            purpose: Purpose::App,
            plan,
            queue: save.into(),
            current: None,
        }
    }

    pub fn next(&mut self) -> Next {
        self.current = self.queue.pop_front();
        match self.current {
            Some(id) => Next::Save(id),
            None => Next::Done,
        }
    }

    pub fn current(&self) -> Option<WorkbookId> {
        self.current
    }

    pub fn purpose_waiting_on(&self, id: WorkbookId) -> Option<Purpose> {
        (self.current == Some(id)).then_some(self.purpose)
    }

    pub fn settled(&mut self, id: WorkbookId, saved: bool) -> Settled {
        if self.current != Some(id) {
            return Settled::Ignored;
        }
        self.current = None;
        if saved {
            Settled::Continue
        } else {
            Settled::Aborted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unsaved(id: u64, name: &str) -> Unsaved {
        Unsaved {
            id: WorkbookId(id),
            name: name.to_string(),
            folder: None,
            since_edit: None,
        }
    }

    fn three() -> Choices {
        Choices::new(
            vec![unsaved(1, "a"), unsaved(2, "b"), unsaved(3, "c")],
            true,
        )
    }

    #[test]
    fn ages_round_down_to_the_largest_unit() {
        let age = |seconds| Age::of(Duration::from_secs(seconds));
        assert_eq!(age(0), Age::Now);
        assert_eq!(age(59), Age::Now);
        assert_eq!(age(60), Age::Minutes(1));
        assert_eq!(age(4 * 60 + 30), Age::Minutes(4));
        assert_eq!(age(3_600), Age::Hours(1));
        assert_eq!(age(23 * 3_600 + 59), Age::Hours(23));
        assert_eq!(age(86_400), Age::Days(1));
        assert_eq!(age(3 * 86_400 + 5), Age::Days(3));
    }

    #[test]
    fn every_workbook_starts_selected_and_the_reopen_choice_follows_the_setting() {
        let choices = three();
        assert_eq!(choices.selected_count(), 3);
        assert!(choices.all_selected());
        assert!(choices.reopen);
        let empty_start = Choices::new(vec![unsaved(1, "a")], false);
        assert!(!empty_start.reopen);
        assert!(!empty_start.reopen_offered);
    }

    #[test]
    fn toggling_a_row_changes_only_that_row_and_the_select_all_state() {
        let mut choices = three();
        choices.toggle(1);
        assert!(choices.is_selected(0) && !choices.is_selected(1) && choices.is_selected(2));
        assert_eq!(choices.selected_count(), 2);
        assert!(!choices.all_selected());
        assert_eq!(choices.ids_to_save(), [WorkbookId(1), WorkbookId(3)]);
        assert_eq!(choices.ids_to_discard(), [WorkbookId(2)]);
        choices.toggle(9);
        assert_eq!(choices.selected_count(), 2);
    }

    #[test]
    fn select_all_selects_everything_unless_everything_is_already_selected() {
        let mut choices = three();
        choices.toggle(0);
        choices.toggle_all();
        assert!(choices.all_selected());
        choices.toggle_all();
        assert_eq!(choices.selected_count(), 0);
        assert!(choices.ids_to_save().is_empty());
        assert_eq!(choices.all_ids().len(), 3);
    }

    #[test]
    fn the_departure_saves_one_workbook_at_a_time_and_then_finishes() {
        let mut departure = Departure::closing_app(
            vec![WorkbookId(1), WorkbookId(3)],
            ClosePlan {
                discard: vec![WorkbookId(2)],
                start_empty: false,
                ..ClosePlan::default()
            },
        );
        assert_eq!(departure.next(), Next::Save(WorkbookId(1)));
        assert_eq!(departure.settled(WorkbookId(1), true), Settled::Continue);
        assert_eq!(departure.next(), Next::Save(WorkbookId(3)));
        assert_eq!(departure.settled(WorkbookId(3), true), Settled::Continue);
        assert_eq!(departure.next(), Next::Done);
        assert!(!departure.plan.keeps(WorkbookId(2)));
        assert!(departure.plan.keeps(WorkbookId(1)));
    }

    #[test]
    fn a_save_that_did_not_happen_aborts_the_close() {
        let mut departure = Departure::closing_tab(WorkbookId(7));
        assert_eq!(departure.next(), Next::Save(WorkbookId(7)));
        assert_eq!(departure.settled(WorkbookId(7), false), Settled::Aborted);
        assert_eq!(departure.current(), None);
    }

    #[test]
    fn an_outcome_for_another_workbook_is_ignored() {
        let mut departure = Departure::closing_tab(WorkbookId(7));
        assert_eq!(departure.settled(WorkbookId(7), true), Settled::Ignored);
        departure.next();
        assert_eq!(departure.settled(WorkbookId(8), false), Settled::Ignored);
        assert_eq!(departure.current(), Some(WorkbookId(7)));
    }

    #[test]
    fn only_the_workbook_being_saved_reports_the_purpose_of_the_close() {
        let mut departure = Departure::closing_app(vec![WorkbookId(1)], ClosePlan::default());
        assert_eq!(departure.purpose_waiting_on(WorkbookId(1)), None);
        departure.next();
        assert_eq!(
            departure.purpose_waiting_on(WorkbookId(1)),
            Some(Purpose::App)
        );
        assert_eq!(departure.purpose_waiting_on(WorkbookId(2)), None);
        departure.settled(WorkbookId(1), false);
        assert_eq!(departure.purpose_waiting_on(WorkbookId(1)), None);

        let mut tab = Departure::closing_tab(WorkbookId(3));
        tab.next();
        assert_eq!(
            tab.purpose_waiting_on(WorkbookId(3)),
            Some(Purpose::Tab(WorkbookId(3)))
        );
    }

    #[test]
    fn the_default_plan_keeps_every_workbook_and_the_session() {
        let plan = ClosePlan::default();
        assert!(plan.keeps(WorkbookId(1)));
        assert!(!plan.start_empty);
    }
}
