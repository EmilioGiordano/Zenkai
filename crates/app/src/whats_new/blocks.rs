use markdown::mdast::Node;

pub const MAX_INDENT: u8 = 4;
const MAX_VISITED: usize = 20_000;
const MAX_BLOCKS: usize = 1_000;
const MAX_TEXT_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
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

// One line of the flat list a note is drawn from: its text, and the styles of its runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    pub indent: u8,
    pub quoted: bool,
    pub marker: Option<String>,
    pub text: String,
    pub spans: Vec<Span>,
}

#[derive(Default)]
struct Budget {
    visited: usize,
    blocks: usize,
    text: usize,
}

impl Budget {
    fn visit(&mut self) -> Option<()> {
        self.visited += 1;
        (self.visited <= MAX_VISITED).then_some(())
    }
}

#[derive(Default)]
struct Text {
    text: String,
    spans: Vec<Span>,
}

impl Text {
    fn push(&mut self, value: &str, style: Style, budget: &mut Budget) -> Option<()> {
        if value.is_empty() {
            return Some(());
        }
        budget.text += value.len();
        if budget.text > MAX_TEXT_BYTES {
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

// The note as a flat list of lines, from the tree the restricted checks already measured. The
// walk is iterative and has a budget on nodes, lines and text, so a note that is past it is
// not drawn at all (None). Only headings, paragraphs, list items, quotes and code keep their
// shape; links, images, HTML, math, tables and footnotes are drawn as their plain text.
pub fn blocks_of(tree: &Node) -> Option<Vec<Block>> {
    let mut budget = Budget::default();
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
            Node::Blockquote(quote) => {
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
                inline_into(&heading.children, &mut text, &mut budget)?;
                Kind::Heading(heading.depth.clamp(1, 3))
            }
            Node::Paragraph(paragraph) => {
                inline_into(&paragraph.children, &mut text, &mut budget)?;
                Kind::Paragraph
            }
            Node::TableRow(row) => {
                for (position, cell) in row.children.iter().enumerate() {
                    if position > 0 {
                        text.push(" | ", Style::default(), &mut budget)?;
                    }
                    if let Some(children) = cell.children() {
                        inline_into(children, &mut text, &mut budget)?;
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
            Node::Math(math) => {
                text.push(&math.value, Style::default(), &mut budget)?;
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
        if budget.blocks > MAX_BLOCKS {
            return None;
        }
        blocks.push(Block {
            kind,
            indent,
            quoted,
            marker,
            text: text.text,
            spans: text.spans,
        });
    }
    Some(blocks)
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

// The children with a style, last first, for the stack.
fn nested(children: &[Node], style: Style) -> impl Iterator<Item = (&Node, Style)> {
    children.iter().rev().map(move |child| (child, style))
}

fn inline_into(children: &[Node], text: &mut Text, budget: &mut Budget) -> Option<()> {
    let mut pending: Vec<(&Node, Style)> = children
        .iter()
        .rev()
        .map(|child| (child, Style::default()))
        .collect();
    while let Some((node, style)) = pending.pop() {
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
            Node::Break(_) => text.push("\n", style, budget)?,
            Node::Html(found) => text.push(&found.value, style, budget)?,
            Node::InlineMath(found) => text.push(&found.value, style, budget)?,
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::markdown_restricted;

    fn blocks(text: &str) -> Vec<Block> {
        let checked = markdown_restricted::check(text).unwrap();
        blocks_of(&checked.tree).unwrap()
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
                code: false
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
        assert!(all.contains("the link and the logo <b>html</b> m note[1]"));
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
        let many_lines = "a\n\n".repeat(MAX_BLOCKS + 1);
        let tree = markdown_restricted::check(&many_lines).unwrap().tree;
        assert!(blocks_of(&tree).is_none());
        // Within the parse budgets (30,000 nodes, 2,000 inline objects a parent), past the walk's.
        let paragraph = format!("{}\n\n", "`a` ".repeat(950));
        let many_nodes = paragraph.repeat(MAX_VISITED / 1_900 + 1);
        let tree = markdown_restricted::check(&many_nodes).unwrap().tree;
        assert!(blocks_of(&tree).is_none());
        assert!(blocks_of(&markdown_restricted::check("fine\n").unwrap().tree).is_some());
    }

    #[test]
    fn text_past_the_budget_is_not_drawn() {
        let text = "word ".repeat(MAX_TEXT_BYTES / 5 + 10);
        let tree = markdown_restricted::check(&text).unwrap().tree;
        assert!(blocks_of(&tree).is_none());
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
                && let Some(found) = blocks_of(&checked.tree)
            {
                prop_assert!(found.len() <= MAX_BLOCKS);
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
}
