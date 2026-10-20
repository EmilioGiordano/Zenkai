use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope, RopeExt,
};
use gpui_kit::*;

pub const LANGUAGE: &str = "markdown";

// Only this much of a line is highlighted, so a line of megabytes costs no more than a short one.
const MAX_HIGHLIGHTED_BYTES: usize = 4096;

fn head_of(characters: impl Iterator<Item = char>) -> String {
    let mut head = String::new();
    for character in characters {
        if head.len() >= MAX_HIGHLIGHTED_BYTES {
            break;
        }
        head.push(character);
    }
    head
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Heading,
    HeadingMarker,
    ListMarker,
    QuoteMarker,
    Rule,
    Fence,
    Code,
    CodeSpan,
    Strong,
    Emphasis,
    LinkText,
    LinkUri,
}

mod blocks;
mod inline;

use blocks::{Fence, next_fence, scan_line};

type Spans = Vec<(Range<usize>, Token)>;

pub struct MarkdownHighlighter {
    text: Rope,
    // The fence open as each line starts; always holds at least line 0, and is cut back to
    // the edited line on a change, then extended only as far as the lines asked for.
    fences: RefCell<Vec<Option<Fence>>>,
}

impl MarkdownHighlighter {
    pub fn factory() -> InputHighlighterFactory {
        Rc::new(|language| {
            (language == LANGUAGE)
                .then(|| Box::new(MarkdownHighlighter::new()) as Box<dyn InputHighlighter>)
        })
    }

    fn new() -> MarkdownHighlighter {
        MarkdownHighlighter {
            text: Rope::new(),
            fences: RefCell::new(vec![None]),
        }
    }

    fn fence_at(&self, row: usize) -> Option<Fence> {
        let mut fences = self.fences.borrow_mut();
        while fences.len() <= row {
            let previous = fences.len() - 1;
            let mut state = fences[previous];
            let slice = self.text.slice_line(previous);
            if slice
                .chars()
                .find(|c| *c != ' ')
                .is_some_and(|c| matches!(c, '`' | '~'))
            {
                state = next_fence(&head_of(slice.chars()), state);
            }
            fences.push(state);
        }
        fences[row]
    }
}

fn style_of(token: Token, resolver: &dyn HighlightStyleResolver) -> HighlightStyle {
    let named = |name: &str| resolver.style(name).unwrap_or_default();
    match token {
        Token::Heading => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..named("title")
        },
        Token::HeadingMarker | Token::QuoteMarker => named("punctuation.special"),
        Token::ListMarker | Token::Rule => named("punctuation.list_marker"),
        Token::Fence => named("punctuation.delimiter"),
        Token::Code => named("text.literal"),
        Token::CodeSpan => named("text.code.span"),
        Token::Strong => HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..named("emphasis.strong")
        },
        Token::Emphasis => HighlightStyle {
            font_style: Some(FontStyle::Italic),
            ..named("emphasis")
        },
        Token::LinkText => named("link_text"),
        Token::LinkUri => named("link_uri"),
    }
}

impl InputHighlighter for MarkdownHighlighter {
    fn language(&self) -> SharedString {
        LANGUAGE.into()
    }

    fn update(
        &mut self,
        edit: Option<InputEdit>,
        text: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        self.text = text.clone();
        let keep = edit.map_or(1, |edit| edit.start_position.row + 1);
        self.fences.get_mut().truncate(keep.max(1));
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let end = range.end.min(self.text.len());
        let start = range.start.min(end);
        let first = self.text.offset_to_point(start).row;
        let last = self.text.offset_to_point(end).row;
        let mut runs = Vec::new();
        let mut position = start;
        for row in first..=last {
            let line_start = self.text.line_start_offset(row);
            let line = head_of(self.text.slice_line(row).chars());
            let open = self.fence_at(row);
            for (span, token) in scan_line(&line, open) {
                let from = (line_start + span.start).max(position).min(end);
                let to = (line_start + span.end).min(end);
                if from >= to {
                    continue;
                }
                if from > position {
                    runs.push((position..from, HighlightStyle::default()));
                }
                runs.push((from..to, style_of(token, resolver)));
                position = to;
            }
        }
        if position < end {
            runs.push((position..end, HighlightStyle::default()));
        }
        runs
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

#[cfg(test)]
fn kinds(line: &str) -> Vec<(&str, Token)> {
    scan_line(line, None)
        .into_iter()
        .map(|(span, token)| (&line[span], token))
        .collect()
}
