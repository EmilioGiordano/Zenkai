// Words of the classic lorem ipsum filler, itself derived from Cicero; public domain.
pub(crate) const WORDS: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
    "enim",
    "ad",
    "minim",
    "veniam",
    "quis",
    "nostrud",
    "exercitation",
    "ullamco",
    "laboris",
    "nisi",
    "aliquip",
    "ex",
    "ea",
    "commodo",
    "consequat",
    "duis",
    "aute",
    "irure",
    "in",
    "reprehenderit",
    "voluptate",
    "velit",
    "esse",
    "cillum",
    "eu",
    "fugiat",
    "nulla",
    "pariatur",
    "excepteur",
    "sint",
    "occaecat",
    "cupidatat",
    "non",
    "proident",
    "sunt",
    "culpa",
    "qui",
    "officia",
    "deserunt",
    "mollit",
    "anim",
    "id",
    "est",
    "laborum",
];

pub(crate) const MAX_WORDS: u16 = 200;

pub(crate) fn sentence<'a>(words: impl Iterator<Item = &'a str>) -> String {
    let mut text = String::new();
    for word in words {
        if text.is_empty() {
            let mut letters = word.chars();
            text.extend(letters.next().map(|first| first.to_ascii_uppercase()));
            text.push_str(letters.as_str());
        } else {
            text.push(' ');
            text.push_str(word);
        }
    }
    text.push('.');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentence_is_capitalised_and_ends_with_a_period() {
        assert_eq!(
            sentence(["lorem", "ipsum", "dolor"].into_iter()),
            "Lorem ipsum dolor."
        );
    }
}
