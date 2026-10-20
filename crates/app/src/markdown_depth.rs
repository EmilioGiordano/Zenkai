// Nesting deeper than this is not parsed for the preview: the parser and the layout recurse
// once per level, so a file built from thousands of `>` or `[` could exhaust the stack.
pub const MAX_NESTING: usize = 64;

const TAB_WIDTH: usize = 4;
const LIST_INDENT: usize = 2;

// One pass over the text, linear in its length. It counts, per line, the block prefix
// (quotes and list levels) plus the inline nesting (open brackets and emphasis), and stops at
// the first line over the limit.
pub fn too_deep(text: &str) -> bool {
    text.split('\n').any(|line| line_depth(line) > MAX_NESTING)
}

fn line_depth(line: &str) -> usize {
    let (blocks, rest) = block_depth(line);
    blocks + inline_depth(rest)
}

fn block_depth(line: &str) -> (usize, &str) {
    let bytes = line.as_bytes();
    let mut depth = 0;
    let mut indent = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b' ' => indent += 1,
            b'\t' => indent += TAB_WIDTH,
            b'>' => {
                depth += 1;
                if depth > MAX_NESTING {
                    return (depth, "");
                }
            }
            b'-' | b'*' | b'+' if bytes.get(at + 1) == Some(&b' ') => depth += 1,
            _ => break,
        }
        at += 1;
    }
    (depth + indent / LIST_INDENT, &line[at..])
}

fn inline_depth(text: &str) -> usize {
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
        if deepest > MAX_NESTING {
            return deepest;
        }
        at += 1;
    }
    deepest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_documents_are_not_too_deep() {
        let text = "# Title\n\n> quote with **bold** and [a link](http://x.y)\n\n- item\n  - nested\n    - deeper\n\n1. one\n2. two\n";
        assert!(!too_deep(text));
    }

    #[test]
    fn a_few_megabytes_of_quote_markers_are_refused_in_one_pass() {
        let text = ">".repeat(4 * 1024 * 1024);
        assert!(too_deep(&text));
        let spaced = "> ".repeat(2 * 1024 * 1024);
        assert!(too_deep(&spaced));
    }

    #[test]
    fn deep_lists_brackets_and_emphasis_are_refused() {
        let list: String = (0..100)
            .map(|level| format!("{}- item\n", " ".repeat(level * 2)))
            .collect();
        assert!(too_deep(&list));
        assert!(too_deep(&"[".repeat(1_000_000)));
        assert!(too_deep(&"*a ".repeat(100_000)));
        assert!(too_deep(&"_a ".repeat(100_000)));
    }

    #[test]
    fn the_limit_itself_is_allowed() {
        assert!(!too_deep(&">".repeat(MAX_NESTING)));
        assert!(too_deep(&">".repeat(MAX_NESTING + 1)));
    }

    #[test]
    fn many_lines_that_are_each_shallow_are_fine() {
        assert!(!too_deep(&"> quoted line\n".repeat(200_000)));
        assert!(!too_deep(&"**bold** and *it* and [x](y)\n".repeat(100_000)));
    }
}
