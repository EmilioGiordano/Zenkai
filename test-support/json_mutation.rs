// Structured fuzzing on stable Rust: a valid JSON document is damaged at random places, the
// way a corrupt or hostile file would be, and the parser under test must not panic or hang.
// Included by `#[path]` into the test files of crates that read JSON from outside.
use proptest::prelude::*;
use serde_json::Value;

fn pool() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Bool(true),
        Value::from(0),
        Value::from(-1),
        Value::from(255),
        Value::from(65_535),
        Value::from(4_294_967_295u64),
        Value::from(i64::MAX),
        Value::from(i64::MIN),
        Value::from(u64::MAX),
        Value::from(1e308),
        Value::from(-0.0),
        Value::from(0.5),
        Value::from(""),
        Value::from("x".repeat(300)),
        Value::from("y".repeat(40_000)),
        Value::from("\u{202e}evil"),
        Value::from("a\nb\tc"),
        Value::from("\u{0}"),
        Value::from("=cmd|' /C calc'!A0"),
        Value::from("../../etc/passwd"),
        Value::Array(Vec::new()),
        Value::Object(serde_json::Map::new()),
        Value::Array(vec![Value::Array(vec![Value::Array(Vec::new())]); 3]),
    ]
}

fn nodes(value: &Value) -> usize {
    1 + match value {
        Value::Array(items) => items.iter().map(nodes).sum(),
        Value::Object(map) => map.values().map(nodes).sum(),
        _ => 0,
    }
}

fn children(value: &mut Value) -> Vec<&mut Value> {
    match value {
        Value::Array(items) => items.iter_mut().collect(),
        Value::Object(map) => map.values_mut().collect(),
        _ => Vec::new(),
    }
}

// Applies `change` to the `n`th node in document order.
fn change_nth(value: &mut Value, n: &mut usize, change: &dyn Fn(&Value) -> Value) -> bool {
    if *n == 0 {
        *value = change(value);
        return true;
    }
    *n -= 1;
    children(value)
        .into_iter()
        .any(|child| change_nth(child, n, change))
}

fn remove_nth(value: &mut Value, n: &mut usize) -> bool {
    match value {
        Value::Array(items) => {
            for index in 0..items.len() {
                if *n == 0 {
                    items.remove(index);
                    return true;
                }
                *n -= 1;
                if remove_nth(&mut items[index], n) {
                    return true;
                }
            }
            false
        }
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                if *n == 0 {
                    map.remove(&key);
                    return true;
                }
                *n -= 1;
                if let Some(child) = map.get_mut(&key)
                    && remove_nth(child, n)
                {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

// Swaps a value for another of the same kind, so the document keeps its shape and the
// mutation reaches the checks behind the parser.
fn same_kind(value: &Value, pick: usize) -> Value {
    const NUMBERS: [i64; 10] = [0, 1, -1, 2, 7, 100, 1_000, 100_000, 5_000_000, i64::MAX];
    const WORDS: [&str; 10] = [
        "",
        "a",
        "Name",
        "x.y",
        "2024-02-30",
        "9999-99-99",
        "AAA",
        "###",
        "\\",
        "\u{200b}",
    ];
    match value {
        Value::Number(_) => Value::from(NUMBERS[pick % NUMBERS.len()]),
        Value::String(_) => Value::from(WORDS[pick % WORDS.len()]),
        Value::Bool(flag) => Value::Bool(!flag),
        Value::Array(items) if pick.is_multiple_of(2) => {
            Value::Array(items.iter().chain(items.first()).cloned().collect())
        }
        Value::Array(items) => Value::Array(items.iter().skip(1).cloned().collect()),
        other => other.clone(),
    }
}

fn damage_bytes(mut bytes: Vec<u8>, at: usize, how: u8, byte: u8) -> Vec<u8> {
    if bytes.is_empty() {
        return bytes;
    }
    let at = at % bytes.len();
    match how % 4 {
        0 => bytes.truncate(at),
        1 => {
            bytes.remove(at);
        }
        2 => bytes.insert(at, byte),
        _ => bytes[at] = byte,
    }
    bytes
}

// The bytes of `base` after one to four value-level mutations and, one time in six, a
// byte-level one that usually breaks the syntax.
pub fn mutated(base: Value) -> impl Strategy<Value = Vec<u8>> {
    let mutation = (any::<prop::sample::Index>(), 0..pool().len(), 0u8..6);
    let bytes = prop::option::weighted(
        1.0 / 6.0,
        (any::<prop::sample::Index>(), any::<u8>(), any::<u8>()),
    );
    (prop::collection::vec(mutation, 1..5), bytes).prop_map(move |(mutations, bytes)| {
        let mut value = base.clone();
        for (at, with, how) in mutations {
            let mut n = at.index(nodes(&value));
            match how {
                0 if n > 0 => {
                    remove_nth(&mut value, &mut n);
                }
                1 => {
                    let replacement = pool()[with].clone();
                    change_nth(&mut value, &mut n, &|_| replacement.clone());
                }
                _ => {
                    change_nth(&mut value, &mut n, &|old| same_kind(old, with));
                }
            }
        }
        let text = serde_json::to_vec(&value).unwrap_or_default();
        match bytes {
            Some((at, how, byte)) => damage_bytes(text, at.index(usize::MAX), how, byte),
            None => text,
        }
    })
}
