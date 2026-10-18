// A patch save loses only what a rewritten worksheet owns: its own elements and what its
// relationships point at. A full save loses everything here.

use std::collections::{BTreeSet, HashMap, HashSet};

use zenkai_xlsx_reader::{PackageLayout, SheetLink, SheetLinks};

use crate::file::Unsupported;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Held {
    // Found in parts other than worksheets. A patch save copies them whole, even those
    // only a rewritten sheet pointed at, so a later open still finds them there.
    book: BTreeSet<Unsupported>,
    // By position, as the file's worksheets.
    sheets: Vec<BTreeSet<Unsupported>>,
}

fn kind_of(link: SheetLink) -> Unsupported {
    match link {
        SheetLink::Drawing | SheetLink::Image => Unsupported::Images,
        SheetLink::Chart => Unsupported::Charts,
        SheetLink::PivotTable => Unsupported::PivotTables,
        SheetLink::Comments => Unsupported::Comments,
        SheetLink::Table => Unsupported::Tables,
        SheetLink::Hyperlink => Unsupported::Hyperlinks,
    }
}

impl Held {
    // `found` holds every kind found in a part, by the part's name in the archive. Names are
    // compared without case, as the package format and the relationships reader do.
    pub(crate) fn of(found: &[(String, Unsupported)], layout: &PackageLayout) -> Held {
        let mut by_part: HashMap<String, BTreeSet<Unsupported>> = HashMap::new();
        for (name, kind) in found {
            by_part
                .entry(name.to_ascii_lowercase())
                .or_default()
                .insert(*kind);
        }
        let worksheets: HashSet<String> = layout
            .worksheets
            .iter()
            .map(|part| part.to_ascii_lowercase())
            .collect();
        let book: BTreeSet<Unsupported> = by_part
            .iter()
            .filter(|(name, _)| !worksheets.contains(*name))
            .flat_map(|(_, kinds)| kinds.iter().copied())
            .collect();
        let sheets = layout
            .worksheets
            .iter()
            .zip(&layout.sheet_links)
            .map(|(part, links)| {
                let part = part.to_ascii_lowercase();
                let mut owned = by_part.get(&part).cloned().unwrap_or_default();
                match links {
                    SheetLinks::Known(links) => owned.extend(links.iter().map(|l| kind_of(*l))),
                    // Whatever the other parts hold may hang off this sheet.
                    SheetLinks::Unknown => owned.extend(book.iter().copied()),
                }
                owned
            })
            .collect();
        Held { book, sheets }
    }

    pub(crate) fn all(&self) -> BTreeSet<Unsupported> {
        self.sheets
            .iter()
            .flatten()
            .chain(&self.book)
            .copied()
            .collect()
    }

    pub(crate) fn owned_by(&self, rewritten: &[usize]) -> BTreeSet<Unsupported> {
        rewritten
            .iter()
            .filter_map(|index| self.sheets.get(*index))
            .flatten()
            .copied()
            .collect()
    }

    // What the file holds once a patch save rewrote these sheets.
    pub(crate) fn without(&self, rewritten: &[usize]) -> Held {
        let sheets = self
            .sheets
            .iter()
            .enumerate()
            .map(|(index, owned)| match rewritten.contains(&index) {
                true => BTreeSet::new(),
                false => owned.clone(),
            })
            .collect();
        Held {
            book: self.book.clone(),
            sheets,
        }
    }
}

#[cfg(test)]
mod tests {
    use zenkai_xlsx_reader::{PartNames, SharedStrings, StyleCounts};

    use super::*;

    fn layout(sheet_links: Vec<SheetLinks>) -> PackageLayout {
        PackageLayout {
            worksheets: (1..=sheet_links.len())
                .map(|n| format!("xl/worksheets/sheet{n}.xml"))
                .collect(),
            shared_strings: SharedStrings::Absent,
            styles: StyleCounts {
                cell_xfs: 1,
                dxfs: 0,
            },
            part_names: PartNames::Unique,
            sheet_links,
        }
    }

    fn found() -> Vec<(String, Unsupported)> {
        [
            ("xl/worksheets/sheet1.xml", Unsupported::AutoFilter),
            ("xl/pivotTables/pivotTable1.xml", Unsupported::PivotTables),
            ("xl/vbaProject.bin", Unsupported::Macros),
        ]
        .map(|(name, kind)| (name.to_string(), kind))
        .to_vec()
    }

    fn set(kinds: &[Unsupported]) -> BTreeSet<Unsupported> {
        kinds.iter().copied().collect()
    }

    #[test]
    fn a_sheet_owns_its_elements_and_what_its_links_point_at() {
        let pivot = SheetLinks::Known(set_of_links(&[SheetLink::PivotTable]));
        let none = SheetLinks::Known(BTreeSet::new());
        let held = Held::of(&found(), &layout(vec![none, pivot]));
        assert_eq!(held.owned_by(&[0]), set(&[Unsupported::AutoFilter]));
        assert_eq!(held.owned_by(&[1]), set(&[Unsupported::PivotTables]));
        let after = held.without(&[0, 1]);
        assert!(after.owned_by(&[0, 1]).is_empty());
        assert_eq!(
            after.all(),
            set(&[Unsupported::PivotTables, Unsupported::Macros])
        );
    }

    #[test]
    fn part_names_match_without_case() {
        let found = [(
            "XL/Worksheets/Sheet1.xml".to_string(),
            Unsupported::AutoFilter,
        )];
        let held = Held::of(&found, &layout(vec![SheetLinks::Known(BTreeSet::new())]));
        assert_eq!(held.owned_by(&[0]), set(&[Unsupported::AutoFilter]));
        assert_eq!(held.without(&[0]).all(), BTreeSet::new());
    }

    #[test]
    fn a_sheet_with_unknown_links_may_own_anything_outside_the_sheets() {
        let held = Held::of(&found(), &layout(vec![SheetLinks::Unknown]));
        assert_eq!(
            held.owned_by(&[0]),
            set(&[
                Unsupported::PivotTables,
                Unsupported::Macros,
                Unsupported::AutoFilter
            ])
        );
    }

    fn set_of_links(links: &[SheetLink]) -> BTreeSet<SheetLink> {
        links.iter().copied().collect()
    }
}
