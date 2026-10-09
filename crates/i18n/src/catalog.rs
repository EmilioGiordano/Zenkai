use std::collections::HashMap;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CatalogError {
    #[error("line {line}: expected `key = text`")]
    MissingSeparator { line: usize },
    #[error("line {line}: the key `{key}` is defined twice")]
    DuplicateKey { line: usize, key: String },
}

#[derive(Debug, Default)]
pub struct Catalog {
    entries: HashMap<&'static str, &'static str>,
}

impl Catalog {
    pub fn parse(text: &'static str) -> Result<Catalog, CatalogError> {
        let mut entries = HashMap::new();
        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or(CatalogError::MissingSeparator { line: line_number })?;
            let key = key.trim();
            if entries.insert(key, value.trim()).is_some() {
                return Err(CatalogError::DuplicateKey {
                    line: line_number,
                    key: key.to_string(),
                });
            }
        }
        Ok(Catalog { entries })
    }

    pub fn get(&self, key: &str) -> Option<&'static str> {
        self.entries.get(key).copied()
    }

    pub fn keys(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.entries.keys().copied()
    }
}
