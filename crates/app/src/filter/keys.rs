use super::list_state::{ListState, Phase};

// The five Tab stops of the list; Retry takes the list's place when the read failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Menu,
    Search,
    List,
    Retry,
    Ok,
    Cancel,
}

pub const MENU_ITEMS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    SortAscending,
    SortDescending,
    ClearFilter,
}

impl MenuItem {
    pub fn at(index: usize) -> Option<MenuItem> {
        [
            MenuItem::SortAscending,
            MenuItem::SortDescending,
            MenuItem::ClearFilter,
        ]
        .get(index)
        .copied()
    }
}

// Which controls take focus: the others are drawn disabled and skipped by Tab and arrows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    pub menu: [bool; MENU_ITEMS],
    pub search: bool,
    pub list: bool,
    pub retry: bool,
    pub ok: bool,
}

impl Controls {
    pub fn of(state: &ListState, column_filtered: bool) -> Controls {
        let phase = state.phase();
        Controls {
            menu: [true, true, column_filtered],
            search: matches!(phase, Phase::Loading | Phase::Ready),
            list: phase == Phase::Ready && !state.rows().is_empty(),
            retry: phase == Phase::Error,
            ok: state.ok_enabled(),
        }
    }

    pub fn stops(&self) -> Vec<Stop> {
        [
            (Stop::Menu, self.menu.iter().any(|enabled| *enabled)),
            (Stop::Search, self.search),
            (Stop::List, self.list),
            (Stop::Retry, self.retry),
            (Stop::Ok, self.ok),
            (Stop::Cancel, true),
        ]
        .into_iter()
        .filter_map(|(stop, enabled)| enabled.then_some(stop))
        .collect()
    }

    // Search when it works, otherwise the first menu item.
    pub fn first_stop(&self) -> Stop {
        if self.search {
            Stop::Search
        } else {
            Stop::Menu
        }
    }

    pub fn first_menu_item(&self) -> Option<usize> {
        self.menu.iter().position(|enabled| *enabled)
    }

    pub fn last_menu_item(&self) -> Option<usize> {
        self.menu.iter().rposition(|enabled| *enabled)
    }

    // Up and Down stay at the ends; a current item that is no longer enabled moves to the
    // nearest one that is.
    pub fn step_menu(&self, current: usize, down: bool) -> usize {
        let candidates: Vec<usize> = if down {
            (current + 1..MENU_ITEMS).collect()
        } else {
            (0..current).rev().collect()
        };
        candidates
            .into_iter()
            .find(|index| self.menu[*index])
            .or_else(|| {
                self.menu
                    .get(current)
                    .copied()
                    .and_then(|on| on.then_some(current))
            })
            .or_else(|| self.first_menu_item())
            .unwrap_or(current)
    }
}

// Tab and Shift+Tab wrap around the enabled stops.
pub fn next_stop(stops: &[Stop], from: Stop, backwards: bool) -> Stop {
    let Some(at) = stops.iter().position(|stop| *stop == from) else {
        return stops.first().copied().unwrap_or(Stop::Cancel);
    };
    let count = stops.len();
    let next = if backwards {
        (at + count - 1) % count
    } else {
        (at + 1) % count
    };
    stops[next]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMove {
    To(usize),
    LeaveUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKey {
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

// Positions are 0 for "(Select All)" and then the values; `len` counts all of them. Down
// from the last stays, Up from the first leaves for Search.
pub fn list_move(at: usize, len: usize, key: ListKey, page: usize) -> ListMove {
    let last = len.saturating_sub(1);
    match key {
        ListKey::Up if at == 0 => ListMove::LeaveUp,
        ListKey::Up => ListMove::To(at - 1),
        ListKey::Down => ListMove::To((at + 1).min(last)),
        ListKey::Home => ListMove::To(0),
        ListKey::End => ListMove::To(last),
        ListKey::PageUp => ListMove::To(at.saturating_sub(page)),
        ListKey::PageDown => ListMove::To((at + page).min(last)),
    }
}

// What Enter does at each stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enter {
    RunItem(MenuItem),
    Apply,
    Retry,
    Close,
    Nothing,
}

pub fn enter(stop: Stop, menu_item: usize, controls: &Controls) -> Enter {
    match stop {
        Stop::Menu => MenuItem::at(menu_item)
            .filter(|_| controls.menu.get(menu_item).copied().unwrap_or(false))
            .map_or(Enter::Nothing, Enter::RunItem),
        Stop::Search | Stop::List | Stop::Ok if controls.ok => Enter::Apply,
        Stop::Search | Stop::List | Stop::Ok => Enter::Nothing,
        Stop::Retry => Enter::Retry,
        Stop::Cancel => Enter::Close,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Escape {
    DismissTooltip,
    ClearSearch,
    Close,
}

// A tooltip goes first, then the text in Search, then the list itself.
pub fn escape(tooltip_visible: bool, stop: Stop, search_empty: bool) -> Escape {
    if tooltip_visible {
        Escape::DismissTooltip
    } else if stop == Stop::Search && !search_empty {
        Escape::ClearSearch
    } else {
        Escape::Close
    }
}

// Search and the list own typing: a printable key on the list moves focus to Search.
pub fn types_into_search(stop: Stop) -> bool {
    stop == Stop::List
}

#[cfg(test)]
mod tests {
    use zenkai_types::{FilterItem, FilterList};

    use super::{
        Controls, Enter, Escape, ListKey, ListMove, MenuItem, Stop, enter, escape, list_move,
        next_stop, types_into_search,
    };
    use crate::filter::list_state::ListState;

    fn ready(values: &[&str]) -> ListState {
        let mut state = ListState::loading();
        state.load(FilterList {
            items: values
                .iter()
                .map(|text| FilterItem {
                    text: (*text).into(),
                    count: 1,
                    shown: true,
                })
                .collect(),
            ..FilterList::default()
        });
        state
    }

    #[test]
    fn five_stops_in_order_and_cancel_wraps_to_the_menu() {
        let controls = Controls::of(&ready(&["a", "b"]), true);
        let stops = controls.stops();
        assert_eq!(
            stops,
            [Stop::Menu, Stop::Search, Stop::List, Stop::Ok, Stop::Cancel]
        );
        assert_eq!(next_stop(&stops, Stop::Cancel, false), Stop::Menu);
        assert_eq!(next_stop(&stops, Stop::Menu, true), Stop::Cancel);
        assert_eq!(next_stop(&stops, Stop::Search, true), Stop::Menu);
    }

    #[test]
    fn disabled_controls_are_skipped() {
        let mut state = ready(&["a"]);
        state.toggle(0);
        let controls = Controls::of(&state, false);
        assert!(!controls.ok);
        assert_eq!(controls.menu, [true, true, false]);
        assert_eq!(
            controls.stops(),
            [Stop::Menu, Stop::Search, Stop::List, Stop::Cancel]
        );
    }

    #[test]
    fn a_failed_read_puts_retry_in_the_lists_place_and_disables_search() {
        let mut state = ready(&["a"]);
        state.fail();
        let controls = Controls::of(&state, true);
        assert_eq!(controls.stops(), [Stop::Menu, Stop::Retry, Stop::Cancel]);
        assert_eq!(controls.first_stop(), Stop::Menu);
    }

    #[test]
    fn a_rule_from_excel_keeps_sort_and_clear_only() {
        let mut state = ListState::loading();
        state.load(FilterList {
            custom: true,
            ..FilterList::default()
        });
        let controls = Controls::of(&state, true);
        assert_eq!(controls.stops(), [Stop::Menu, Stop::Cancel]);
        assert_eq!(controls.menu, [true, true, true]);
    }

    #[test]
    fn menu_arrows_stop_at_the_ends_and_skip_a_disabled_item() {
        let unfiltered = Controls::of(&ready(&["a"]), false);
        assert_eq!(unfiltered.step_menu(0, true), 1);
        assert_eq!(unfiltered.step_menu(1, true), 1);
        assert_eq!(unfiltered.step_menu(0, false), 0);
        let filtered = Controls::of(&ready(&["a"]), true);
        assert_eq!(filtered.step_menu(1, true), 2);
        assert_eq!(filtered.step_menu(2, true), 2);
        assert_eq!(filtered.last_menu_item(), Some(2));
        assert_eq!(unfiltered.last_menu_item(), Some(1));
    }

    #[test]
    fn the_list_moves_by_row_and_page_and_leaves_upwards_from_the_first() {
        assert_eq!(list_move(0, 20, ListKey::Up, 9), ListMove::LeaveUp);
        assert_eq!(list_move(3, 20, ListKey::Up, 9), ListMove::To(2));
        assert_eq!(list_move(19, 20, ListKey::Down, 9), ListMove::To(19));
        assert_eq!(list_move(5, 20, ListKey::Home, 9), ListMove::To(0));
        assert_eq!(list_move(5, 20, ListKey::End, 9), ListMove::To(19));
        assert_eq!(list_move(5, 20, ListKey::PageDown, 9), ListMove::To(14));
        assert_eq!(list_move(15, 20, ListKey::PageDown, 9), ListMove::To(19));
        assert_eq!(list_move(5, 20, ListKey::PageUp, 9), ListMove::To(0));
    }

    #[test]
    fn enter_runs_the_menu_item_and_applies_from_search_list_and_ok() {
        let controls = Controls::of(&ready(&["a"]), true);
        assert_eq!(
            enter(Stop::Menu, 1, &controls),
            Enter::RunItem(MenuItem::SortDescending)
        );
        for stop in [Stop::Search, Stop::List, Stop::Ok] {
            assert_eq!(enter(stop, 0, &controls), Enter::Apply);
        }
        assert_eq!(enter(Stop::Cancel, 0, &controls), Enter::Close);
        assert_eq!(enter(Stop::Retry, 0, &controls), Enter::Retry);
    }

    #[test]
    fn enter_in_search_with_nothing_checked_does_nothing() {
        let mut state = ready(&["a"]);
        state.toggle(0);
        let controls = Controls::of(&state, false);
        assert_eq!(enter(Stop::Search, 0, &controls), Enter::Nothing);
        assert_eq!(enter(Stop::Menu, 2, &controls), Enter::Nothing);
    }

    #[test]
    fn escape_dismisses_the_tooltip_then_clears_search_then_closes() {
        assert_eq!(escape(true, Stop::Search, false), Escape::DismissTooltip);
        assert_eq!(escape(false, Stop::Search, false), Escape::ClearSearch);
        assert_eq!(escape(false, Stop::Search, true), Escape::Close);
        assert_eq!(escape(false, Stop::List, false), Escape::Close);
        assert_eq!(escape(false, Stop::Menu, true), Escape::Close);
    }

    #[test]
    fn only_the_list_hands_typing_to_search() {
        assert!(types_into_search(Stop::List));
        assert!(!types_into_search(Stop::Ok));
    }
}
