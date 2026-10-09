#![forbid(unsafe_code)]

mod catalog;
#[cfg(test)]
mod tests;
mod translator;

use std::sync::OnceLock;

use zenkai_types::Language;

pub use catalog::{Catalog, CatalogError};

pub use translator::{Arguments, Translator};

const ENGLISH: &str = include_str!("../locales/en.properties");
const SPANISH: &str = include_str!("../locales/es.properties");

static ACTIVE: OnceLock<Translator> = OnceLock::new();

pub fn embedded_catalog(language: Language) -> Result<Catalog, CatalogError> {
    Catalog::parse(match language {
        Language::English => ENGLISH,
        Language::Spanish => SPANISH,
    })
}

fn build(language: Language) -> Translator {
    let load = |language| {
        embedded_catalog(language).unwrap_or_else(|error| {
            tracing::error!(%error, ?language, "the embedded catalog is malformed");
            Catalog::default()
        })
    };
    Translator::new(language, load(language), load(Language::English))
}

pub fn init(language: Language) {
    if ACTIVE.set(build(language)).is_err() {
        tracing::warn!("the interface language was already set; keeping it");
    }
}

pub fn active() -> &'static Translator {
    ACTIVE.get_or_init(|| build(Language::English))
}

pub fn number(value: u64) -> String {
    active().group_digits(value)
}

#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::active().text($key)
    };
    ($key:literal, count = $count:expr $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::active().format(
            $key,
            &$crate::Arguments {
                count: Some(($count) as u64),
                named: &[$((stringify!($name), ($value).to_string())),*],
            },
        )
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::active().format(
            $key,
            &$crate::Arguments {
                count: None,
                named: &[$((stringify!($name), ($value).to_string())),+],
            },
        )
    };
}
