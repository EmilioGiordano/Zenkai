// Structured fuzzing of the Markdown depth check, which reads text from outside: an opened
// .md file and a release note. The parser underneath unwraps in places, so any text must be
// refused or accepted without a panic, and whatever is accepted stays within the limits.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "the module is included whole; the test uses only part of it"
)]

#[path = "../src/markdown_depth.rs"]
mod markdown_depth;

use markdown_depth::{MAX_NESTING, MAX_PREVIEW_BYTES, Refusal, parse_within, preview_refusal};
use proptest::prelude::*;

const SEEDS: &[&str] = &[
    "# Title\n\n> quote with **bold** and [a link](https://x.y)\n\n- item\n  - nested\n    - deeper\n\n1. one\n2. two\n",
    "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 |\n\nText with <b>html</b> and $math$ and [^1].\n\n[^1]: the note\n",
    "```rust\nlet a = 1;\n```\n\n~~~\nplain\n~~~\n\n*a\n*b\nc*\nd*\n\n![alt](img.png) <!-- c --> <br/>\n",
    "Setext\n======\n\n- [ ] task\n- [x] done\n\nhttps://example.com and www.example.com\n\n***\n",
];

const PIECES: &[&str] = &[
    "> ",
    "- ",
    "1. ",
    "1) ",
    "\t",
    "  ",
    "*",
    "**",
    "***",
    "_",
    "__",
    "~",
    "~~",
    "$",
    "$$",
    "[",
    "](x)",
    "](",
    "[^a]: ",
    "[^a]",
    "<b>",
    "</b>",
    "<br>",
    "<!--",
    "-->",
    "|a|b|\n|-|-|\n",
    "|x|\n",
    "```\n",
    "~~~\n",
    "# ",
    "#tag ",
    "a* ",
    "a_ ",
    "&amp;",
    "\\",
    "www.",
    "![",
    "`",
    "\n",
    "\n\n",
    "a ",
    "a",
];

fn mutated() -> impl Strategy<Value = String> {
    let edit = (
        0usize..4,
        any::<prop::sample::Index>(),
        any::<prop::sample::Index>(),
        prop::sample::select(PIECES.to_vec()),
    );
    (
        prop::sample::select(SEEDS.to_vec()),
        prop::collection::vec(edit, 0..14),
    )
        .prop_map(|(seed, edits)| {
            let mut text: Vec<char> = seed.chars().collect();
            for (kind, first, second, piece) in edits {
                let start = first.index(text.len() + 1);
                let end = (start + second.index(40)).min(text.len());
                match kind {
                    0 => {
                        text.splice(start..start, piece.chars());
                    }
                    1 => {
                        text.drain(start..end);
                    }
                    2 => {
                        let copy: Vec<char> = text[start..end].to_vec();
                        text.splice(end..end, copy);
                    }
                    _ => {
                        text.splice(start..end, piece.chars());
                    }
                }
            }
            text.into_iter().collect()
        })
}

fn assembled() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(PIECES.to_vec()), 0..120)
        .prop_map(|parts| parts.concat())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

    #[test]
    fn a_mutated_document_is_refused_or_within_the_limit(text in mutated(), limit in 2usize..70) {
        if let Ok(tree) = parse_within(&text, limit) {
            prop_assert!(markdown_depth::tree_depth(&tree) <= limit);
        }
    }

    #[test]
    fn a_text_made_of_markup_pieces_is_refused_or_within_the_limit(text in assembled()) {
        if let Ok(tree) = parse_within(&text, MAX_NESTING) {
            prop_assert!(markdown_depth::tree_depth(&tree) <= MAX_NESTING);
        }
        prop_assert_eq!(
            preview_refusal(&text).is_none(),
            parse_within(&text, MAX_NESTING).is_ok()
        );
    }

    #[test]
    fn any_bytes_are_a_text_that_is_refused_or_accepted(bytes in prop::collection::vec(any::<u8>(), 0..600)) {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let _ = parse_within(&text, MAX_NESTING);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    // A piece repeated up to the byte cap is the shape a hostile file takes.
    #[test]
    fn a_piece_repeated_up_to_the_byte_cap_is_refused_or_within_the_limit(
        piece in prop::sample::select(PIECES.to_vec()),
        fraction in 1usize..=100,
    ) {
        let times = MAX_PREVIEW_BYTES / piece.len() * fraction / 100;
        let text = piece.repeat(times);
        match preview_refusal(&text) {
            None => {
                let tree = parse_within(&text, MAX_NESTING).unwrap();
                prop_assert!(markdown_depth::tree_depth(&tree) <= MAX_NESTING);
            }
            Some(refusal) => prop_assert!(matches!(refusal, Refusal::TooDeep | Refusal::TooLarge)),
        }
    }
}
