use markdown::ParseOptions;
use markdown::mdast::Node;

// A tree deeper than this is not rendered: the layout recurses once per level, so a file
// built from thousands of nested quotes, lists or emphasis could exhaust the stack.
pub const MAX_NESTING: usize = 64;

// What the text view parses on the UI thread; anything longer is checked in the background.
pub const SYNC_BYTES: usize = 4 * 1024;

// The parser's cost grows faster than the text, so a preview is not offered past this size;
// a longer file is still edited in Source.
pub const MAX_PREVIEW_BYTES: usize = 64 * 1024;

// Budgets on the shape of the whole document, for what the text view does per node, per inline
// object that it pairs or measures among siblings (raw HTML tags, which it pairs and recurses
// into, and math spans), per marked inline child of one parent (it combines the marks of a
// paragraph pairwise), and per cell of a table (it pads every row to the header).
const MAX_NODES: usize = 30_000;
const MAX_INLINE_OBJECTS: usize = 2_000;
const MAX_MARKED_CHILDREN: usize = 2_000;
const MAX_TABLE_CELLS: usize = 5_000;

// Closers of emphasis that have no opener: the parser looks back over them again and again.
const MAX_LONE_CLOSERS: usize = 3_000;

// Totals over the whole text, not per paragraph, because what ends a paragraph is for the
// parser to say: delimiter runs (`*`, `_`, `~`), which it resolves against each other, and
// openers of links and images (`[`), which it resolves against each closer.
const MAX_DELIMITER_RUNS: usize = 5_000;
const MAX_BRACKET_OPENERS: usize = 2_000;

const TAB_WIDTH: usize = 4;
const LIST_INDENT: usize = 2;

// The inline HTML tags the text view pairs, each matched only by its own name.
const PAIRED_TAGS: [&str; 8] = ["strong", "b", "em", "i", "u", "s", "del", "strike"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    TooDeep,
    TooLarge,
}

// The parsed text when its tree is within `limit` levels and the budgets, so nothing beyond
// them reaches the renderer. The tree is what the text view will build, and it is measured
// without recursion.
pub fn parse_within(text: &str, limit: usize) -> Result<Node, Refusal> {
    if deeper_than(text, limit) {
        return Err(Refusal::TooDeep);
    }
    let options = parse_options();
    // The parser unwraps in places; a panic on a hostile text must not end a background task
    // unseen, so it counts as a text that cannot be shown.
    let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        markdown::to_mdast(text, &options)
    }));
    let tree = match parsed {
        Ok(Ok(tree)) => tree,
        Ok(Err(error)) => {
            tracing::debug!(%error, "text that could not be parsed is not shown");
            return Err(Refusal::TooLarge);
        }
        Err(_) => {
            tracing::error!("the Markdown parser panicked on a text; it is not shown");
            return Err(Refusal::TooLarge);
        }
    };
    if let Some(refusal) = over_budget(&tree, limit) {
        dismantle(tree);
        return Err(refusal);
    }
    Ok(tree)
}

pub fn preview_refusal(text: &str) -> Option<Refusal> {
    if text.len() > MAX_PREVIEW_BYTES {
        return Some(Refusal::TooLarge);
    }
    parse_within(text, MAX_NESTING).err()
}

// The options the text view parses with first, so the checked tree is the rendered one. The
// view's plugins claim math spans, which keeps it from parsing their text again as prose.
fn parse_options() -> ParseOptions {
    let mut options = ParseOptions::gfm();
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    options
}

// Depth counts the inline HTML tags still open around a node as levels: the text view pairs
// the tags of a parent's children and recurses once per open pair, inside every level above.
fn over_budget(tree: &Node, limit: usize) -> Option<Refusal> {
    let mut nodes = 0usize;
    let mut objects = 0usize;
    let mut cells = 0usize;
    let mut pending = vec![(tree, 0usize)];
    while let Some((node, depth)) = pending.pop() {
        if depth > limit {
            return Some(Refusal::TooDeep);
        }
        nodes += 1;
        if nodes > MAX_NODES {
            return Some(Refusal::TooLarge);
        }
        if let Node::Table(table) = node {
            let header = table.children.first().and_then(Node::children);
            let columns = table.align.len().max(header.map_or(0, Vec::len));
            cells = cells.saturating_add(columns.saturating_mul(table.children.len()));
            if cells > MAX_TABLE_CELLS {
                return Some(Refusal::TooLarge);
            }
        }
        if let Some(children) = node.children() {
            let mut open = TagBalance::default();
            let mut marked = 0usize;
            for child in children {
                if matches!(
                    child,
                    Node::Emphasis(_)
                        | Node::Strong(_)
                        | Node::Delete(_)
                        | Node::Link(_)
                        | Node::LinkReference(_)
                        | Node::Image(_)
                        | Node::ImageReference(_)
                        | Node::InlineCode(_)
                        | Node::InlineMath(_)
                        | Node::FootnoteReference(_)
                        | Node::Html(_)
                ) {
                    marked += 1;
                    if marked > MAX_MARKED_CHILDREN {
                        return Some(Refusal::TooLarge);
                    }
                }
                if matches!(child, Node::Html(_) | Node::InlineMath(_)) {
                    objects += 1;
                    if objects > MAX_INLINE_OBJECTS {
                        return Some(Refusal::TooLarge);
                    }
                }
                if let Node::Html(tag) = child {
                    open.apply(&tag.value);
                }
                pending.push((child, depth + 1 + open.total()));
            }
        }
    }
    None
}

// How many tags of each paired name are open among the siblings so far. A closing tag takes
// one off its own name only, as the text view matches it, so a closer of another name cannot
// hide the nesting of the openers.
#[derive(Default)]
struct TagBalance {
    open: [usize; PAIRED_TAGS.len()],
}

impl TagBalance {
    fn total(&self) -> usize {
        self.open.iter().sum()
    }

    fn apply(&mut self, html: &str) {
        let Some(inner) = html
            .trim()
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'))
        else {
            return;
        };
        if inner.ends_with('/') {
            return;
        }
        let (closing, rest) = match inner.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, inner),
        };
        let name = rest
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        let Some(slot) = PAIRED_TAGS.iter().position(|paired| *paired == name) else {
            return;
        };
        if closing {
            self.open[slot] = self.open[slot].saturating_sub(1);
        } else {
            self.open[slot] += 1;
        }
    }
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
    let mut fence = None;
    // CommonMark ends a line at a line feed, a carriage return or both; the empty piece between
    // a carriage return and its line feed is harmless.
    text.split(['\n', '\r'])
        .any(|line| line_exceeds(line, limit, &mut paragraph, &mut fence))
}

fn line_exceeds(
    line: &str,
    limit: usize,
    paragraph: &mut Inline,
    fence: &mut Option<Fence>,
) -> bool {
    if let Some(open) = *fence {
        if open.is_closed_by(line) {
            *fence = None;
            paragraph.clear();
        }
        return false;
    }
    if let Some(open) = Fence::opened_by(line) {
        *fence = Some(open);
        paragraph.clear();
        return false;
    }
    if line.trim().is_empty() {
        paragraph.clear();
        return false;
    }
    let (blocks, starts_item, rest) = block_depth(line, limit);
    if blocks > limit {
        return true;
    }
    // Only what truly ends a paragraph clears it: an item that has content (an empty one, or a
    // number other than 1, cannot interrupt a paragraph) and a real heading.
    if (starts_item && !rest.trim().is_empty()) || is_atx_heading(rest) {
        paragraph.clear();
    }
    blocks + paragraph.scan(rest, limit) > limit
        || paragraph.lone_closers > MAX_LONE_CLOSERS
        || paragraph.runs > MAX_DELIMITER_RUNS
        || paragraph.openers > MAX_BRACKET_OPENERS
}

// A code fence as CommonMark has it: up to three spaces of indent, then three or more
// backticks or tildes; the info string of a backtick fence holds no backtick.
#[derive(Clone, Copy)]
struct Fence {
    mark: u8,
    length: usize,
}

impl Fence {
    fn opened_by(line: &str) -> Option<Fence> {
        let (mark, length, after) = Fence::run(line)?;
        let info_ok = mark == b'~' || !after.contains('`');
        info_ok.then_some(Fence { mark, length })
    }

    fn is_closed_by(self, line: &str) -> bool {
        Fence::run(line).is_some_and(|(mark, length, after)| {
            mark == self.mark && length >= self.length && after.trim().is_empty()
        })
    }

    // The mark, how many of it, and what follows, for a line that starts with a fence run.
    fn run(line: &str) -> Option<(u8, usize, &str)> {
        let indent = line.bytes().take_while(|byte| *byte == b' ').count();
        if indent > 3 {
            return None;
        }
        let rest = &line[indent..];
        let mark = *rest.as_bytes().first()?;
        if mark != b'`' && mark != b'~' {
            return None;
        }
        let length = rest.bytes().take_while(|byte| *byte == mark).count();
        (length >= 3).then(|| (mark, length, &rest[length..]))
    }
}

// One to six `#` and then a space, a tab or the end of the line.
fn is_atx_heading(text: &str) -> bool {
    let hashes = text.bytes().take_while(|byte| *byte == b'#').count();
    (1..=6).contains(&hashes) && ends_marker(text.as_bytes(), hashes)
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
                    starts_item |= &bytes[at..end - 1] == b"1";
                    at = end - 1;
                }
                None => break,
            },
            b'[' => match footnote_marker_end(bytes, at) {
                Some(end) => {
                    depth += 1;
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

// A footnote definition, `[^label]:`, holds blocks like a list item does; the index after it.
fn footnote_marker_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start + 1) != Some(&b'^') {
        return None;
    }
    let label = bytes[start + 2..]
        .iter()
        .take_while(|byte| !byte.is_ascii_whitespace() && **byte != b']')
        .count();
    let close = start + 2 + label;
    let marker =
        label > 0 && bytes.get(close) == Some(&b']') && bytes.get(close + 1) == Some(&b':');
    marker.then_some(close + 2)
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

// The brackets and emphasis opened so far in a paragraph and not yet closed, and the totals
// over the whole text, which a paragraph's end does not reset.
#[derive(Default)]
struct Inline {
    brackets: usize,
    emphasis: Vec<u8>,
    runs: usize,
    openers: usize,
    lone_closers: usize,
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
                    self.openers += 1;
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
                    self.runs += 1;
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
                    } else if closes {
                        self.lone_closers += 1;
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
        assert!(parse_within(&text, MAX_NESTING).is_err());
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
            assert!(!too_deep(&marker.repeat(10)), "{marker:?}");
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
        assert!(!too_deep(&"**bold** and *it* and [x](y)\n\n".repeat(1_000)));
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
            assert!(parse_within(&text, MAX_NESTING).is_err(), "{open}");
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
            assert!(parse_within(&text, MAX_NESTING).is_err());
        }
    }

    #[test]
    fn a_document_within_the_limit_gives_its_tree() {
        let tree = parse_within("- a\n  - b *c*\n", MAX_NESTING).unwrap();
        assert!(tree_depth(&tree) <= MAX_NESTING);
        assert!(parse_within("- a\n  - b *c*\n", 3).is_err());
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
        assert_eq!(over_budget(&tree, MAX_NESTING), Some(Refusal::TooDeep));
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

    #[test]
    fn only_a_real_heading_ends_what_a_paragraph_carries() {
        for heading in ["# h", "###### h", "#", "##\t", "  ## h"] {
            let text = format!("*a\n{heading}\n").repeat(200);
            assert!(!too_deep(&text), "{heading:?}");
        }
        for not_heading in ["#tag", "####### h", "#5 items", "\\# h"] {
            let text = format!("*a\n{not_heading}\n").repeat(200);
            assert!(too_deep(&text), "{not_heading:?}");
        }
    }

    #[test]
    fn only_a_real_fence_hides_its_lines() {
        let inside = "*a\n".repeat(200);
        for (open, close) in [
            ("```", "```"),
            ("~~~", "~~~"),
            ("   ````", "`````"),
            ("```rust", "```"),
        ] {
            let text = format!("{open}\n{inside}{close}\n");
            assert!(!too_deep(&text), "{open:?}");
        }
        for not_fence in ["    ```", "``` a`b", "``", "~~", "x ```"] {
            let text = format!("{not_fence}\n{inside}");
            assert!(too_deep(&text), "{not_fence:?}");
        }
        let unclosed_by_other_mark = format!("```\n{inside}~~~\n{inside}");
        assert!(!too_deep(&unclosed_by_other_mark));
        let closed = format!("```\n```\n{inside}");
        assert!(too_deep(&closed));
    }

    // Each case below is within the byte cap of the pane, and only its shape is hostile.
    #[test]
    fn a_flood_of_html_tags_among_siblings_is_refused_at_the_byte_cap() {
        let pairs = (MAX_PREVIEW_BYTES - 2) / 7;
        let text = format!("x{}y{}", "<b>".repeat(pairs), "</b>".repeat(pairs));
        assert!(text.len() <= MAX_PREVIEW_BYTES);
        assert_eq!(preview_refusal(&text), Some(Refusal::TooLarge));
        assert_eq!(preview_refusal("x <b>y</b> z"), None);
    }

    // Tags split over many parents add up: the budget is for the document, not per parent.
    #[test]
    fn html_tags_spread_over_nested_emphasis_are_refused_at_the_byte_cap() {
        let level = format!("*a{}", "<b>".repeat(499));
        let close = format!("{}a*", "</b>".repeat(499));
        let levels = MAX_PREVIEW_BYTES / (level.len() + close.len() + 2) - 1;
        let text = format!(
            "{}{}",
            format!("{level}\n").repeat(levels),
            format!("{close}\n").repeat(levels)
        );
        assert!(text.len() <= MAX_PREVIEW_BYTES);
        assert!(preview_refusal(&text).is_some());
    }

    #[test]
    fn unclosed_tags_count_as_levels_and_closed_ones_do_not() {
        assert_eq!(
            preview_refusal(&format!("{}x", "<b>".repeat(MAX_NESTING + 1))),
            Some(Refusal::TooDeep)
        );
        assert_eq!(
            preview_refusal(&format!("{}x{}", "<b>".repeat(30), "</b>".repeat(30))),
            None
        );
        assert_eq!(
            preview_refusal(&"<br>a<hr/><!-- c --><img src=x>\n".repeat(20)),
            None
        );
    }

    // A closer of another name does not close a tag, so it cannot hide that the openers nest.
    #[test]
    fn a_closer_of_another_name_does_not_hide_the_nesting_of_the_openers() {
        let foreign = format!("{}x{}", "<b>".repeat(MAX_NESTING + 1), "</i>".repeat(200));
        assert_eq!(preview_refusal(&foreign), Some(Refusal::TooDeep));
        let mixed = format!("{}x", "<b><i>".repeat(MAX_NESTING / 2 + 1));
        assert_eq!(preview_refusal(&mixed), Some(Refusal::TooDeep));
        let unknown = format!("{}x", "<span>".repeat(MAX_NESTING + 1));
        assert_eq!(preview_refusal(&unknown), None);
        let balanced = format!("{}x{}", "<b><i>".repeat(20), "</i></b>".repeat(20));
        assert_eq!(preview_refusal(&balanced), None);
        let strays = "</b></i></u>".repeat(500);
        assert_eq!(preview_refusal(&strays), None);
    }

    // The text view combines the marks of a paragraph pairwise, so there is a cap per parent.
    #[test]
    fn thousands_of_marked_children_of_one_parent_are_refused_at_the_byte_cap() {
        let code = "`a` ".repeat(14_000);
        assert!(code.len() <= MAX_PREVIEW_BYTES);
        assert_eq!(preview_refusal(&code), Some(Refusal::TooLarge));
        let links = "[a](b) ".repeat(MAX_PREVIEW_BYTES / 7);
        assert!(preview_refusal(&links).is_some());
        let images = "![a](b) ".repeat(MAX_PREVIEW_BYTES / 8);
        assert!(preview_refusal(&images).is_some());
        let footnotes = "x[^1] ".repeat(MAX_PREVIEW_BYTES / 6);
        assert!(preview_refusal(&footnotes).is_some());
        assert_eq!(preview_refusal(&"`a` ".repeat(500)), None);
        let two = format!("{}\n\n", "`a` ".repeat(1_500)).repeat(2);
        assert_eq!(preview_refusal(&two), None);
    }

    // Math spans and raw tags are measured per document: the view lays each one out apart.
    #[test]
    fn thousands_of_math_spans_and_tags_are_refused_at_the_byte_cap() {
        let math = "$a$ ".repeat(14_000);
        assert!(math.len() <= MAX_PREVIEW_BYTES);
        assert_eq!(preview_refusal(&math), Some(Refusal::TooLarge));
        assert_eq!(preview_refusal(&"$a$ ".repeat(500)), None);
        let apart = format!("{}\n\n", "$a$ ".repeat(900)).repeat(3);
        assert_eq!(preview_refusal(&apart), Some(Refusal::TooLarge));
    }

    // What ends a paragraph is for the parser to say, so the totals are for the whole text and
    // a line that only looks like a new item does not reset them.
    #[test]
    fn totals_are_for_the_text_and_false_item_lines_do_not_reset_a_paragraph() {
        let images = "![*a\n2.\n".repeat(MAX_PREVIEW_BYTES / 8);
        assert!(images.len() <= MAX_PREVIEW_BYTES);
        assert!(deeper_than(&images, MAX_NESTING));
        assert_eq!(preview_refusal(&images), Some(Refusal::TooDeep));
        let tildes = "x~~~y ".repeat(10_000);
        assert_eq!(preview_refusal(&tildes), Some(Refusal::TooDeep));
        let separated = "x~~~y \n2.\n".repeat(5_000);
        assert_eq!(preview_refusal(&separated), Some(Refusal::TooDeep));
        let group = format!("{}\n2.\n", "[".repeat(63));
        let brackets = format!("{}{}", group.repeat(490), "]".repeat(31_000));
        assert!(brackets.len() <= MAX_PREVIEW_BYTES);
        assert_eq!(preview_refusal(&brackets), Some(Refusal::TooDeep));
        let started = Instant::now();
        let once = format!("{}\n2.\n{}", "[".repeat(63), "]".repeat(31_000));
        let _ = preview_refusal(&once);
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn only_an_item_that_can_interrupt_a_paragraph_ends_it() {
        assert!(too_deep(&"*a\n2. b\n".repeat(200)));
        assert!(too_deep(&"*a\n-\n".repeat(200)));
        assert!(too_deep(&"*a\n1.\n".repeat(200)));
        assert!(!too_deep(&"*a\n1. b\n".repeat(200)));
        assert!(!too_deep(&"*a\n- b\n".repeat(200)));
    }

    #[test]
    fn a_wide_header_over_many_rows_is_refused_at_the_byte_cap() {
        let header = format!("{}|\n{}|\n", "|a".repeat(4_000), "|-".repeat(4_000));
        let text = format!("{header}{}", "|x|\n".repeat(11_000));
        assert!(text.len() <= MAX_PREVIEW_BYTES);
        assert_eq!(preview_refusal(&text), Some(Refusal::TooLarge));
        assert_eq!(preview_refusal("|a|b|\n|-|-|\n|1|2|\n"), None);
    }

    // The cell budget is one sum over the document, not one per table.
    #[test]
    fn many_tables_that_are_each_within_the_cell_budget_are_refused_together() {
        let table = format!(
            "{}|\n{}|\n{}\n",
            "|a".repeat(40),
            "|-".repeat(40),
            "|x|\n".repeat(100)
        );
        assert_eq!(preview_refusal(&table), None);
        let text = table.repeat(2);
        assert!(text.len() <= MAX_PREVIEW_BYTES);
        assert_eq!(preview_refusal(&text), Some(Refusal::TooLarge));
    }

    #[test]
    fn more_nodes_than_the_budget_are_refused_at_the_byte_cap() {
        let text = "a\n\n".repeat(MAX_PREVIEW_BYTES / 3);
        assert!(text.len() <= MAX_PREVIEW_BYTES);
        assert_eq!(preview_refusal(&text), Some(Refusal::TooLarge));
        assert_eq!(preview_refusal(&"a\n\n".repeat(1000)), None);
    }

    // The case that is quadratic inside the parser: attention runs that resolve against each
    // other, in one paragraph. It is turned away before the parse, and fast.
    #[test]
    fn a_paragraph_of_emphasis_runs_is_turned_away_before_parsing_at_the_byte_cap() {
        let text = "*a* ".repeat(MAX_PREVIEW_BYTES / 4);
        assert_eq!(text.len(), MAX_PREVIEW_BYTES);
        let started = Instant::now();
        assert!(deeper_than(&text, MAX_NESTING));
        assert_eq!(preview_refusal(&text), Some(Refusal::TooDeep));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(preview_refusal(&"*a* ".repeat(500)), None);
        assert_eq!(
            preview_refusal(&"*a* and _b_ and ~~c~~\n\n".repeat(700)),
            None
        );
        let tildes = "~a~ ".repeat(MAX_PREVIEW_BYTES / 4);
        assert_eq!(preview_refusal(&tildes), Some(Refusal::TooDeep));
    }

    #[test]
    fn closers_without_openers_are_turned_away_before_parsing_at_the_byte_cap() {
        let text = "a* ".repeat(MAX_PREVIEW_BYTES / 3);
        assert!(text.len() <= MAX_PREVIEW_BYTES);
        assert!(deeper_than(&text, MAX_NESTING));
        assert_eq!(preview_refusal(&text), Some(Refusal::TooDeep));
        let underscores = "a_ ".repeat(MAX_PREVIEW_BYTES / 3);
        assert_eq!(preview_refusal(&underscores), Some(Refusal::TooDeep));
        assert_eq!(preview_refusal(&"snake_case_word ".repeat(1_500)), None);
        assert_eq!(preview_refusal(&"*a* and **b** and _c_ ".repeat(500)), None);
    }

    #[test]
    fn nested_footnote_definitions_are_turned_away_before_parsing_at_the_byte_cap() {
        let text = "[^a]: ".repeat(MAX_PREVIEW_BYTES / 6);
        assert!(text.len() <= MAX_PREVIEW_BYTES);
        assert_eq!(preview_refusal(&text), Some(Refusal::TooDeep));
        assert_eq!(preview_refusal("text[^1]\n\n[^1]: the note\n"), None);
        assert!(too_deep(&"[^a]: ".repeat(MAX_NESTING + 1)));
        assert!(!too_deep(&"[^a]: ".repeat(10)));
    }

    // Math spans are one node here and the view must not parse their text again; whatever is
    // inside them is measured as the text it is.
    #[test]
    fn a_math_span_does_not_hide_nesting_in_the_checked_tree() {
        for (open, close) in [("*", "*"), ("~~", "~~")] {
            let text = format!("${}$", opened_per_line(open, close, 2_000));
            assert!(parse_within(&text, MAX_NESTING).is_err(), "{open}");
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
            if let Ok(tree) = parse_within(&text, limit) {
                prop_assert!(tree_depth(&tree) <= limit);
            }
        }

        #[test]
        fn text_that_nests_by_repetition_is_refused_or_within_the_limit(
            piece in prop::sample::select(PIECES.to_vec()),
            times in 1usize..300,
        ) {
            let text = piece.repeat(times);
            if let Ok(tree) = parse_within(&text, MAX_NESTING) {
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
            if parse_within(&prefix, limit).is_ok() && parse_within(&whole, limit).is_ok() {
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
            assert!(parse_within(&text, 32).is_err(), "{open}");
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "{open:?} took {:?}",
                started.elapsed()
            );
        }
    }
}
