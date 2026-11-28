use zenkai_types::{SheetId, SheetInfo, SheetVisibility};

const MAX_ORIGINS: usize = 32;

// What an Unhide did: the sheets it showed and the sheet that was active before it, which
// undoing it goes back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnhideOrigin {
    pub(crate) sheets: Vec<SheetId>,
    pub(crate) active_before: SheetId,
}

pub(crate) fn remember(origins: &mut Vec<UnhideOrigin>, origin: UnhideOrigin) {
    origins.push(origin);
    if origins.len() > MAX_ORIGINS {
        origins.remove(0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cause {
    Command,
    Undo,
}

// The sheets the tab strip lists: the visible ones. A file with none is shown through its
// first hidden sheet, or its first sheet when every one is very hidden; nothing is edited.
pub(crate) fn shown(sheets: &[SheetInfo]) -> Vec<SheetId> {
    let visible: Vec<SheetId> = sheets
        .iter()
        .filter(|sheet| sheet.visibility == SheetVisibility::Visible)
        .map(|sheet| sheet.id)
        .collect();
    if !visible.is_empty() {
        return visible;
    }
    sheets
        .iter()
        .find(|sheet| sheet.visibility == SheetVisibility::Hidden)
        .or(sheets.first())
        .map(|sheet| vec![sheet.id])
        .unwrap_or_default()
}

pub(crate) fn first_shown(sheets: &[SheetInfo]) -> SheetId {
    shown(sheets).first().copied().unwrap_or(SheetId(0))
}

pub(crate) fn nothing_visible(sheets: &[SheetInfo]) -> bool {
    !sheets
        .iter()
        .any(|sheet| sheet.visibility == SheetVisibility::Visible)
}

// A sheet in the strip that is not visible, which the strip marks.
pub(crate) fn is_shown_hidden(sheets: &[SheetInfo], id: SheetId) -> bool {
    sheets
        .iter()
        .any(|sheet| sheet.id == id && sheet.visibility != SheetVisibility::Visible)
        && shown(sheets).contains(&id)
}

pub(crate) fn can_hide(sheets: &[SheetInfo]) -> bool {
    sheets
        .iter()
        .filter(|sheet| sheet.visibility == SheetVisibility::Visible)
        .count()
        > 1
}

// Very hidden sheets never count: Excel only lists them through macros.
pub(crate) fn hidden(sheets: &[SheetInfo]) -> Vec<SheetId> {
    sheets
        .iter()
        .filter(|sheet| sheet.visibility == SheetVisibility::Hidden)
        .map(|sheet| sheet.id)
        .collect()
}

// The next sheet of the strip after `from`, or the one before it.
pub(crate) fn neighbour(shown: &[SheetId], from: SheetId, forward: bool) -> Option<SheetId> {
    if forward {
        shown.iter().copied().find(|sheet| *sheet > from)
    } else {
        shown.iter().copied().rev().find(|sheet| *sheet < from)
    }
}

// `from` itself when the strip lists it, else the sheet to its right, else the nearest on
// its left.
pub(crate) fn nearest(shown: &[SheetId], from: SheetId) -> Option<SheetId> {
    if shown.contains(&from) {
        return Some(from);
    }
    neighbour(shown, from, true).or_else(|| neighbour(shown, from, false))
}

// Where the sheet goes when moved past its nearest neighbour in the strip, stepping over
// the hidden sheets between them.
pub(crate) fn move_target(sheets: &[SheetInfo], from: SheetId, left: bool) -> Option<u32> {
    neighbour(&shown(sheets), from, !left).map(|sheet| sheet.0)
}

fn same_layout(before: &[SheetInfo], after: &[SheetInfo]) -> bool {
    before.len() == after.len()
        && before
            .iter()
            .zip(after)
            .all(|(was, is)| was.name == is.name)
}

fn changed(
    before: &[SheetInfo],
    after: &[SheetInfo],
    wanted: impl Fn(SheetVisibility, SheetVisibility) -> bool,
) -> Vec<SheetId> {
    before
        .iter()
        .zip(after)
        .filter(|(was, is)| wanted(was.visibility, is.visibility))
        .map(|(_, is)| is.id)
        .collect()
}

// The sheet to show once the workbook changed from `before` to `after`. Sheets that came
// back make the leftmost of them active (Unhide, undoing a Hide); an active sheet that went
// away makes the next one to its right active, else the nearest on its left, except that
// undoing an Unhide returns to the sheet that was active before it.
pub(crate) fn settled_active(
    before: &[SheetInfo],
    after: &[SheetInfo],
    active: SheetId,
    cause: Cause,
    origins: &[UnhideOrigin],
) -> SheetId {
    let now = shown(after);
    if same_layout(before, after) {
        let shown_again = changed(before, after, |was, is| {
            was != SheetVisibility::Visible && is == SheetVisibility::Visible
        });
        let gone = changed(before, after, |was, is| {
            was == SheetVisibility::Visible && is != SheetVisibility::Visible
        });
        if gone.is_empty()
            && let Some(first) = shown_again.first()
        {
            return *first;
        }
        if cause == Cause::Undo
            && !gone.is_empty()
            && !now.contains(&active)
            && let Some(origin) = origins.iter().rev().find(|origin| origin.sheets == gone)
            && now.contains(&origin.active_before)
        {
            return origin.active_before;
        }
    }
    nearest(&now, active).unwrap_or(active)
}

#[cfg(test)]
mod tests {
    use zenkai_types::{SheetId, SheetInfo, SheetVisibility};

    use super::{
        Cause, UnhideOrigin, can_hide, first_shown, hidden, is_shown_hidden, move_target,
        neighbour, nothing_visible, remember, settled_active, shown,
    };

    const V: SheetVisibility = SheetVisibility::Visible;
    const H: SheetVisibility = SheetVisibility::Hidden;
    const VH: SheetVisibility = SheetVisibility::VeryHidden;

    fn book(sheets: &[(&str, SheetVisibility)]) -> Vec<SheetInfo> {
        sheets
            .iter()
            .enumerate()
            .map(|(index, (name, visibility))| SheetInfo {
                id: SheetId(u32::try_from(index).unwrap()),
                name: (*name).to_string(),
                visibility: *visibility,
                color: None,
            })
            .collect()
    }

    fn ids(sheets: &[u32]) -> Vec<SheetId> {
        sheets.iter().copied().map(SheetId).collect()
    }

    #[test]
    fn the_strip_lists_the_visible_sheets_in_order() {
        let sheets = book(&[("A", V), ("B", H), ("C", V), ("D", VH)]);
        assert_eq!(shown(&sheets), ids(&[0, 2]));
        assert_eq!(hidden(&sheets), ids(&[1]));
        assert!(can_hide(&sheets));
        assert!(!nothing_visible(&sheets));
    }

    #[test]
    fn a_file_with_nothing_visible_shows_its_first_hidden_sheet() {
        let sheets = book(&[("A", VH), ("B", H), ("C", H)]);
        assert_eq!(shown(&sheets), ids(&[1]));
        assert_eq!(first_shown(&sheets), SheetId(1));
        assert!(nothing_visible(&sheets));
        assert!(!can_hide(&sheets));
        assert!(is_shown_hidden(&sheets, SheetId(1)));
        assert!(!is_shown_hidden(&sheets, SheetId(2)));
        assert_eq!(hidden(&sheets), ids(&[1, 2]));
    }

    #[test]
    fn a_file_with_only_very_hidden_sheets_shows_its_first_sheet() {
        let sheets = book(&[("A", VH), ("B", VH)]);
        assert_eq!(shown(&sheets), ids(&[0]));
        assert!(hidden(&sheets).is_empty());
        assert!(is_shown_hidden(&sheets, SheetId(0)));
    }

    #[test]
    fn one_visible_sheet_cannot_be_hidden() {
        assert!(!can_hide(&book(&[("A", V), ("B", H)])));
        assert!(can_hide(&book(&[("A", V), ("B", V)])));
    }

    #[test]
    fn next_and_previous_skip_hidden_sheets_and_stop_at_the_ends() {
        let sheets = book(&[("A", V), ("B", H), ("C", V), ("D", V)]);
        let strip = shown(&sheets);
        assert_eq!(neighbour(&strip, SheetId(0), true), Some(SheetId(2)));
        assert_eq!(neighbour(&strip, SheetId(2), false), Some(SheetId(0)));
        assert_eq!(neighbour(&strip, SheetId(3), true), None);
        assert_eq!(neighbour(&strip, SheetId(0), false), None);
    }

    #[test]
    fn a_sheet_moves_past_its_nearest_visible_neighbour() {
        let sheets = book(&[("Summary", V), ("Inputs", H), ("Q1", V), ("Q2", V)]);
        assert_eq!(move_target(&sheets, SheetId(2), true), Some(0));
        assert_eq!(move_target(&sheets, SheetId(0), false), Some(2));
        assert_eq!(move_target(&sheets, SheetId(0), true), None);
        assert_eq!(move_target(&sheets, SheetId(3), false), None);
    }

    fn after_hiding(before: &[(&str, SheetVisibility)], hide: &[usize], active: u32) -> SheetId {
        let was = book(before);
        let mut now = before.to_vec();
        for index in hide {
            now[*index].1 = H;
        }
        settled_active(&was, &book(&now), SheetId(active), Cause::Command, &[])
    }

    #[test]
    fn hiding_activates_the_next_visible_sheet_to_the_right() {
        let sheets = [("Summary", V), ("Inputs", V), ("Q1", V), ("Q2", V)];
        assert_eq!(after_hiding(&sheets, &[2], 2), SheetId(3));
    }

    #[test]
    fn hiding_the_last_visible_sheet_activates_the_nearest_on_the_left() {
        let sheets = [("Summary", V), ("Inputs", V), ("Q1", V), ("Q2", V)];
        assert_eq!(after_hiding(&sheets, &[3], 3), SheetId(2));
        let with_hidden = [("A", V), ("B", H), ("C", V)];
        assert_eq!(after_hiding(&with_hidden, &[2], 2), SheetId(0));
    }

    #[test]
    fn hiding_skips_hidden_sheets_on_the_right() {
        let sheets = [("A", V), ("B", V), ("C", H), ("D", V)];
        assert_eq!(after_hiding(&sheets, &[1], 1), SheetId(3));
    }

    #[test]
    fn unhiding_activates_the_leftmost_sheet_that_came_back() {
        let before = book(&[("Summary", V), ("Lookups", H), ("Q1", V), ("Archive", H)]);
        let after = book(&[("Summary", V), ("Lookups", V), ("Q1", V), ("Archive", V)]);
        assert_eq!(
            settled_active(&before, &after, SheetId(0), Cause::Command, &[]),
            SheetId(1)
        );
        let one = book(&[("Summary", V), ("Lookups", H), ("Q1", V), ("Archive", V)]);
        assert_eq!(
            settled_active(&one, &after, SheetId(0), Cause::Command, &[]),
            SheetId(1)
        );
        let last = book(&[("Summary", V), ("Lookups", V), ("Q1", V), ("Archive", H)]);
        assert_eq!(
            settled_active(&before, &last, SheetId(0), Cause::Command, &[]),
            SheetId(1)
        );
        assert_eq!(
            settled_active(&last, &after, SheetId(0), Cause::Command, &[]),
            SheetId(3)
        );
    }

    #[test]
    fn undoing_a_hide_makes_the_restored_sheet_active_again() {
        let hidden_one = book(&[("Summary", V), ("Inputs", V), ("Q1", H), ("Q2", V)]);
        let restored = book(&[("Summary", V), ("Inputs", V), ("Q1", V), ("Q2", V)]);
        assert_eq!(
            settled_active(&hidden_one, &restored, SheetId(3), Cause::Undo, &[]),
            SheetId(2)
        );
    }

    #[test]
    fn undoing_an_unhide_returns_to_the_sheet_that_was_active_before() {
        let shown_all = book(&[("Summary", V), ("Inputs", V), ("Q1", V), ("Archive", V)]);
        let archive_hidden = book(&[("Summary", V), ("Inputs", V), ("Q1", V), ("Archive", H)]);
        let mut origins = Vec::new();
        remember(
            &mut origins,
            UnhideOrigin {
                sheets: ids(&[3]),
                active_before: SheetId(0),
            },
        );
        assert_eq!(
            settled_active(
                &shown_all,
                &archive_hidden,
                SheetId(3),
                Cause::Undo,
                &origins
            ),
            SheetId(0)
        );
    }

    #[test]
    fn hiding_again_a_sheet_that_was_unhidden_still_goes_to_the_right() {
        let shown_all = book(&[("A", V), ("B", V), ("C", V)]);
        let b_hidden = book(&[("A", V), ("B", H), ("C", V)]);
        let origins = [UnhideOrigin {
            sheets: ids(&[1]),
            active_before: SheetId(0),
        }];
        assert_eq!(
            settled_active(&shown_all, &b_hidden, SheetId(1), Cause::Command, &origins),
            SheetId(2)
        );
    }

    #[test]
    fn undoing_an_unhide_of_several_sheets_hides_them_all_and_goes_back() {
        let both = book(&[("A", V), ("B", V), ("C", V), ("D", V)]);
        let again = book(&[("A", V), ("B", H), ("C", H), ("D", V)]);
        let origins = [UnhideOrigin {
            sheets: ids(&[1, 2]),
            active_before: SheetId(3),
        }];
        assert_eq!(
            settled_active(&both, &again, SheetId(1), Cause::Undo, &origins),
            SheetId(3)
        );
    }

    #[test]
    fn a_shown_hidden_sheet_gives_way_to_a_sheet_that_is_unhidden() {
        let before = book(&[("Shown", H), ("Other", H)]);
        let after = book(&[("Shown", H), ("Other", V)]);
        assert_eq!(
            settled_active(&before, &after, SheetId(0), Cause::Command, &[]),
            SheetId(1)
        );
        assert_eq!(shown(&after), ids(&[1]));
    }

    #[test]
    fn the_active_sheet_stays_when_nothing_changes_for_it() {
        let sheets = book(&[("A", V), ("B", H), ("C", V)]);
        assert_eq!(
            settled_active(&sheets, &sheets, SheetId(2), Cause::Command, &[]),
            SheetId(2)
        );
    }

    #[test]
    fn a_deleted_active_sheet_falls_to_its_neighbour_in_the_strip() {
        let before = book(&[("A", V), ("B", V), ("C", H), ("D", V)]);
        let after = book(&[("A", V), ("B", V), ("D", V)]);
        assert_eq!(
            settled_active(&before, &after, SheetId(2), Cause::Command, &[]),
            SheetId(2)
        );
        let tail = book(&[("A", V), ("B", V)]);
        assert_eq!(
            settled_active(&after, &tail, SheetId(2), Cause::Command, &[]),
            SheetId(1)
        );
    }

    #[test]
    fn only_the_latest_origins_are_kept() {
        let mut origins = Vec::new();
        for index in 0..40 {
            remember(
                &mut origins,
                UnhideOrigin {
                    sheets: ids(&[index]),
                    active_before: SheetId(0),
                },
            );
        }
        assert_eq!(origins.len(), 32);
        assert_eq!(origins[0].sheets, ids(&[8]));
    }
}
