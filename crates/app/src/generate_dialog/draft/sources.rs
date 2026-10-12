use zenkai_datagen::{ColumnKind, ColumnSpec, GenerationSpec};

use super::{Draft, NameRole, Source};

impl Draft {
    pub fn source_candidates(&self, own: usize, role: NameRole) -> Vec<usize> {
        self.columns
            .iter()
            .enumerate()
            .filter(|(index, column)| *index != own && supplies(&column.kind, role))
            .map(|(index, _)| index)
            .collect()
    }

    pub fn resolved_source(&self, own: usize, role: NameRole) -> Option<usize> {
        let column = self.columns.get(own)?;
        let source = match role {
            NameRole::First => column.first_from,
            NameRole::Last => column.last_from,
        };
        match source {
            Source::NoColumn => None,
            Source::Column(index) => (index != own && index < self.columns.len()).then_some(index),
            Source::Auto => {
                let candidates = self.source_candidates(own, role);
                let exact = |index: &&usize| {
                    !matches!(self.columns[**index].kind, ColumnKind::FullName { .. })
                };
                candidates
                    .iter()
                    .find(exact)
                    .or_else(|| candidates.first())
                    .copied()
            }
        }
    }

    pub fn spec(&self) -> GenerationSpec {
        GenerationSpec {
            rows: self.rows,
            locale: self.locale,
            seed: self.seed,
            columns: (0..self.columns.len())
                .map(|index| self.column_spec(index))
                .collect(),
        }
    }

    pub(super) fn column_spec(&self, index: usize) -> ColumnSpec {
        let column = &self.columns[index];
        let mut kind = column.kind.clone();
        if let ColumnKind::Email {
            first_name_from,
            last_name_from,
            ..
        } = &mut kind
        {
            let header_of = |role| {
                self.resolved_source(index, role)
                    .map(|source| self.columns[source].header.clone())
            };
            *first_name_from = header_of(NameRole::First);
            *last_name_from = header_of(NameRole::Last);
        }
        ColumnSpec {
            header: column.header.clone(),
            kind,
            blanks: column.blanks,
            unique: column.unique,
        }
    }
}

fn supplies(kind: &ColumnKind, role: NameRole) -> bool {
    match role {
        NameRole::First => matches!(
            kind,
            ColumnKind::FirstName { .. } | ColumnKind::FullName { .. }
        ),
        NameRole::Last => matches!(
            kind,
            ColumnKind::LastName { .. } | ColumnKind::FullName { .. }
        ),
    }
}
