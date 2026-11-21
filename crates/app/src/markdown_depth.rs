use markdown::ParseOptions;
use markdown::mdast::Node;

// A tree deeper than this is not rendered: the layout recurses once per level, so a file
// built from thousands of nested quotes, lists or emphasis could exhaust the stack.
pub const MAX_NESTING: usize = 64;

// What the text view parses on the UI thread; anything longer is checked in the background.
pub const SYNC_BYTES: usize = 4 * 1024;

const TAB_WIDTH: usize = 4;
const LIST_INDENT: usize = 2;

// The parsed text when its tree is no deeper than `limit`, so nothing deeper reaches the
// renderer. The tree is what the text view will build, and it is measured without recursion.
pub fn parse_within(text: &str, limit: usize) -> Option<Node> {
    if deeper_than(text, limit) {
        return None;
    }
    let tree = match markdown::to_mdast(text, &parse_options()) {
        Ok(tree) => tree,
        Err(error) => {
            tracing::debug!(%error, "text that could not be parsed is not shown");
            return None;
        }
    };
    if depth_exceeds(&tree, limit) {
        dismantle(tree);
        return None;
    }
    Some(tree)
}

pub fn fits(text: &str) -> bool {
    parse_within(text, MAX_NESTING).is_some()
}

// The options the text view parses with, so both build the same tree.
fn parse_options() -> ParseOptions {
    let mut options = ParseOptions::gfm();
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    options
}

fn depth_exceeds(tree: &Node, limit: usize) -> bool {
    let mut pending = vec![(tree, 0usize)];
    while let Some((node, depth)) = pending.pop() {
        if depth > limit {
            return true;
        }
        if let Some(children) = node.children() {
            pending.extend(children.iter().map(|child| (child, depth + 1)));
        }
    }
    false
}

// Dropping a deep tree recurses once per level; taking the children out first does not.
fn dismantle(tree: Node) {
    let mut pending = vec![tree];
    while let Some(mut node) = pending.pop() {
        if let Some(children) = node.children_mut() {
            pending.append(children);
        }
    }
}

// A cheap pass, linear in the text, that turns away the obvious cases before they are parsed:
// per line, the block prefix (quotes and list markers) plus open brackets and emphasis.
// The parsed tree decides; this only keeps a hostile file from costing a parse.
pub fn deeper_than(text: &str, limit: usize) -> bool {
    // CommonMark ends a line at a line feed, a carriage return or both; the empty piece between
    // a carriage return and its line feed is harmless.
    text.split(['\n', '\r'])
        .any(|line| line_depth(line, limit) > limit)
}

fn line_depth(line: &str, limit: usize) -> usize {
    let (blocks, rest) = block_depth(line, limit);
    blocks + inline_depth(rest, limit)
}

fn block_depth(line: &str, limit: usize) -> (usize, &str) {
    let bytes = line.as_bytes();
    let mut depth = 0;
    let mut indent = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b' ' => indent += 1,
            b'\t' => indent += TAB_WIDTH,
            b'>' => depth += 1,
            b'-' | b'*' | b'+' if ends_marker(bytes, at + 1) => depth += 1,
            b'0'..=b'9' => match ordered_marker_end(bytes, at) {
                Some(end) => {
                    depth += 1;
                    at = end - 1;
                }
                None => break,
            },
            _ => break,
        }
        if depth > limit {
            return (depth, "");
        }
        at += 1;
    }
    (depth + indent / LIST_INDENT, &line[at..])
}

// A list marker is followed by a space, a tab or the end of the line.
fn ends_marker(bytes: &[u8], at: usize) -> bool {
    matches!(bytes.get(at), None | Some(b' ' | b'\t'))
}

// Up to nine digits and a `.` or `)`; the index after the delimiter.
fn ordered_marker_end(bytes: &[u8], start: usize) -> Option<usize> {
    let digits = bytes[start..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    let delimiter = start + digits;
    let marker = digits <= 9 && matches!(bytes.get(delimiter), Some(b'.' | b')'));
    (marker && ends_marker(bytes, delimiter + 1)).then_some(delimiter + 1)
}

fn inline_depth(text: &str, limit: usize) -> usize {
    let bytes = text.as_bytes();
    let mut brackets = 0usize;
    let mut deepest = 0usize;
    let mut emphasis: Vec<u8> = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 1,
            b'[' => {
                brackets += 1;
                deepest = deepest.max(brackets + emphasis.len());
            }
            b']' => brackets = brackets.saturating_sub(1),
            delimiter @ (b'*' | b'_') => {
                let run = bytes[at..]
                    .iter()
                    .take_while(|byte| **byte == delimiter)
                    .count();
                let opens = bytes
                    .get(at + run)
                    .is_some_and(|next| !next.is_ascii_whitespace());
                let closes = at > 0 && !bytes[at - 1].is_ascii_whitespace();
                if closes && emphasis.last() == Some(&delimiter) {
                    emphasis.pop();
                } else if opens {
                    emphasis.push(delimiter);
                    deepest = deepest.max(brackets + emphasis.len());
                }
                at += run - 1;
            }
            _ => {}
        }
        if deepest > limit {
            return deepest;
        }
        at += 1;
    }
    deepest
}

// The depth of the deepest node, measured apart from the code under test.
#[cfg(test)]
pub(crate) fn tree_depth(tree: &Node) -> usize {
    let mut deepest = 0;
    let mut pending = vec![(tree, 0usize)];
    while let Some((node, depth)) = pending.pop() {
        deepest = deepest.max(depth);
        if let Some(children) = node.children() {
            pending.extend(children.iter().map(|child| (child, depth + 1)));
        }
    }
    deepest
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use proptest::prelude::*;

    use super::*;

    fn too_deep(text: &str) -> bool {
        deeper_than(text, MAX_NESTING)
    }

    #[test]
    fn ordinary_documents_are_not_too_deep() {
        let text = "# Title\n\n> quote with **bold** and [a link](http://x.y)\n\n- item\n  - nested\n    - deeper\n\n1. one\n2. two\n";
        assert!(!too_deep(text));
        assert!(fits(text));
    }

    #[test]
    fn a_few_megabytes_of_quote_markers_are_refused_in_one_pass() {
        let text = ">".repeat(4 * 1024 * 1024);
        assert!(too_deep(&text));
        let spaced = "> ".repeat(2 * 1024 * 1024);
        assert!(too_deep(&spaced));
        assert!(parse_within(&text, MAX_NESTING).is_none());
    }

    #[test]
    fn deep_lists_brackets_and_emphasis_are_turned_away_before_parsing() {
        let list: String = (0..100)
            .map(|level| format!("{}- item\n", " ".repeat(level * 2)))
            .collect();
        assert!(too_deep(&list));
        assert!(too_deep(&"[".repeat(1_000_000)));
        assert!(too_deep(&"*a ".repeat(100_000)));
        assert!(too_deep(&"_a ".repeat(100_000)));
    }

    #[test]
    fn every_list_marker_counts_on_one_line() {
        for marker in ["1. ", "1) ", "- ", "* ", "+ ", "-\t", "10. ", "123456789. "] {
            assert!(too_deep(&marker.repeat(MAX_NESTING + 1)), "{marker:?}");
            assert!(!too_deep(&marker.repeat(MAX_NESTING)), "{marker:?}");
        }
        assert!(!too_deep(&"1234567890. ".repeat(MAX_NESTING + 1)));
        assert!(!too_deep(&"1.5 ".repeat(1000)));
    }

    #[test]
    fn lines_ended_by_a_lone_carriage_return_are_counted_like_any_other() {
        let list: String = (0..100)
            .map(|level| format!("{}- item\r", " ".repeat(level * 2)))
            .collect();
        assert!(too_deep(&list));
        let crlf: String = (0..100)
            .map(|level| format!("{}- item\r\n", " ".repeat(level * 2)))
            .collect();
        assert!(too_deep(&crlf));
        assert!(!too_deep(&"> quoted\r".repeat(100_000)));
    }

    #[test]
    fn the_limit_itself_is_allowed() {
        assert!(!too_deep(&">".repeat(MAX_NESTING)));
        assert!(too_deep(&">".repeat(MAX_NESTING + 1)));
    }

    #[test]
    fn a_stricter_limit_refuses_what_the_default_allows() {
        let text = ">".repeat(20);
        assert!(!too_deep(&text));
        assert!(deeper_than(&text, 16));
        assert!(!deeper_than(&">".repeat(16), 16));
    }

    #[test]
    fn many_lines_that_are_each_shallow_are_fine() {
        assert!(!too_deep(&"> quoted line\n".repeat(200_000)));
        assert!(!too_deep(&"**bold** and *it* and [x](y)\n".repeat(100_000)));
    }

    // The case the line count cannot see: one level per line, each line shallow.
    #[test]
    fn emphasis_opened_on_each_line_and_closed_later_is_refused() {
        for (open, close) in [
            ("*", "*"),
            ("_", "_"),
            ("**", "**"),
            ("__", "__"),
            ("~~", "~~"),
        ] {
            let text = format!(
                "{}\n{}\n",
                format!("{open}a\n").repeat(2_000),
                format!("a{close}\n").repeat(2_000)
            );
            assert!(!too_deep(&text), "{open}");
            assert!(parse_within(&text, MAX_NESTING).is_none(), "{open}");
        }
    }

    #[test]
    fn quotes_and_ordered_lists_deeper_than_the_limit_are_refused_whatever_the_markers() {
        let ordered: String = (0..100)
            .map(|level| format!("{}1. item\n", " ".repeat(level * 3)))
            .collect();
        let quoted: String = (0..100)
            .map(|level| format!("{}> item\n", "> ".repeat(level)))
            .collect();
        let mixed: String = (0..100)
            .map(|level| match level % 3 {
                0 => format!("{}- item\n", " ".repeat(level * 2)),
                1 => format!("{}1) item\n", " ".repeat(level * 2)),
                _ => format!("{}> item\n", " ".repeat(level)),
            })
            .collect();
        for text in [ordered, quoted, mixed] {
            assert!(parse_within(&text, MAX_NESTING).is_none());
        }
    }

    #[test]
    fn a_document_within_the_limit_gives_its_tree() {
        let tree = parse_within("- a\n  - b *c*\n", MAX_NESTING).unwrap();
        assert!(tree_depth(&tree) <= MAX_NESTING);
        assert!(parse_within("- a\n  - b *c*\n", 3).is_none());
    }

    const PIECES: &[&str] = &[
        "> ", ">", "- ", "* ", "+ ", "1. ", "1) ", "10. ", "\t", "  ", "    ", "-\t", "*", "**",
        "***", "_", "__", "~", "~~", "[", "](x)", "](", "a ", "a", "\n", "\n  ", "\n\n", "`", "<",
        "![",
    ];

    fn assembled() -> impl Strategy<Value = String> {
        prop::collection::vec(prop::sample::select(PIECES.to_vec()), 0..80)
            .prop_map(|parts| parts.concat())
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

        #[test]
        fn accepted_text_never_parses_deeper_than_the_limit(text in assembled(), limit in 2usize..40) {
            if let Some(tree) = parse_within(&text, limit) {
                prop_assert!(tree_depth(&tree) <= limit);
            }
        }

        #[test]
        fn text_that_nests_by_repetition_is_refused_or_within_the_limit(
            piece in prop::sample::select(PIECES.to_vec()),
            times in 1usize..300,
        ) {
            let text = piece.repeat(times);
            if let Some(tree) = parse_within(&text, MAX_NESTING) {
                prop_assert!(tree_depth(&tree) <= MAX_NESTING);
            }
        }
    }

    // The parse runs for every note in the list, so no 64 KiB text may take long to refuse or
    // to accept. The bound is loose: it catches a growth that is worse than linear.
    #[test]
    fn adversarial_notes_of_the_size_limit_are_parsed_in_reasonable_time() {
        let size = 64 * 1024;
        for piece in [
            "[a]", "*a ", "<", "www.", "![", "[a](", "> ", "1. ", "`", "&amp", "\\",
        ] {
            let text = piece.repeat(size / piece.len());
            let started = Instant::now();
            let _ = parse_within(&text, 32);
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "{piece:?} took {:?}",
                started.elapsed()
            );
        }
    }
}
