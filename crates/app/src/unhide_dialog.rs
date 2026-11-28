use gpui_kit::{FocusHandle, ScrollHandle};
use zenkai_types::SheetId;

pub(crate) const ROW_HEIGHT: f32 = 28.0;
pub(crate) const VISIBLE_ROWS: usize = 7;
pub(crate) const MIN_VISIBLE_ROWS: usize = 3;

// The rows of the list, which rows are selected and where focus is, the way a Windows list
// box with extended selection keeps them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    selected: Vec<bool>,
    focus: usize,
    anchor: usize,
}

impl Selection {
    // The first row is selected and focused when the list opens.
    pub(crate) fn new(rows: usize) -> Selection {
        let mut selected = vec![false; rows];
        if let Some(first) = selected.first_mut() {
            *first = true;
        }
        Selection {
            selected,
            focus: 0,
            anchor: 0,
        }
    }

    pub(crate) fn focus(&self) -> usize {
        self.focus
    }

    pub(crate) fn is_selected(&self, row: usize) -> bool {
        self.selected.get(row).copied().unwrap_or(false)
    }

    pub(crate) fn count(&self) -> usize {
        self.selected.iter().filter(|selected| **selected).count()
    }

    pub(crate) fn rows(&self) -> usize {
        self.selected.len()
    }

    pub(crate) fn selected_rows(&self) -> Vec<usize> {
        (0..self.selected.len())
            .filter(|row| self.selected[*row])
            .collect()
    }

    fn clamped(&self, row: isize) -> usize {
        usize::try_from(row)
            .unwrap_or(0)
            .min(self.selected.len().saturating_sub(1))
    }

    fn select_only(&mut self, row: usize) {
        self.selected.fill(false);
        if let Some(selected) = self.selected.get_mut(row) {
            *selected = true;
        }
        self.focus = row;
        self.anchor = row;
    }

    fn select_range(&mut self, from: usize, to: usize) {
        let (first, last) = (from.min(to), from.max(to));
        for (row, selected) in self.selected.iter_mut().enumerate() {
            *selected = (first..=last).contains(&row);
        }
        self.focus = to;
    }

    // Up and Down: focus and the one selected row move together.
    pub(crate) fn step(&mut self, delta: isize) {
        let row = self.clamped(self.focus as isize + delta);
        self.select_only(row);
    }

    pub(crate) fn first(&mut self) {
        self.select_only(0);
    }

    pub(crate) fn last(&mut self) {
        self.select_only(self.selected.len().saturating_sub(1));
    }

    // Shift with an arrow: the rows between the anchor and the new focus.
    pub(crate) fn extend(&mut self, delta: isize) {
        let row = self.clamped(self.focus as isize + delta);
        self.select_range(self.anchor, row);
    }

    // Ctrl with an arrow: focus moves and the selection stays.
    pub(crate) fn move_focus(&mut self, delta: isize) {
        self.focus = self.clamped(self.focus as isize + delta);
    }

    pub(crate) fn toggle_focused(&mut self) {
        let row = self.focus;
        self.toggle(row);
    }

    fn toggle(&mut self, row: usize) {
        if let Some(selected) = self.selected.get_mut(row) {
            *selected = !*selected;
            self.focus = row;
            self.anchor = row;
        }
    }

    pub(crate) fn select_all(&mut self) {
        self.selected.fill(true);
    }

    pub(crate) fn click(&mut self, row: usize, ctrl: bool, shift: bool) {
        if row >= self.selected.len() {
            return;
        }
        match (ctrl, shift) {
            (_, true) => self.select_range(self.anchor, row),
            (true, false) => self.toggle(row),
            (false, false) => self.select_only(row),
        }
    }
}

// Seven rows, then the list scrolls; a short window gives it fewer rows, down to three.
pub(crate) fn rows_shown(rows: usize, room: f32) -> usize {
    let fits = (room / ROW_HEIGHT).floor().max(0.0) as usize;
    rows.min(VISIBLE_ROWS)
        .min(fits.max(MIN_VISIBLE_ROWS))
        .max(1)
}

pub(crate) struct UnhideDialog {
    pub(crate) generation: u64,
    pub(crate) sheets: Vec<SheetId>,
    pub(crate) selection: Selection,
    pub(crate) dialog_focus: FocusHandle,
    pub(crate) list_focus: FocusHandle,
    pub(crate) scroll: ScrollHandle,
}

impl UnhideDialog {
    pub(crate) fn chosen(&self) -> Vec<SheetId> {
        self.selection
            .selected_rows()
            .into_iter()
            .filter_map(|row| self.sheets.get(row).copied())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Selection, rows_shown};

    fn rows(selection: &Selection) -> Vec<usize> {
        selection.selected_rows()
    }

    #[test]
    fn the_first_row_is_selected_and_focused_on_open() {
        let selection = Selection::new(4);
        assert_eq!(rows(&selection), [0]);
        assert_eq!(selection.focus(), 0);
        assert_eq!(selection.count(), 1);
    }

    #[test]
    fn up_and_down_move_focus_and_select_that_one_row() {
        let mut selection = Selection::new(4);
        selection.step(1);
        selection.step(1);
        assert_eq!((rows(&selection), selection.focus()), (vec![2], 2));
        selection.step(-1);
        assert_eq!((rows(&selection), selection.focus()), (vec![1], 1));
        selection.step(-5);
        assert_eq!(rows(&selection), [0]);
        selection.step(9);
        assert_eq!(rows(&selection), [3]);
    }

    #[test]
    fn shift_extends_from_the_anchor_and_back() {
        let mut selection = Selection::new(5);
        selection.step(1);
        selection.extend(1);
        selection.extend(1);
        assert_eq!(rows(&selection), [1, 2, 3]);
        selection.extend(-1);
        assert_eq!(rows(&selection), [1, 2]);
        selection.extend(-2);
        assert_eq!(rows(&selection), [0, 1]);
        assert_eq!(selection.focus(), 0);
    }

    #[test]
    fn ctrl_arrows_move_focus_without_selecting() {
        let mut selection = Selection::new(5);
        selection.move_focus(1);
        selection.move_focus(1);
        assert_eq!((rows(&selection), selection.focus()), (vec![0], 2));
        selection.toggle_focused();
        assert_eq!(rows(&selection), [0, 2]);
        selection.toggle_focused();
        assert_eq!(rows(&selection), [0]);
    }

    #[test]
    fn toggling_a_row_makes_it_the_anchor_of_the_next_shift_selection() {
        let mut selection = Selection::new(6);
        selection.move_focus(3);
        selection.toggle_focused();
        selection.extend(1);
        assert_eq!(rows(&selection), [3, 4]);
    }

    #[test]
    fn select_all_and_home_and_end() {
        let mut selection = Selection::new(3);
        selection.select_all();
        assert_eq!(rows(&selection), [0, 1, 2]);
        selection.last();
        assert_eq!((rows(&selection), selection.focus()), (vec![2], 2));
        selection.first();
        assert_eq!((rows(&selection), selection.focus()), (vec![0], 0));
    }

    #[test]
    fn clicks_work_as_in_a_windows_list_box() {
        let mut selection = Selection::new(6);
        selection.click(2, false, false);
        assert_eq!(rows(&selection), [2]);
        selection.click(4, true, false);
        assert_eq!(rows(&selection), [2, 4]);
        selection.click(2, true, false);
        assert_eq!(rows(&selection), [4]);
        selection.click(0, false, true);
        assert_eq!(rows(&selection), [0, 1, 2]);
        selection.click(9, false, false);
        assert_eq!(rows(&selection), [0, 1, 2]);
    }

    #[test]
    fn a_list_without_rows_selects_nothing_and_does_not_panic() {
        let mut selection = Selection::new(0);
        selection.step(1);
        selection.extend(1);
        selection.toggle_focused();
        selection.select_all();
        assert_eq!(selection.count(), 0);
        assert_eq!(selection.rows(), 0);
    }

    #[test]
    fn seven_rows_show_then_scroll_and_a_short_window_gives_fewer_down_to_three() {
        assert_eq!(rows_shown(10, 1000.0), 7);
        assert_eq!(rows_shown(4, 1000.0), 4);
        assert_eq!(rows_shown(1, 1000.0), 1);
        assert_eq!(rows_shown(10, 5.0 * 28.0), 5);
        assert_eq!(rows_shown(10, 20.0), 3);
        assert_eq!(rows_shown(2, 20.0), 2);
    }
}
