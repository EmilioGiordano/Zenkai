use std::collections::HashMap;
use std::ops::Range;

use markdown::mdast::Node;

pub const MAX_INDENT: u8 = 4;
const MAX_URL_BYTES: usize = 4096;
const MAX_DEFINITIONS: usize = 5_000;

// How much of a parsed text is drawn. A note is short, shows its links as plain text and has a
// tight budget; a document keeps its links, and its budget is the parse budget's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Note,
    Document,
}

struct Limits {
    visited: usize,
    blocks: usize,
    text: usize,
    links: bool,
}

impl Mode {
    fn limits(self) -> Limits {
        match self {
            Mode::Note => Limits {
                visited: 20_000,
                blocks: 1_000,
                text: 128 * 1024,
                links: false,
            },
            Mode::Document => Limits {
                visited: 150_000,
                blocks: 30_000,
                text: 512 * 1024,
                links: true,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub len: usize,
    pub style: Style,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Paragraph,
    Heading(u8),
    Code,
}

// Where a link is in the text of a block, and where it goes as the document wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRange {
    pub range: Range<usize>,
    pub url: String,
}

// One line of the flat list a text is drawn from: its text, the styles of its runs and its
// links. `start` is where it begins in the source, for following the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    pub indent: u8,
    pub quoted: bool,
    pub marker: Option<String>,
    pub text: String,
    pub spans: Vec<Span>,
    pub links: Vec<LinkRange>,
    pub start: usize,
}

struct Budget {
    limits: Limits,
    visited: usize,
    blocks: usize,
    text: usize,
}

impl Budget {
    fn visit(&mut self) -> Option<()> {
        self.visited += 1;
        (self.visited <= self.limits.visited).then_some(())
    }
}

#[derive(Default)]
struct Text {
    text: String,
    spans: Vec<Span>,
    links: Vec<LinkRange>,
}

impl Text {
    fn push(&mut self, value: &str, style: Style, budget: &mut Budget) -> Option<()> {
        if value.is_empty() {
            return Some(());
        }
        budget.text += value.len();
        if budget.text > budget.limits.text {
            return None;
        }
        self.text.push_str(value);
        match self.spans.last_mut() {
            Some(last) if last.style == style => last.len += value.len(),
            _ => self.spans.push(Span {
                len: value.len(),
                style,
            }),
        }
        Some(())
    }
}

struct Work<'a> {
    node: &'a Node,
    indent: u8,
    quoted: bool,
    marker: Option<String>,
}

// The text as a flat list of lines, from the tree the checks already measured. The walk is
// iterative and has a budget on nodes, lines and text, so a text that is past it is not drawn
// at all (None). Only headings, paragraphs, list items, quotes, code and, in a document, links
// keep their shape; images, HTML, tables and footnotes are drawn as their plain text.
pub fn blocks_of(tree: &Node, mode: Mode) -> Option<Vec<Block>> {
    let mut budget = Budget {
        limits: mode.limits(),
        visited: 0,
        blocks: 0,
        text: 0,
    };
    let definitions = if budget.limits.links {
        definitions_of(tree)
    } else {
        HashMap::new()
    };
    let links = Links {
        keep: budget.limits.links,
        definitions: &definitions,
    };
    let mut blocks = Vec::new();
    let mut pending = vec![Work {
        node: tree,
        indent: 0,
        quoted: false,
        marker: None,
    }];
    while let Some(work) = pending.pop() {
        budget.visit()?;
        let Work {
            node,
            indent,
            quoted,
            marker,
        } = work;
        let mut text = Text::default();
        let kind = match node {
            Node::BlockQuote(quote) => {
                push_children(&mut pending, &quote.children, indent, true, None);
                continue;
            }
            Node::List(list) => {
                let start = u64::from(list.start.unwrap_or(1));
                let inner = (indent + 1).min(MAX_INDENT);
                for (position, item) in list.children.iter().enumerate().rev() {
                    let mark = if list.ordered {
                        format!("{}.", start + position as u64)
                    } else {
                        "\u{2022}".to_owned()
                    };
                    pending.push(Work {
                        node: item,
                        indent: inner,
                        quoted,
                        marker: Some(mark),
                    });
                }
                continue;
            }
            Node::ListItem(item) => {
                push_children(&mut pending, &item.children, indent, quoted, marker);
                continue;
            }
            Node::Heading(heading) => {
                inline_into(&heading.children, &mut text, &mut budget, &links)?;
                Kind::Heading(heading.depth.clamp(1, 3))
            }
            Node::Paragraph(paragraph) => {
                inline_into(&paragraph.children, &mut text, &mut budget, &links)?;
                Kind::Paragraph
            }
            Node::TableRow(row) => {
                for (position, cell) in row.children.iter().enumerate() {
                    if position > 0 {
                        text.push(" | ", Style::default(), &mut budget)?;
                    }
                    if let Some(children) = cell.children() {
                        inline_into(children, &mut text, &mut budget, &links)?;
                    }
                }
                Kind::Paragraph
            }
            Node::Code(code) => {
                let style = Style {
                    code: true,
                    ..Style::default()
                };
                text.push(&code.value, style, &mut budget)?;
                Kind::Code
            }
            Node::Html(html) => {
                text.push(&html.value, Style::default(), &mut budget)?;
                Kind::Paragraph
            }
            Node::ThematicBreak(_) | Node::Definition(_) => continue,
            other => {
                if let Some(children) = other.children() {
                    push_children(&mut pending, children, indent, quoted, marker);
                }
                continue;
            }
        };
        if text.text.trim().is_empty() {
            continue;
        }
        budget.blocks += 1;
        if budget.blocks > budget.limits.blocks {
            return None;
        }
        blocks.push(Block {
            kind,
            indent,
            quoted,
            marker,
            text: text.text,
            spans: text.spans,
            links: text.links,
            start: node.position().map_or(0, |position| position.start.offset),
        });
    }
    Some(blocks)
}

struct Links<'a> {
    keep: bool,
    definitions: &'a HashMap<String, String>,
}

// What `[text][label]` points at: the first definition of each label, as CommonMark has it.
fn definitions_of(tree: &Node) -> HashMap<String, String> {
    let mut found = HashMap::new();
    let mut pending = vec![tree];
    while let Some(node) = pending.pop() {
        if let Node::Definition(definition) = node
            && found.len() < MAX_DEFINITIONS
        {
            found
                .entry(definition.identifier.clone())
                .or_insert_with(|| definition.url.clone());
        }
        if let Some(children) = node.children() {
            pending.extend(children.iter().rev());
        }
    }
    found
}

// The first child takes the list marker of its item.
fn push_children<'a>(
    pending: &mut Vec<Work<'a>>,
    children: &'a [Node],
    indent: u8,
    quoted: bool,
    mut marker: Option<String>,
) {
    for (position, child) in children.iter().enumerate().rev() {
        pending.push(Work {
            node: child,
            indent,
            quoted,
            marker: if position == 0 { marker.take() } else { None },
        });
    }
}

enum Item<'a> {
    Node(&'a Node, Style),
    EndLink { start: usize, url: String },
}

// The children with a style, last first, for the stack.
fn nested(children: &[Node], style: Style) -> impl Iterator<Item = Item<'_>> {
    children
        .iter()
        .rev()
        .map(move |child| Item::Node(child, style))
}

fn inline_into(
    children: &[Node],
    text: &mut Text,
    budget: &mut Budget,
    links: &Links<'_>,
) -> Option<()> {
    let mut pending: Vec<Item> = nested(children, Style::default()).collect();
    while let Some(item) = pending.pop() {
        let (node, style) = match item {
            Item::Node(node, style) => (node, style),
            Item::EndLink { start, url } => {
                if text.text.len() > start {
                    text.links.push(LinkRange {
                        range: start..text.text.len(),
                        url,
                    });
                }
                continue;
            }
        };
        budget.visit()?;
        match node {
            Node::Text(found) => text.push(&found.value, style, budget)?,
            Node::InlineCode(found) => {
                let code = Style {
                    code: true,
                    ..style
                };
                text.push(&found.value, code, budget)?;
            }
            Node::Strong(found) => {
                let bold = Style {
                    bold: true,
                    ..style
                };
                pending.extend(nested(&found.children, bold));
            }
            Node::Emphasis(found) => {
                let italic = Style {
                    italic: true,
                    ..style
                };
                pending.extend(nested(&found.children, italic));
            }
            Node::Link(found) => open_link(
                &mut pending,
                text,
                links,
                Some(found.url.as_str()),
                &found.children,
                style,
            ),
            Node::LinkReference(found) => open_link(
                &mut pending,
                text,
                links,
                links.definitions.get(&found.identifier).map(String::as_str),
                &found.children,
                style,
            ),
            Node::Break(_) => text.push("\n", style, budget)?,
            Node::Html(found) => text.push(&found.value, style, budget)?,
            Node::Image(found) => text.push(&found.alt, style, budget)?,
            Node::ImageReference(found) => text.push(&found.alt, style, budget)?,
            Node::FootnoteReference(found) => {
                text.push(&format!("[{}]", found.identifier), style, budget)?;
            }
            other => {
                if let Some(children) = other.children() {
                    pending.extend(nested(children, style));
                }
            }
        }
    }
    Some(())
}

// A link keeps its address when the mode keeps links and the address is of a sane size;
// otherwise its children are drawn as the plain text they are.
fn open_link<'a>(
    pending: &mut Vec<Item<'a>>,
    text: &Text,
    links: &Links<'_>,
    url: Option<&str>,
    children: &'a [Node],
    style: Style,
) {
    match url.filter(|url| links.keep && !url.is_empty() && url.len() <= MAX_URL_BYTES) {
        Some(url) => {
            pending.push(Item::EndLink {
                start: text.text.len(),
                url: url.to_owned(),
            });
            pending.extend(nested(
                children,
                Style {
                    link: true,
                    ..style
                },
            ));
        }
        None => pending.extend(nested(children, style)),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::markdown_restricted;

    fn blocks(text: &str) -> Vec<Block> {
        let checked = markdown_restricted::check(text).unwrap();
        blocks_of(&checked.tree, Mode::Note).unwrap()
    }

    fn lines(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .map(|block| match &block.marker {
                Some(marker) => format!("{marker} {}", block.text),
                None => block.text.clone(),
            })
            .collect()
    }

    fn style_of(block: &Block, text: &str) -> Option<Style> {
        let at = block.text.find(text)?;
        let mut end = 0;
        block.spans.iter().find_map(|span| {
            let start = end;
            end += span.len;
            (start <= at && at < end).then_some(span.style)
        })
    }

    #[test]
    fn headings_keep_three_sizes_and_deeper_ones_are_the_third() {
        let found = blocks("# a\n\n## b\n\n### c\n\n#### d\n\n###### e\n");
        let kinds: Vec<Kind> = found.iter().map(|block| block.kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::Heading(1),
                Kind::Heading(2),
                Kind::Heading(3),
                Kind::Heading(3),
                Kind::Heading(3)
            ]
        );
    }

    #[test]
    fn lists_have_markers_and_an_indent_that_stops_growing() {
        let found = blocks("- one\n- two\n\n3. three\n4. four\n");
        assert_eq!(
            lines(&found),
            ["\u{2022} one", "\u{2022} two", "3. three", "4. four"]
        );
        let mut nested = String::new();
        for level in 0..8 {
            nested.push_str(&format!("{}- item {level}\n", "  ".repeat(level)));
        }
        let found = blocks(&nested);
        assert_eq!(found.len(), 8);
        assert_eq!(found[0].indent, 1);
        assert_eq!(found[7].indent, MAX_INDENT);
        assert!(found.iter().all(|block| block.indent <= MAX_INDENT));
    }

    #[test]
    fn an_item_with_several_paragraphs_marks_only_its_first() {
        let found = blocks("- first\n\n  second\n- next\n");
        assert_eq!(lines(&found), ["\u{2022} first", "second", "\u{2022} next"]);
    }

    #[test]
    fn quotes_are_marked_and_nest_with_their_content() {
        let found = blocks("> quoted\n>\n> - item\n\nplain\n");
        assert!(found[0].quoted && found[1].quoted);
        assert_eq!(found[1].marker.as_deref(), Some("\u{2022}"));
        assert!(!found[2].quoted);
    }

    #[test]
    fn bold_italic_and_code_are_runs_of_the_text() {
        let found = blocks("a **b** *c* `d` ***e***\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "a b c d e");
        assert_eq!(style_of(&found[0], "a"), Some(Style::default()));
        assert_eq!(
            style_of(&found[0], "b"),
            Some(Style {
                bold: true,
                ..Style::default()
            })
        );
        assert_eq!(
            style_of(&found[0], "c"),
            Some(Style {
                italic: true,
                ..Style::default()
            })
        );
        assert_eq!(
            style_of(&found[0], "d"),
            Some(Style {
                code: true,
                ..Style::default()
            })
        );
        assert_eq!(
            style_of(&found[0], "e"),
            Some(Style {
                bold: true,
                italic: true,
                ..Style::default()
            })
        );
        let total: usize = found[0].spans.iter().map(|span| span.len).sum();
        assert_eq!(total, found[0].text.len());
    }

    #[test]
    fn code_blocks_keep_their_lines_in_code_style() {
        let found = blocks("```rust\nlet a = 1;\nlet b = 2;\n```\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, Kind::Code);
        assert_eq!(found[0].text, "let a = 1;\nlet b = 2;");
        assert!(found[0].spans.iter().all(|span| span.style.code));
    }

    #[test]
    fn everything_else_is_its_plain_text() {
        let text = "[the link](https://github.com/EmilioGiordano/Zenkai) and ![the logo](x.png) \
                    <b>html</b> $m$ note[^1]\n\n<div>block</div>\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\n[^1]: the note\n";
        let found = blocks(text);
        let all = lines(&found).join("\n");
        assert!(all.contains("the link and the logo <b>html</b> $m$ note[1]"));
        assert!(all.contains("<div>block</div>"));
        assert!(all.contains("a | b"));
        assert!(all.contains("1 | 2"));
        assert!(all.contains("the note"));
        assert!(!all.contains("github.com"));
        assert!(!all.contains("x.png"));
    }

    #[test]
    fn a_hard_break_is_a_line_break() {
        let found = blocks("one  \ntwo\n");
        assert_eq!(found[0].text, "one\ntwo");
    }

    #[test]
    fn a_note_past_the_budget_is_not_drawn() {
        let many_lines = "a\n\n".repeat(Mode::Note.limits().blocks + 1);
        let tree = markdown_restricted::check(&many_lines).unwrap().tree;
        assert!(blocks_of(&tree, Mode::Note).is_none());
        let many_nodes = "`a` ".repeat(Mode::Note.limits().visited);
        let tree = markdown_restricted::check(&many_nodes).unwrap().tree;
        assert!(blocks_of(&tree, Mode::Note).is_none());
        let fine = markdown_restricted::check("fine\n").unwrap();
        assert!(blocks_of(&fine.tree, Mode::Note).is_some());
    }

    #[test]
    fn text_past_the_budget_is_not_drawn() {
        let text = "word ".repeat(Mode::Note.limits().text / 5 + 10);
        let tree = markdown_restricted::check(&text).unwrap().tree;
        assert!(blocks_of(&tree, Mode::Note).is_none());
    }

    #[test]
    fn nothing_is_drawn_for_empty_things() {
        assert!(blocks("").is_empty());
        assert!(blocks("---\n\n[a]: https://x.y\n").is_empty());
    }

    const PIECES: &[&str] = &[
        "# ",
        "## ",
        "- ",
        "1. ",
        "> ",
        "  ",
        "\n",
        "\n\n",
        "*a*",
        "**b**",
        "`c`",
        "```\n",
        "[l](u)",
        "![i](u)",
        "<b>",
        "$m$",
        "|a|b|\n|-|-|\n|1|2|\n",
        "word ",
        "\t",
        "[^1]",
        "---\n",
        "~~s~~",
    ];

    proptest! {
        #![proptest_config(ProptestConfig { cases: 1500, ..ProptestConfig::default() })]

        #[test]
        fn whatever_the_text_the_lines_are_bounded_and_whole(
            parts in prop::collection::vec(prop::sample::select(PIECES.to_vec()), 0..60)
        ) {
            if let Some(checked) = markdown_restricted::check(&parts.concat())
                && let Some(found) = blocks_of(&checked.tree, Mode::Note)
            {
                prop_assert!(found.len() <= Mode::Note.limits().blocks);
                for block in &found {
                    prop_assert!(block.indent <= MAX_INDENT);
                    prop_assert!(!block.text.trim().is_empty());
                    let total: usize = block.spans.iter().map(|span| span.len).sum();
                    prop_assert_eq!(total, block.text.len());
                    if let Kind::Heading(level) = block.kind {
                        prop_assert!((1..=3).contains(&level));
                    }
                }
            }
        }
    }

    fn document(text: &str) -> Vec<Block> {
        let tree =
            crate::markdown_depth::parse_within(text, crate::markdown_depth::MAX_NESTING).unwrap();
        blocks_of(&tree, Mode::Document).unwrap()
    }

    #[test]
    fn a_note_draws_links_as_their_text_and_a_document_keeps_them() {
        let text = "see [the docs](https://x.y/a?b=1) and [ref][r] and <https://z.w>\n\n[r]: https://r.s/t\n";
        let note = blocks(text);
        assert_eq!(note[0].text, "see the docs and ref and https://z.w");
        assert!(note[0].links.is_empty());
        let found = document(text);
        assert_eq!(found.len(), 1);
        let urls: Vec<(&str, &str)> = found[0]
            .links
            .iter()
            .map(|link| (&found[0].text[link.range.clone()], link.url.as_str()))
            .collect();
        assert_eq!(
            urls,
            [
                ("the docs", "https://x.y/a?b=1"),
                ("ref", "https://r.s/t"),
                ("https://z.w", "https://z.w"),
            ]
        );
        assert!(found[0].spans.iter().any(|span| span.style.link));
    }

    #[test]
    fn a_link_inside_emphasis_and_a_link_without_text_or_target() {
        let found = document("*a [b](u) c* [](v) [e]()\n");
        assert_eq!(found[0].text, "a b c  e");
        assert_eq!(found[0].links.len(), 1);
        assert_eq!(&found[0].text[found[0].links[0].range.clone()], "b");
        assert_eq!(found[0].links[0].url, "u");
    }

    #[test]
    fn a_very_long_address_is_not_a_link() {
        let long = format!("[a](https://x.y/{})", "a".repeat(MAX_URL_BYTES));
        assert!(document(&long)[0].links.is_empty());
    }

    #[test]
    fn a_block_remembers_where_it_starts_in_the_source() {
        let found = document("# one\n\ntwo\n\n- three\n");
        let starts: Vec<usize> = found.iter().map(|block| block.start).collect();
        assert_eq!(starts, [0, 7, 14]);
    }

    #[test]
    fn a_document_has_a_wider_budget_than_a_note() {
        let many_lines = "a\n\n".repeat(Mode::Note.limits().blocks + 1);
        assert_eq!(document(&many_lines).len(), Mode::Note.limits().blocks + 1);
        let too_many = "a\n\n".repeat(Mode::Document.limits().blocks + 1);
        let tree =
            crate::markdown_depth::parse_within(&too_many, crate::markdown_depth::MAX_NESTING);
        if let Ok(tree) = tree {
            assert!(blocks_of(&tree, Mode::Document).is_none());
        }
    }
}
