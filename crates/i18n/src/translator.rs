use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::PoisonError;

use zenkai_types::Language;

use crate::catalog::Catalog;

pub struct Arguments<'a> {
    pub count: Option<u64>,
    pub named: &'a [(&'static str, String)],
}

pub struct Translator {
    language: Language,
    primary: Catalog,
    fallback: Catalog,
    reported: Mutex<HashSet<String>>,
}

impl Translator {
    pub fn new(language: Language, primary: Catalog, fallback: Catalog) -> Translator {
        Translator {
            language,
            primary,
            fallback,
            reported: Mutex::new(HashSet::new()),
        }
    }

    pub fn text(&self, key: &'static str) -> &'static str {
        self.find(key).unwrap_or(key)
    }

    pub fn format(&self, key: &'static str, arguments: &Arguments) -> String {
        let template = match arguments.count {
            Some(1) => self.find_plural(key, "one"),
            Some(_) => self.find_plural(key, "other"),
            None => None,
        };
        let template = template.or_else(|| self.find(key)).unwrap_or(key);
        let mut result = String::with_capacity(template.len() + 8);
        let mut rest = template;
        while let Some(start) = rest.find('{') {
            result.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let Some(end) = after.find('}') else {
                result.push_str(&rest[start..]);
                return result;
            };
            let name = &after[..end];
            match self.value_of(name, arguments) {
                Some(value) => result.push_str(&value),
                None => result.push_str(&rest[start..start + end + 2]),
            }
            rest = &after[end + 1..];
        }
        result.push_str(rest);
        result
    }

    fn value_of(&self, name: &str, arguments: &Arguments) -> Option<String> {
        if name == "count" {
            return arguments.count.map(|count| self.group_digits(count));
        }
        arguments
            .named
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| value.clone())
    }

    pub fn group_digits(&self, number: u64) -> String {
        let separator = match self.language {
            Language::English => ',',
            Language::Spanish => '.',
        };
        let digits = number.to_string();
        let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
        for (index, digit) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                grouped.push(separator);
            }
            grouped.push(digit);
        }
        grouped
    }

    fn find_plural(&self, key: &str, form: &str) -> Option<&'static str> {
        self.find(&format!("{key}.{form}"))
    }

    fn find(&self, key: &str) -> Option<&'static str> {
        if let Some(text) = self.primary.get(key) {
            return Some(text);
        }
        self.report_missing(key);
        self.fallback.get(key)
    }

    fn report_missing(&self, key: &str) {
        let first_time = self
            .reported
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_string());
        if first_time {
            tracing::warn!(key, language = ?self.language, "missing translation, using English");
        }
    }

    #[cfg(test)]
    pub fn reported_count(&self) -> usize {
        self.reported
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}
