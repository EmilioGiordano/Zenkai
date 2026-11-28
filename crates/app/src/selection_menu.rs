use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;
use zenkai_grid::{DeleteForward, MenuTarget};
use zenkai_i18n::t;

use crate::actions;

pub enum Entry {
    Item(&'static str, Box<dyn Action>),
    Separator,
}

fn item(label: &'static str, action: impl Action) -> Entry {
    Entry::Item(label, Box::new(action))
}

// Excel's context menu, trimmed to what Zenkai supports. A right click on a row or column
// header shows only the insert and delete of its own line and the size and visibility
// commands of the lines.
pub fn entries(target: MenuTarget) -> Vec<Entry> {
    let rows = target != MenuTarget::ColumnHeader;
    let columns = target != MenuTarget::RowHeader;
    let mut entries = vec![
        item(t!("menu.cut"), actions::Cut),
        item(t!("menu.copy"), actions::Copy),
        item(t!("menu.paste"), actions::Paste),
        item(t!("menu.paste_values"), actions::PasteValues),
        Entry::Separator,
    ];
    if rows {
        entries.push(item(t!("menu.insert_rows"), actions::InsertRows));
    }
    if columns {
        entries.push(item(t!("menu.insert_columns"), actions::InsertColumns));
    }
    if rows {
        entries.push(item(t!("menu.delete_rows"), actions::DeleteRows));
    }
    if columns {
        entries.push(item(t!("menu.delete_columns"), actions::DeleteColumns));
    }
    entries.extend([
        Entry::Separator,
        item(t!("menu.clear_contents"), DeleteForward),
        item(t!("menu.clear_formats"), actions::ClearFormats),
        item(t!("menu.clear_all"), actions::ClearAll),
        Entry::Separator,
    ]);
    match target {
        MenuTarget::Cells => {}
        MenuTarget::RowHeader => entries.extend([
            item(t!("menu.row_height"), actions::RowHeight),
            item(t!("menu.autofit_row_height"), actions::AutoFitRowHeight),
            item(t!("menu.hide_rows"), actions::HideRows),
            item(t!("menu.unhide_rows"), actions::UnhideRows),
            Entry::Separator,
        ]),
        MenuTarget::ColumnHeader => entries.extend([
            item(t!("menu.column_width"), actions::ColumnWidth),
            item(t!("menu.autofit_column_width"), actions::AutoFitColumnWidth),
            item(t!("menu.hide_columns"), actions::HideColumns),
            item(t!("menu.unhide_columns"), actions::UnhideColumns),
            Entry::Separator,
        ]),
    }
    entries.extend([
        item(t!("menu.sort_ascending"), actions::SortAscending),
        item(t!("menu.sort_descending"), actions::SortDescending),
        Entry::Separator,
        item(t!("menu.generate_data"), actions::GenerateData),
        Entry::Separator,
        item(t!("menu.insert_chart"), actions::InsertChart),
    ]);
    entries
}

// Actions go to the grid so they act on the selection the right click just set.
pub fn build(menu: PopupMenu, grid_focus: FocusHandle, target: MenuTarget) -> PopupMenu {
    entries(target)
        .into_iter()
        .fold(menu.action_context(grid_focus), |menu, entry| match entry {
            Entry::Item(label, action) => menu.menu(label, action),
            Entry::Separator => menu.separator(),
        })
}

#[cfg(test)]
mod tests {
    use zenkai_grid::MenuTarget;

    use super::{Entry, entries};

    fn labels(target: MenuTarget) -> Vec<&'static str> {
        entries(target)
            .iter()
            .map(|entry| match entry {
                Entry::Item(label, _) => *label,
                Entry::Separator => "-",
            })
            .collect()
    }

    fn action_of(target: MenuTarget, label: &str) -> String {
        entries(target)
            .into_iter()
            .find_map(|entry| match entry {
                Entry::Item(found, action) if found == label => Some(action.name().to_string()),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn the_cell_menu_is_the_one_it_was() {
        assert_eq!(
            labels(MenuTarget::Cells),
            [
                "Cut",
                "Copy",
                "Paste",
                "Paste values",
                "-",
                "Insert rows above",
                "Insert columns to the left",
                "Delete rows",
                "Delete columns",
                "-",
                "Clear contents",
                "Clear formats",
                "Clear all",
                "-",
                "Sort A to Z",
                "Sort Z to A",
                "-",
                "Generate data…",
                "-",
                "Insert chart",
            ]
        );
    }

    #[test]
    fn the_column_header_menu_adds_width_and_visibility_between_clear_and_sort() {
        let labels = labels(MenuTarget::ColumnHeader);
        assert_eq!(
            labels[8..19],
            [
                "Clear contents",
                "Clear formats",
                "Clear all",
                "-",
                "Column width…",
                "AutoFit column width",
                "Hide columns",
                "Unhide columns",
                "-",
                "Sort A to Z",
                "Sort Z to A",
            ]
        );
        assert!(labels.contains(&"Insert columns to the left"));
        assert!(labels.contains(&"Delete columns"));
        assert!(!labels.contains(&"Insert rows above"));
        assert!(!labels.contains(&"Delete rows"));
        assert!(!labels.contains(&"Row height…"));
    }

    #[test]
    fn the_row_header_menu_adds_height_and_visibility_between_clear_and_sort() {
        let labels = labels(MenuTarget::RowHeader);
        assert_eq!(
            labels[8..19],
            [
                "Clear contents",
                "Clear formats",
                "Clear all",
                "-",
                "Row height…",
                "AutoFit row height",
                "Hide rows",
                "Unhide rows",
                "-",
                "Sort A to Z",
                "Sort Z to A",
            ]
        );
        assert!(labels.contains(&"Insert rows above"));
        assert!(labels.contains(&"Delete rows"));
        assert!(!labels.contains(&"Insert columns to the left"));
        assert!(!labels.contains(&"Delete columns"));
        assert!(!labels.contains(&"Column width…"));
    }

    #[test]
    fn the_header_entries_run_the_size_and_visibility_actions() {
        assert_eq!(
            action_of(MenuTarget::RowHeader, "Row height…"),
            "zenkai::RowHeight"
        );
        assert_eq!(
            action_of(MenuTarget::RowHeader, "AutoFit row height"),
            "zenkai::AutoFitRowHeight"
        );
        assert_eq!(
            action_of(MenuTarget::RowHeader, "Hide rows"),
            "zenkai::HideRows"
        );
        assert_eq!(
            action_of(MenuTarget::RowHeader, "Unhide rows"),
            "zenkai::UnhideRows"
        );
        assert_eq!(
            action_of(MenuTarget::ColumnHeader, "Column width…"),
            "zenkai::ColumnWidth"
        );
        assert_eq!(
            action_of(MenuTarget::ColumnHeader, "AutoFit column width"),
            "zenkai::AutoFitColumnWidth"
        );
        assert_eq!(
            action_of(MenuTarget::ColumnHeader, "Hide columns"),
            "zenkai::HideColumns"
        );
        assert_eq!(
            action_of(MenuTarget::ColumnHeader, "Unhide columns"),
            "zenkai::UnhideColumns"
        );
    }
}
