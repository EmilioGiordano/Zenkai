use markdown::ParseOptions;
use markdown::mdast::Node;

// A tree deeper than this is not rendered: the layout recurses once per level, so a file
// built from thousands of nested quotes, lists or emphasis could exhaust the stack.
pub const MAX_NESTING: usize = 64;

// What the text view parses on the UI thread; anything longer is checked in the background.
pub const SYNC_BYTES: usize = 4 * 1024;

// The parser's cost grows faster than the text, so a preview is not offered past this size.
pub const MAX_PREVIEW_BYTES: usize = 1024 * 1024;

const TAB_WIDTH: usize = 4;
const LIST_INDENT: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    TooDeep,
    TooLarge,
}

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

pub fn preview_refusal(text: &str) -> Option<Refusal> {
    if text.len() > MAX_PREVIEW_BYTES {
        return Some(Refusal::TooLarge);
    }
    parse_within(text, MAX_NESTING)
        .is_none()
        .then_some(Refusal::TooDeep)
}

// The options the text view parses with first, so the checked tree is the rendered one. The
// view's plugins claim math spans, which keeps it from parsing their text again as prose.
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

// A cheap pass, linear in the text, that turns away the obvious cases before they are parsed,
// because the parser costs more than the text is long when the nesting is deep: per line, the
// block prefix (quotes and list markers) plus the inline openers still open in the paragraph
// (brackets and emphasis, carried across its lines). The parsed tree decides; this only keeps
// a hostile file from costing a parse.
pub fn deeper_than(text: &str, limit: usize) -> bool {
    let mut paragraph = Inline::default();
    let mut in_fence = false;
    // CommonMark ends a line at a line feed, a carriage return or both; the empty piece between
    // a carriage return and its line feed is harmless.
    text.split(['\n', '\r'])
        .any(|line| line_exceeds(line, limit, &mut paragraph, &mut in_fence))
}

fn line_exceeds(line: &str, limit: usize, paragraph: &mut Inline, in_fence: &mut bool) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        *in_fence = !*in_fence;
        paragraph.clear();
        return false;
    }
    if *in_fence {
        return false;
    }
    if trimmed.is_empty() {
        paragraph.clear();
        return false;
    }
    let (blocks, starts_item, rest) = block_depth(line, limit);
    if blocks > limit {
        return true;
    }
    if starts_item || rest.starts_with('#') {
        paragraph.clear();
    }
    blocks + paragraph.scan(rest, limit) > limit
}

fn block_depth(line: &str, limit: usize) -> (usize, bool, &str) {
    let bytes = line.as_bytes();
    let mut depth = 0;
    let mut indent = 0;
    let mut starts_item = false;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b' ' => indent += 1,
            b'\t' => indent += TAB_WIDTH,
            b'>' => depth += 1,
            b'-' | b'*' | b'+' if ends_marker(bytes, at + 1) => {
                depth += 1;
                starts_item = true;
            }
            b'0'..=b'9' => match ordered_marker_end(bytes, at) {
                Some(end) => {
                    depth += 1;
                    starts_item = true;
                    at = end - 1;
                }
                None => break,
            },
            _ => break,
        }
        if depth > limit {
            return (depth, starts_item, "");
        }
        at += 1;
    }
    (depth + indent / LIST_INDENT, starts_item, &line[at..])
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

// The brackets and emphasis opened so far in a paragraph and not yet closed.
#[derive(Default)]
struct Inline {
    brackets: usize,
    emphasis: Vec<u8>,
}

impl Inline {
    fn clear(&mut self) {
        self.brackets = 0;
        self.emphasis.clear();
    }

    // How deep the open inline constructs get on this line, counting those carried in. A run of
    // n delimiters can open up to ceil(n / 2) levels, so it is counted as that many.
    fn scan(&mut self, text: &str, limit: usize) -> usize {
        let bytes = text.as_bytes();
        let mut deepest = self.brackets + self.emphasis.len();
        let mut at = 0;
        while at < bytes.len() {
            match bytes[at] {
                b'\\' => at += 1,
                b'[' => {
                    self.brackets += 1;
                    deepest = deepest.max(self.brackets + self.emphasis.len());
                }
                b']' => self.brackets = self.brackets.saturating_sub(1),
                delimiter @ (b'*' | b'_' | b'~') => {
                    let run = bytes[at..]
                        .iter()
                        .take_while(|byte| **byte == delimiter)
                        .count();
                    let opens = bytes
                        .get(at + run)
                        .is_some_and(|next| !next.is_ascii_whitespace());
                    let closes = at > 0 && !bytes[at - 1].is_ascii_whitespace();
                    let levels = run.div_ceil(2).min(limit + 1);
                    if closes && self.emphasis.last() == Some(&delimiter) {
                        for _ in 0..levels {
                            if self.emphasis.last() == Some(&delimiter) {
                                self.emphasis.pop();
                            }
                        }
                    } else if opens {
                        self.emphasis
                            .resize(self.emphasis.len() + levels, delimiter);
                        deepest = deepest.max(self.brackets + self.emphasis.len());
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

    use markdown::mdast::BlockQuote;
    use proptest::prelude::*;

    use super::*;

    fn too_deep(text: &str) -> bool {
        deeper_than(text, MAX_NESTING)
    }

    fn fits(text: &str) -> bool {
        preview_refusal(text).is_none()
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

    fn opened_per_line(open: &str, close: &str, times: usize) -> String {
        format!(
            "{}{}",
            format!("{open}a\n").repeat(times),
            format!("a{close}\n").repeat(times)
        )
    }

    // The case a per-line count cannot see: one level per line, each line shallow.
    #[test]
    fn emphasis_opened_on_each_line_and_closed_later_is_turned_away_before_parsing() {
        for (open, close) in [
            ("*", "*"),
            ("_", "_"),
            ("**", "**"),
            ("__", "__"),
            ("~~", "~~"),
            ("[", "]"),
        ] {
            let text = opened_per_line(open, close, 2_000);
            assert!(too_deep(&text), "{open}");
            assert!(parse_within(&text, MAX_NESTING).is_none(), "{open}");
        }
    }

    #[test]
    fn what_a_blank_line_a_heading_a_list_item_or_a_fence_ends_does_not_carry_over() {
        let apart = |separator: &str| format!("*a{separator}").repeat(200);
        assert!(!too_deep(&apart("\n\n")));
        assert!(!too_deep(&apart("\n# h\n")));
        assert!(!too_deep(&"- *a\n".repeat(200)));
        assert!(!too_deep(&format!("```\n{}```\n", "*a\n".repeat(200))));
        assert!(!too_deep(&format!("~~~\n{}~~~\n", "[a\n".repeat(200))));
        assert!(too_deep(&"> *a\n".repeat(200)));
    }

    #[test]
    fn balanced_emphasis_across_lines_is_not_counted_as_nesting() {
        assert!(!too_deep(&"*a\na*\n".repeat(500)));
        assert!(!too_deep(&"**a\nb** and [c\nd](e)\n".repeat(500)));
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

    // A tree the pre-pass never sees is still measured, and taking it apart does not recurse.
    #[test]
    fn a_tree_a_hundred_thousand_levels_deep_is_measured_and_dropped_without_recursion() {
        let mut tree = Node::Text(markdown::mdast::Text {
            value: String::new(),
            position: None,
        });
        for _ in 0..100_000 {
            tree = Node::BlockQuote(BlockQuote {
                children: vec![tree],
                position: None,
            });
        }
        assert!(depth_exceeds(&tree, MAX_NESTING));
        dismantle(tree);
    }

    #[test]
    fn a_text_past_the_preview_size_is_refused_without_a_parse() {
        let big = "a\n".repeat(MAX_PREVIEW_BYTES / 2 + 1);
        assert_eq!(preview_refusal(&big), Some(Refusal::TooLarge));
        assert_eq!(
            preview_refusal(&">".repeat(MAX_NESTING + 1)),
            Some(Refusal::TooDeep)
        );
        assert_eq!(preview_refusal("plain"), None);
    }

    // Math spans are one node here and the view must not parse their text again; whatever is
    // inside them is measured as the text it is.
    #[test]
    fn a_math_span_does_not_hide_nesting_in_the_checked_tree() {
        for (open, close) in [("*", "*"), ("~~", "~~")] {
            let text = format!("${}$", opened_per_line(open, close, 2_000));
            assert!(parse_within(&text, MAX_NESTING).is_none(), "{open}");
        }
        assert!(fits("it cost $5 and $10, or $*a*$ in math"));
    }

    // What the view builds when text is appended: the blocks before the last stay as they were
    // parsed, and the last block is parsed again together with the new text.
    fn appended_depth(prefix: &str, delta: &str) -> usize {
        let options = parse_options();
        let Ok(Node::Root(first)) = markdown::to_mdast(prefix, &options) else {
            return 0;
        };
        let (kept, restart) = match first.children.last().and_then(|block| block.position()) {
            Some(position) => (
                &first.children[..first.children.len() - 1],
                position.start.offset,
            ),
            None => (&first.children[..], prefix.len()),
        };
        let mut deepest = kept
            .iter()
            .map(|block| tree_depth(block) + 1)
            .max()
            .unwrap_or(0);
        let again = format!("{}{delta}", &prefix[restart..]);
        if let Ok(tree) = markdown::to_mdast(&again, &options) {
            deepest = deepest.max(tree_depth(&tree));
        }
        deepest
    }

    #[test]
    fn appending_to_accepted_text_stays_within_what_the_whole_text_was_measured_at() {
        let cases = [
            ("- a\n  - b", "\n    - c"),
            ("> a\n> *b", "*\n> > c"),
            ("para *a", " b* c\n\n- d"),
            ("1. a\n", "   1. b\n      1. c"),
        ];
        for (prefix, delta) in cases {
            let whole = format!("{prefix}{delta}");
            let measured = tree_depth(&parse_within(&whole, MAX_NESTING).unwrap());
            assert!(appended_depth(prefix, delta) <= measured);
        }
    }

    const PIECES: &[&str] = &[
        "> ", ">", "- ", "* ", "+ ", "1. ", "1) ", "10. ", "\t", "  ", "    ", "-\t", "*", "**",
        "***", "_", "__", "~", "~~", "$", "$$", "[", "](x)", "](", "a ", "a", "\n", "\n  ", "\n\n",
        "`", "<", "![",
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

        #[test]
        fn appending_never_builds_a_tree_deeper_than_the_whole_text_check_accepted(
            prefix in assembled(),
            delta in assembled(),
            limit in 3usize..20,
        ) {
            let whole = format!("{prefix}{delta}");
            if parse_within(&prefix, limit).is_some() && parse_within(&whole, limit).is_some() {
                prop_assert!(appended_depth(&prefix, &delta) <= limit);
            }
        }
    }

    // The parse runs for every note in the list, so no 64 KiB text may take long to refuse or
    // to accept. The bound is loose for the parser alone and tight for what the pre-pass
    // turns away.
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

    #[test]
    fn deep_inline_nesting_across_lines_is_refused_fast_at_the_size_limit() {
        for (open, close) in [("*", "*"), ("_", "_"), ("~~", "~~"), ("[", "]")] {
            let text = opened_per_line(open, close, 64 * 1024 / (open.len() + 3) / 2);
            let started = Instant::now();
            assert!(parse_within(&text, 32).is_none(), "{open}");
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "{open:?} took {:?}",
                started.elapsed()
            );
        }
    }
}
