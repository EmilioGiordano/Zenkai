use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SummaryItem {
    Average,
    Count,
    NumericalCount,
    Min,
    Max,
    Sum,
}

impl SummaryItem {
    pub const ALL: [SummaryItem; 6] = [
        SummaryItem::Average,
        SummaryItem::Count,
        SummaryItem::NumericalCount,
        SummaryItem::Min,
        SummaryItem::Max,
        SummaryItem::Sum,
    ];
}

// Items are kept in Excel's order whatever order the file lists them in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(
    description = "Which figures the status bar shows for the selected cells, in Excel's order: average, count, numerical_count, min, max, sum."
)]
pub struct StatusSummary(Vec<SummaryItem>);

impl Default for StatusSummary {
    fn default() -> StatusSummary {
        StatusSummary(vec![
            SummaryItem::Average,
            SummaryItem::Count,
            SummaryItem::Sum,
        ])
    }
}

impl StatusSummary {
    pub fn from_items(items: impl IntoIterator<Item = SummaryItem>) -> StatusSummary {
        let mut items: Vec<SummaryItem> = items.into_iter().collect();
        items.sort_unstable();
        items.dedup();
        StatusSummary(items)
    }

    pub fn items(&self) -> &[SummaryItem] {
        &self.0
    }

    pub fn contains(&self, item: SummaryItem) -> bool {
        self.0.contains(&item)
    }

    pub fn toggled(&self, item: SummaryItem) -> StatusSummary {
        if self.contains(item) {
            StatusSummary::from_items(self.0.iter().copied().filter(|shown| *shown != item))
        } else {
            StatusSummary::from_items(self.0.iter().copied().chain([item]))
        }
    }
}

// A name this version does not know is skipped, and anything that is not a list gives the
// default, so a settings.json written by another version still opens.
impl<'de> Deserialize<'de> for StatusSummary {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<StatusSummary, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Entry {
            Known(SummaryItem),
            Unknown(serde::de::IgnoredAny),
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            List(Vec<Entry>),
            Other(serde::de::IgnoredAny),
        }
        Ok(match Written::deserialize(deserializer)? {
            Written::List(entries) => {
                StatusSummary::from_items(entries.into_iter().filter_map(|entry| match entry {
                    Entry::Known(item) => Some(item),
                    Entry::Unknown(_) => None,
                }))
            }
            Written::Other(_) => StatusSummary::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> StatusSummary {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn the_default_is_excels_average_count_sum() {
        assert_eq!(
            StatusSummary::default().items(),
            [SummaryItem::Average, SummaryItem::Count, SummaryItem::Sum]
        );
    }

    #[test]
    fn toggling_adds_in_excels_order_and_removes() {
        let summary = StatusSummary::default().toggled(SummaryItem::Min);
        assert_eq!(
            summary.items(),
            [
                SummaryItem::Average,
                SummaryItem::Count,
                SummaryItem::Min,
                SummaryItem::Sum
            ]
        );
        let summary = summary
            .toggled(SummaryItem::Average)
            .toggled(SummaryItem::Sum);
        assert_eq!(summary.items(), [SummaryItem::Count, SummaryItem::Min]);
    }

    #[test]
    fn a_choice_survives_a_round_trip() {
        let summary = StatusSummary::from_items([SummaryItem::NumericalCount, SummaryItem::Max]);
        let text = serde_json::to_string(&summary).unwrap();
        assert_eq!(text, r#"["numerical_count","max"]"#);
        assert_eq!(read(&text), summary);
    }

    #[test]
    fn an_empty_list_shows_nothing() {
        assert!(read("[]").items().is_empty());
    }

    #[test]
    fn unknown_names_and_duplicates_are_dropped_and_order_is_restored() {
        let summary = read(r#"["sum", "median", 7, "sum", "average", null]"#);
        assert_eq!(summary.items(), [SummaryItem::Average, SummaryItem::Sum]);
    }

    #[test]
    fn anything_but_a_list_gives_the_default() {
        for text in ["\"sum\"", "3", "null", "{}"] {
            assert_eq!(read(text), StatusSummary::default(), "{text}");
        }
    }
}
