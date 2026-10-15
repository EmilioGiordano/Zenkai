// A patch save can lose only what a rewritten worksheet owns: its own elements and what
// its relationships point at, when it is written without them. A full save loses
// everything here.

use std::collections::{BTreeSet, HashMap, HashSet};

use zenkai_xlsx_reader::{PackageLayout, SheetLink, SheetLinks};

use crate::file::Unsupported;
use crate::sheet_elements::Relationships;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Held {
    // Found in parts other than worksheets. A patch save copies them whole, even those
    // only a rewritten sheet pointed at, so a later open still finds them there.
    book: BTreeSet<Unsupported>,
    // By position, as the file's worksheets.
    sheets: Vec<Owned>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Owned {
    // Found in the sheet's own part.
    elements: BTreeSet<Unsupported>,
    // Reached through its relationships part.
    linked: BTreeSet<Unsupported>,
}

impl Owned {
    fn all(&self) -> BTreeSet<Unsupported> {
        self.elements.union(&self.linked).copied().collect()
    }
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
    // `found` holds every kind found in a part, by the part's name in the archive.
    pub(crate) fn of(found: &[(String, Unsupported)], layout: &PackageLayout) -> Held {
        let mut by_part: HashMap<&str, BTreeSet<Unsupported>> = HashMap::new();
        for (name, kind) in found {
            by_part.entry(name.as_str()).or_default().insert(*kind);
        }
        let worksheets: HashSet<&str> = layout.worksheets.iter().map(String::as_str).collect();
        let book: BTreeSet<Unsupported> = by_part
            .iter()
            .filter(|(name, _)| !worksheets.contains(*name))
            .flat_map(|(_, kinds)| kinds.iter().copied())
            .collect();
        let sheets = layout
            .worksheets
            .iter()
            .zip(&layout.sheet_links)
            .map(|(part, links)| Owned {
                elements: by_part.get(part.as_str()).cloned().unwrap_or_default(),
                linked: match links {
                    SheetLinks::Known(links) => links.iter().map(|l| kind_of(*l)).collect(),
                    // Whatever the other parts hold may hang off this sheet.
                    SheetLinks::Unknown => book.clone(),
                },
            })
            .collect();
        Held { book, sheets }
    }

    pub(crate) fn all(&self) -> BTreeSet<Unsupported> {
        self.sheets
            .iter()
            .flat_map(Owned::all)
            .chain(self.book.iter().copied())
            .collect()
    }

    // Sheet `index` is rewritten with elements that hold `written`, keeping its
    // relationships part or not. Returns what it held and no longer does.
    pub(crate) fn rewrite(
        &mut self,
        index: usize,
        written: BTreeSet<Unsupported>,
        relationships: Relationships,
    ) -> BTreeSet<Unsupported> {
        let Some(owned) = self.sheets.get_mut(index) else {
            return BTreeSet::new();
        };
        let after = Owned {
            elements: written,
            linked: match relationships {
                Relationships::Kept => owned.linked.clone(),
                Relationships::Dropped => BTreeSet::new(),
            },
        };
        let lost = owned.all().difference(&after.all()).copied().collect();
        *owned = after;
        lost
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
    fn a_rewritten_sheet_loses_only_what_it_is_written_without() {
        let pivot = SheetLinks::Known(set_of_links(&[SheetLink::PivotTable]));
        let none = SheetLinks::Known(BTreeSet::new());
        let mut held = Held::of(&found(), &layout(vec![none, pivot]));
        assert!(
            held.rewrite(1, BTreeSet::new(), Relationships::Kept)
                .is_empty()
        );
        let filter = set(&[Unsupported::AutoFilter]);
        assert!(held.rewrite(0, filter, Relationships::Kept).is_empty());
        assert_eq!(
            held.rewrite(0, BTreeSet::new(), Relationships::Kept),
            set(&[Unsupported::AutoFilter])
        );
        assert_eq!(
            held.rewrite(1, BTreeSet::new(), Relationships::Dropped),
            set(&[Unsupported::PivotTables])
        );
        assert_eq!(
            held.all(),
            set(&[Unsupported::PivotTables, Unsupported::Macros])
        );
    }

    #[test]
    fn a_sheet_with_unknown_links_may_own_anything_outside_the_sheets() {
        let mut held = Held::of(&found(), &layout(vec![SheetLinks::Unknown]));
        assert_eq!(
            held.rewrite(0, BTreeSet::new(), Relationships::Dropped),
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
