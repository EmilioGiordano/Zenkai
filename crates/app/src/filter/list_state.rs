use zenkai_types::{FilterItem, FilterList, ShownValues};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    Checked,
    Unchecked,
    Mixed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub text: String,
    pub blank: bool,
    pub checked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Loading,
    Ready,
    Error,
    // The column holds a rule made in Excel that the list cannot show.
    Foreign,
}

// What OK does to the column's criteria.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Unchanged,
    Clear,
    Set(ShownValues),
}

#[derive(Clone, Debug)]
pub struct ListState {
    phase: Phase,
    rows: Vec<Row>,
    truncated: bool,
    search: String,
    touched: bool,
}

impl ListState {
    pub fn loading() -> ListState {
        ListState {
            phase: Phase::Loading,
            rows: Vec::new(),
            truncated: false,
            search: String::new(),
            touched: false,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn searching(&self) -> bool {
        !self.search.is_empty()
    }

    pub fn search(&self) -> &str {
        &self.search
    }

    pub fn set_search(&mut self, text: &str) {
        self.search = text.to_string();
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn start_loading(&mut self) {
        self.phase = Phase::Loading;
    }

    pub fn fail(&mut self) {
        self.phase = Phase::Error;
        self.rows.clear();
    }

    // A search starts with every result checked, as in Excel; with no search the checks
    // are the column's criteria.
    pub fn load(&mut self, list: FilterList) {
        self.touched = false;
        self.truncated = list.truncated;
        if list.custom {
            self.phase = Phase::Foreign;
            self.rows.clear();
            return;
        }
        let searching = self.searching();
        let row = |item: FilterItem, blank: bool| Row {
            text: item.text,
            blank,
            checked: searching || item.shown,
        };
        self.rows = list
            .items
            .into_iter()
            .map(|item| row(item, false))
            .chain(list.blanks.map(|item| row(item, true)))
            .collect();
        self.phase = Phase::Ready;
    }

    pub fn select_all_state(&self) -> CheckState {
        let checked = self.rows.iter().filter(|row| row.checked).count();
        match checked {
            0 => CheckState::Unchecked,
            all if all == self.rows.len() => CheckState::Checked,
            _ => CheckState::Mixed,
        }
    }

    pub fn toggle_select_all(&mut self) {
        let check = self.select_all_state() != CheckState::Checked;
        for row in &mut self.rows {
            row.checked = check;
        }
        self.touched = true;
    }

    pub fn toggle(&mut self, index: usize) {
        if let Some(row) = self.rows.get_mut(index) {
            row.checked = !row.checked;
            self.touched = true;
        }
    }

    // Space on the list: entry 0 is "(Select All)", the others are the values after it.
    pub fn toggle_entry(&mut self, index: usize) {
        match index.checked_sub(1) {
            None => self.toggle_select_all(),
            Some(row) => self.toggle(row),
        }
    }

    pub fn ok_enabled(&self) -> bool {
        self.phase == Phase::Ready && self.rows.iter().any(|row| row.checked)
    }

    pub fn shows_no_matches(&self) -> bool {
        self.phase == Phase::Ready && self.searching() && self.rows.is_empty()
    }

    // With a search only the checked results stay. A truncated list cannot name the values
    // it does not show, so any change hides them and no change leaves the column alone.
    pub fn outcome(&self) -> Outcome {
        if !self.ok_enabled() {
            return Outcome::Unchanged;
        }
        let everything = self.select_all_state() == CheckState::Checked;
        if !self.searching() {
            if self.truncated && !self.touched {
                return Outcome::Unchanged;
            }
            if everything && !self.truncated {
                return Outcome::Clear;
            }
        }
        let checked: Vec<&Row> = self.rows.iter().filter(|row| row.checked).collect();
        Outcome::Set(ShownValues {
            values: checked
                .iter()
                .filter(|row| !row.blank)
                .map(|row| row.text.clone())
                .collect(),
            blanks: checked.iter().any(|row| row.blank),
        })
    }
}

#[cfg(test)]
mod tests {
    use zenkai_types::{FilterItem, FilterList, ShownValues};

    use super::{CheckState, ListState, Outcome, Phase};

    fn item(text: &str, shown: bool) -> FilterItem {
        FilterItem {
            text: text.into(),
            count: 1,
            shown,
        }
    }

    fn list(shown: &[bool], blanks: Option<bool>) -> FilterList {
        FilterList {
            items: ["East", "North", "West"]
                .iter()
                .zip(shown)
                .map(|(text, shown)| item(text, *shown))
                .collect(),
            blanks: blanks.map(|shown| item("", shown)),
            truncated: false,
            custom: false,
        }
    }

    fn loaded(list: FilterList) -> ListState {
        let mut state = ListState::loading();
        state.load(list);
        state
    }

    #[test]
    fn select_all_is_checked_unchecked_or_mixed_from_the_values() {
        let mut state = loaded(list(&[true, true, true], None));
        assert_eq!(state.select_all_state(), CheckState::Checked);
        state.toggle(1);
        assert_eq!(state.select_all_state(), CheckState::Mixed);
        state.toggle_select_all();
        assert_eq!(state.select_all_state(), CheckState::Checked);
        state.toggle_select_all();
        assert_eq!(state.select_all_state(), CheckState::Unchecked);
    }

    #[test]
    fn space_on_the_first_entry_toggles_all_and_on_the_others_one_value() {
        let mut state = loaded(list(&[true, true, true], None));
        state.toggle_entry(2);
        assert!(!state.rows()[1].checked);
        assert_eq!(state.select_all_state(), CheckState::Mixed);
        state.toggle_entry(0);
        assert_eq!(state.select_all_state(), CheckState::Checked);
        state.toggle_entry(0);
        assert_eq!(state.select_all_state(), CheckState::Unchecked);
        state.toggle_entry(99);
        assert_eq!(state.select_all_state(), CheckState::Unchecked);
    }

    #[test]
    fn ok_is_disabled_until_something_is_checked() {
        let mut state = loaded(list(&[false, false, false], None));
        assert!(!state.ok_enabled());
        assert_eq!(state.outcome(), Outcome::Unchanged);
        state.toggle(2);
        assert!(state.ok_enabled());
    }

    #[test]
    fn everything_checked_without_a_search_removes_the_criteria() {
        let state = loaded(list(&[true, true, true], Some(true)));
        assert_eq!(state.outcome(), Outcome::Clear);
    }

    #[test]
    fn some_values_checked_keep_only_those_and_blanks_when_checked() {
        let mut state = loaded(list(&[true, true, true], Some(true)));
        state.toggle(0);
        state.toggle(3);
        assert_eq!(
            state.outcome(),
            Outcome::Set(ShownValues {
                values: vec!["North".into(), "West".into()],
                blanks: false
            })
        );
    }

    #[test]
    fn a_blank_entry_is_listed_last_and_selects_the_blanks() {
        let mut state = loaded(list(&[false, false, false], Some(false)));
        state.toggle(3);
        assert_eq!(
            state.outcome(),
            Outcome::Set(ShownValues {
                values: Vec::new(),
                blanks: true
            })
        );
        assert!(state.rows()[3].blank);
    }

    #[test]
    fn a_search_starts_with_every_result_checked_and_keeps_only_the_checked_ones() {
        let mut state = ListState::loading();
        state.set_search("o");
        state.load(list(&[false, false, false], None));
        assert!(state.rows().iter().all(|row| row.checked));
        assert_eq!(state.select_all_state(), CheckState::Checked);
        state.toggle(0);
        match state.outcome() {
            Outcome::Set(shown) => assert_eq!(shown.values, ["North", "West"]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_search_with_every_result_checked_still_filters_to_them() {
        let mut state = ListState::loading();
        state.set_search("o");
        state.load(list(&[true, true, true], None));
        assert!(matches!(state.outcome(), Outcome::Set(_)));
    }

    #[test]
    fn a_search_with_no_results_shows_no_matches_and_disables_ok() {
        let mut state = ListState::loading();
        state.set_search("zzz");
        state.load(FilterList::default());
        assert!(state.shows_no_matches());
        assert!(!state.ok_enabled());
    }

    #[test]
    fn a_truncated_list_is_left_alone_until_a_check_changes() {
        let mut truncated = list(&[true, true, true], None);
        truncated.truncated = true;
        let mut state = loaded(truncated);
        assert!(state.truncated());
        assert_eq!(state.outcome(), Outcome::Unchanged);
        state.toggle(1);
        state.toggle(1);
        match state.outcome() {
            Outcome::Set(shown) => assert_eq!(shown.values.len(), 3),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_rule_made_in_excel_shows_no_values_and_nothing_to_apply() {
        let state = loaded(FilterList {
            custom: true,
            ..FilterList::default()
        });
        assert_eq!(state.phase(), Phase::Foreign);
        assert!(state.rows().is_empty());
        assert!(!state.ok_enabled());
    }

    #[test]
    fn a_failed_read_clears_the_rows_and_a_retry_loads_again() {
        let mut state = loaded(list(&[true, true, true], None));
        state.fail();
        assert_eq!(state.phase(), Phase::Error);
        assert!(state.rows().is_empty());
        state.start_loading();
        assert_eq!(state.phase(), Phase::Loading);
        assert!(!state.ok_enabled());
    }
}
