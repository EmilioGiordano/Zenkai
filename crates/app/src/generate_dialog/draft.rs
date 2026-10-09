use zenkai_datagen::{ColumnKind, Date, Locale, Percent};

use super::layout::{Layout, MAX_COLUMNS};
use super::options::{self, KindChoice, OptionError};

pub const DEFAULT_ROWS: u32 = 1000;
pub const PREVIEW_ROWS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindSource {
    Detected,
    Fallback,
    Chosen,
}

impl KindSource {
    fn follows_header(self) -> bool {
        self != KindSource::Chosen
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Auto,
    NoColumn,
    Column(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameRole {
    First,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    BelowHeaders,
    AfterData,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("blanks must be a whole percentage from 0 to 100, not \"{0}\"")]
pub struct InvalidBlanks(pub String);

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FieldIssue {
    #[error("the row count must be a whole number from 1, not \"{0}\"")]
    Rows(String),
    #[error("the seed must be a whole number, not \"{0}\"")]
    Seed(String),
    #[error("\"{0}\" is not a range such as A1:E1")]
    Range(String),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WriteIssue {
    #[error("the rows would run past the last row of the sheet")]
    PastLastRow,
    #[error("the columns would run past the last column of the sheet")]
    PastLastColumn,
    #[error(
        "unsaved headers cannot be written with rows placed after the data; discard them or place the rows below the headers"
    )]
    HeadersWithAppendedRows,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnDraft {
    pub header: String,
    pub original: String,
    pub kind: ColumnKind,
    pub kind_source: KindSource,
    pub first_from: Source,
    pub last_from: Source,
    pub blanks: Percent,
    pub unique: bool,
    pub blanks_issue: Option<InvalidBlanks>,
    pub options_issue: Option<OptionError>,
}

impl ColumnDraft {
    pub fn header_changed(&self) -> bool {
        self.header != self.original
    }

    pub fn local_issue(&self) -> Option<String> {
        self.blanks_issue
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| self.options_issue.as_ref().map(ToString::to_string))
    }
}

mod sources;
#[cfg(test)]
mod tests;
mod write;

pub use write::{Block, count_label, parse_range};

pub struct Draft {
    layout: Layout,
    columns: Vec<ColumnDraft>,
    rows: u32,
    placement: Placement,
    locale: Locale,
    seed: u64,
    today: Date,
    range_issue: Option<FieldIssue>,
    rows_issue: Option<FieldIssue>,
    seed_issue: Option<FieldIssue>,
}

impl Draft {
    pub fn new(layout: Layout, locale: Locale, seed: u64, today: Date) -> Draft {
        let rows = match layout.data_rows() {
            0 => DEFAULT_ROWS,
            selected => selected,
        };
        let mut draft = Draft {
            columns: Vec::new(),
            layout,
            rows,
            placement: Placement::BelowHeaders,
            locale,
            seed,
            today,
            range_issue: None,
            rows_issue: None,
            seed_issue: None,
        };
        draft.columns = draft.detected_columns(&draft.layout.headers);
        draft
    }

    fn detected_columns(&self, headers: &[String]) -> Vec<ColumnDraft> {
        headers
            .iter()
            .map(|header| self.detected_column(header.clone(), header.clone()))
            .collect()
    }

    fn detected_column(&self, header: String, original: String) -> ColumnDraft {
        let (kind, kind_source) = match options::detect(&header, self.locale, self.today) {
            Some(kind) => (kind, KindSource::Detected),
            None => (self.fallback_kind(), KindSource::Fallback),
        };
        ColumnDraft {
            header,
            original,
            kind,
            kind_source,
            first_from: Source::Auto,
            last_from: Source::Auto,
            blanks: Percent::ZERO,
            unique: false,
            blanks_issue: None,
            options_issue: None,
        }
    }

    fn fallback_kind(&self) -> ColumnKind {
        KindChoice::Lorem.default_kind(self.locale, self.today)
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn columns(&self) -> &[ColumnDraft] {
        &self.columns
    }

    pub fn rows(&self) -> u32 {
        self.rows
    }

    pub fn rows_locked(&self) -> bool {
        self.layout.data_rows() > 0 && self.placement == Placement::BelowHeaders
    }

    pub fn placement(&self) -> Placement {
        self.placement
    }

    pub fn locale(&self) -> Locale {
        self.locale
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn range_text(&self) -> String {
        self.layout.range.to_string()
    }

    pub fn field_issue(&self) -> Option<&FieldIssue> {
        self.range_issue
            .as_ref()
            .or(self.rows_issue.as_ref())
            .or(self.seed_issue.as_ref())
    }

    pub fn can_add_column(&self) -> bool {
        self.columns.len() < usize::from(MAX_COLUMNS)
    }

    pub fn is_added(&self, index: usize) -> bool {
        index >= self.layout.headers.len()
    }

    pub fn relayout(&mut self, layout: Layout) {
        self.range_issue = None;
        let mut columns = Vec::with_capacity(layout.headers.len());
        for (index, header) in layout.headers.iter().enumerate() {
            columns.push(match self.columns.get(index) {
                Some(edited) if edited.header_changed() => ColumnDraft {
                    original: header.clone(),
                    ..edited.clone()
                },
                _ => self.detected_column(header.clone(), header.clone()),
            });
        }
        self.columns = columns;
        self.layout = layout;
        if self.layout.data_rows() > 0 && self.placement == Placement::BelowHeaders {
            self.rows = self.layout.data_rows();
            self.rows_issue = None;
        }
    }

    pub fn reject_range(&mut self, text: &str) {
        self.range_issue = Some(FieldIssue::Range(text.to_string()));
    }

    pub fn accept_range(&mut self) {
        self.range_issue = None;
    }

    pub fn add_column(&mut self) {
        if self.can_add_column() {
            let column = self.detected_column(String::new(), String::new());
            self.columns.push(column);
        }
    }

    pub fn remove_added_column(&mut self, index: usize) {
        if self.is_added(index) && index + 1 == self.columns.len() {
            self.columns.pop();
        }
    }

    pub fn rename(&mut self, index: usize, text: &str) {
        let detected = options::detect(text, self.locale, self.today);
        let Some(column) = self.columns.get_mut(index) else {
            return;
        };
        column.header = text.to_string();
        if !column.kind_source.follows_header() {
            return;
        }
        match detected {
            Some(kind) => {
                if KindChoice::of(&kind) != KindChoice::of(&column.kind) {
                    column.kind = kind;
                    column.options_issue = None;
                }
                column.kind_source = KindSource::Detected;
            }
            None => {
                if column.kind_source == KindSource::Detected {
                    column.kind_source = KindSource::Chosen;
                }
            }
        }
    }

    // A type that only came from the edited header goes back with it.
    pub fn discard(&mut self, index: usize) {
        let Some(column) = self.columns.get(index) else {
            return;
        };
        let original = column.original.clone();
        let from_header =
            options::detect(&column.header, self.locale, self.today).as_ref() == Some(&column.kind);
        if from_header {
            let restored = self.detected_column(original.clone(), original);
            let column = &mut self.columns[index];
            column.header = restored.header;
            column.kind = restored.kind;
            column.kind_source = restored.kind_source;
            column.options_issue = None;
        } else {
            self.rename(index, &original);
        }
    }

    pub fn set_kind_choice(&mut self, index: usize, choice: KindChoice) {
        let kind = choice.default_kind(self.locale, self.today);
        if let Some(column) = self.columns.get_mut(index) {
            column.kind = kind;
            column.kind_source = KindSource::Chosen;
            column.options_issue = None;
        }
    }

    pub fn set_kind(&mut self, index: usize, kind: ColumnKind) {
        if let Some(column) = self.columns.get_mut(index) {
            column.kind = kind;
            column.options_issue = None;
        }
    }

    pub fn set_option_fields(&mut self, index: usize, values: &[String]) {
        let Some(column) = self.columns.get_mut(index) else {
            return;
        };
        match options::with_fields(&column.kind, values) {
            Ok(kind) => {
                column.kind = kind;
                column.options_issue = None;
            }
            Err(error) => column.options_issue = Some(error),
        }
    }

    pub fn set_blanks(&mut self, index: usize, text: &str) {
        let Some(column) = self.columns.get_mut(index) else {
            return;
        };
        let typed = text.trim().trim_end_matches('%').trim();
        let parsed = if typed.is_empty() {
            Some(Percent::ZERO)
        } else {
            typed
                .parse::<u8>()
                .ok()
                .and_then(|value| Percent::new(value).ok())
        };
        match parsed {
            Some(percent) => {
                column.blanks = percent;
                column.blanks_issue = None;
            }
            None => column.blanks_issue = Some(InvalidBlanks(text.to_string())),
        }
    }

    pub fn set_unique(&mut self, index: usize, unique: bool) {
        if let Some(column) = self.columns.get_mut(index) {
            column.unique = unique;
        }
    }

    pub fn set_source(&mut self, index: usize, role: NameRole, source: Source) {
        if let Some(column) = self.columns.get_mut(index) {
            match role {
                NameRole::First => column.first_from = source,
                NameRole::Last => column.last_from = source,
            }
        }
    }

    pub fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
        let today = self.today;
        for column in &mut self.columns {
            if !column.kind_source.follows_header() {
                continue;
            }
            if let Some(kind) = options::detect(&column.header, locale, today) {
                column.kind = kind;
                column.kind_source = KindSource::Detected;
                column.options_issue = None;
            }
        }
    }

    pub fn set_placement(&mut self, placement: Placement) {
        self.placement = placement;
        if self.rows_locked() {
            self.rows = self.layout.data_rows();
            self.rows_issue = None;
        }
    }

    pub fn set_rows_text(&mut self, text: &str) {
        let digits: String = text
            .chars()
            .filter(|symbol| !matches!(symbol, ',' | '.' | ' ' | '_'))
            .collect();
        match digits.parse::<u32>() {
            Ok(rows) if rows > 0 => {
                self.rows = rows;
                self.rows_issue = None;
            }
            _ => self.rows_issue = Some(FieldIssue::Rows(text.to_string())),
        }
    }

    pub fn set_seed_text(&mut self, text: &str) {
        match text.trim().parse::<u64>() {
            Ok(seed) => {
                self.seed = seed;
                self.seed_issue = None;
            }
            Err(_) => self.seed_issue = Some(FieldIssue::Seed(text.to_string())),
        }
    }

    pub fn changed_headers(&self) -> usize {
        self.columns
            .iter()
            .filter(|column| column.header_changed())
            .count()
    }

    pub fn has_local_issue(&self) -> bool {
        self.field_issue().is_some()
            || self
                .columns
                .iter()
                .any(|column| column.local_issue().is_some())
    }
}
