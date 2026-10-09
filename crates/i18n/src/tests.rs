use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use zenkai_types::Language;

use super::*;
use crate::t;

fn translator(language: Language, primary: &'static str, fallback: &'static str) -> Translator {
    let load = |text| Catalog::parse(text).unwrap();
    Translator::new(language, load(primary), load(fallback))
}

fn keys_of(language: Language) -> BTreeSet<&'static str> {
    embedded_catalog(language).unwrap().keys().collect()
}

fn placeholders(text: &str) -> BTreeSet<&str> {
    text.split('{')
        .skip(1)
        .filter_map(|tail| tail.split_once('}').map(|(name, _)| name))
        .collect()
}

fn base_key(key: &str) -> &str {
    key.strip_suffix(".one")
        .or_else(|| key.strip_suffix(".other"))
        .unwrap_or(key)
}

fn used_keys() -> BTreeSet<String> {
    fn visit(folder: &Path, keys: &mut BTreeSet<String>) {
        for entry in fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, keys);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                collect(&fs::read_to_string(&path).unwrap(), keys);
            }
        }
    }
    fn collect(source: &str, keys: &mut BTreeSet<String>) {
        let mut rest = source;
        while let Some(at) = rest.find("t!(") {
            let before = rest[..at].chars().next_back();
            let tail = rest[at + 3..].trim_start();
            if !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
                && let Some(literal) = tail.strip_prefix('"')
                && let Some((key, _)) = literal.split_once('"')
            {
                keys.insert(key.to_string());
            }
            rest = &rest[at + 3..];
        }
    }
    let mut keys = BTreeSet::new();
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for member in ["app", "grid"] {
        visit(&crates.join(member).join("src"), &mut keys);
    }
    keys
}

#[test]
fn both_catalogs_parse() {
    assert!(embedded_catalog(Language::English).is_ok());
    assert!(embedded_catalog(Language::Spanish).is_ok());
}

#[test]
fn catalogs_define_the_same_keys_with_the_same_placeholders() {
    let english = embedded_catalog(Language::English).unwrap();
    let spanish = embedded_catalog(Language::Spanish).unwrap();
    assert_eq!(
        keys_of(Language::English)
            .symmetric_difference(&keys_of(Language::Spanish))
            .collect::<Vec<_>>(),
        Vec::<&&str>::new()
    );
    for key in english.keys() {
        let in_english = placeholders(english.get(key).unwrap());
        let in_spanish = placeholders(spanish.get(key).unwrap());
        assert_eq!(in_english, in_spanish, "placeholders of {key}");
    }
}

#[test]
fn plural_keys_come_in_pairs() {
    let keys = keys_of(Language::English);
    for key in &keys {
        if let Some(base) = key.strip_suffix(".one") {
            assert!(keys.contains(format!("{base}.other").as_str()), "{base}");
        }
        if let Some(base) = key.strip_suffix(".other") {
            assert!(keys.contains(format!("{base}.one").as_str()), "{base}");
        }
    }
}

#[test]
fn every_key_used_in_the_code_exists_and_every_key_is_used() {
    let defined: BTreeSet<String> = keys_of(Language::English)
        .into_iter()
        .map(|key| base_key(key).to_string())
        .collect();
    let used = used_keys();
    assert_eq!(
        used.difference(&defined).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "keys used but not defined"
    );
    assert_eq!(
        defined.difference(&used).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "keys defined but never used"
    );
}

#[test]
fn copy_has_no_middle_dot() {
    for text in [super::ENGLISH, super::SPANISH] {
        assert!(!text.contains('\u{b7}'));
    }
}

#[test]
fn variables_are_substituted() {
    let english = translator(Language::English, "saved = Saved {name} to {folder}", "");
    let text = english.format(
        "saved",
        &Arguments {
            count: None,
            named: &[("name", "a.xlsx".into()), ("folder", "Docs".into())],
        },
    );
    assert_eq!(text, "Saved a.xlsx to Docs");
}

#[test]
fn plurals_follow_the_count_in_both_languages() {
    let count = |translator: &Translator, number| {
        translator.format(
            "rows",
            &Arguments {
                count: Some(number),
                named: &[],
            },
        )
    };
    let english = translator(
        Language::English,
        "rows.one = {count} row\nrows.other = {count} rows",
        "",
    );
    assert_eq!(count(&english, 1), "1 row");
    assert_eq!(count(&english, 0), "0 rows");
    assert_eq!(count(&english, 1000), "1,000 rows");
    assert_eq!(count(&english, 1_234_567), "1,234,567 rows");
    let spanish = translator(
        Language::Spanish,
        "rows.one = {count} fila\nrows.other = {count} filas",
        "",
    );
    assert_eq!(count(&spanish, 1), "1 fila");
    assert_eq!(count(&spanish, 1000), "1.000 filas");
    assert_eq!(count(&spanish, 999), "999 filas");
}

#[test]
fn a_missing_key_falls_back_to_english_and_is_reported_once() {
    let spanish = translator(
        Language::Spanish,
        "hello = Hola",
        "hello = Hello\nbye = Bye",
    );
    assert_eq!(spanish.text("hello"), "Hola");
    assert_eq!(spanish.text("bye"), "Bye");
    assert_eq!(spanish.text("bye"), "Bye");
    assert_eq!(spanish.reported_count(), 1);
}

#[test]
fn a_count_on_a_key_without_plural_forms_is_not_a_missing_translation() {
    let spanish = translator(
        Language::Spanish,
        "found = {count} encontradas",
        "found = {count} found",
    );
    let text = spanish.format(
        "found",
        &Arguments {
            count: Some(2500),
            named: &[],
        },
    );
    assert_eq!(text, "2.500 encontradas");
    assert_eq!(spanish.reported_count(), 0);
}

#[test]
fn a_key_missing_everywhere_shows_the_key() {
    let spanish = translator(Language::Spanish, "", "");
    assert_eq!(spanish.text("no.such.key"), "no.such.key");
}

#[test]
fn a_malformed_catalog_is_an_error_not_a_panic() {
    assert!(matches!(
        Catalog::parse("no separator here"),
        Err(CatalogError::MissingSeparator { line: 1 })
    ));
    assert!(matches!(
        Catalog::parse("a = 1\na = 2"),
        Err(CatalogError::DuplicateKey { line: 2, .. })
    ));
}

#[test]
fn the_macro_picks_text_variables_and_plurals() {
    assert_eq!(t!("no.such.key"), "no.such.key");
    assert_eq!(t!("no.such.key", count = 3, name = "x"), "no.such.key");
}
