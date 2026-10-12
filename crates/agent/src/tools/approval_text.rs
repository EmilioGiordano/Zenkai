// What the user reads before allowing an agent write. Entries and sheet names come from
// the agent, which may have read them from the file: control and format characters
// (line breaks, right-to-left overrides) are shown escaped so the text cannot be
// reshaped, and formulas are marked so the user sees they will compute.

use zenkai_i18n::t;

const SHOWN_CHARS: usize = 40;
const LISTED_FUNCTIONS: usize = 5;

// Unicode general category Cf; std only knows the control category Cc.
fn is_format_char(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

fn escaped(c: char) -> String {
    if c.is_control() {
        c.escape_debug().collect()
    } else if is_format_char(c) {
        format!("\\u{{{:X}}}", u32::from(c))
    } else {
        c.to_string()
    }
}

// The first characters of `text`, escaped, with an ellipsis when cut.
pub fn shown_text(text: &str) -> String {
    let mut shown: String = text.chars().take(SHOWN_CHARS).map(escaped).collect();
    if text.chars().count() > SHOWN_CHARS {
        shown.push('…');
    }
    shown
}

// Names called in the part of a formula the prompt does not show.
fn functions_after_cut(formula: &str) -> Vec<String> {
    let rest: String = formula.chars().skip(SHOWN_CHARS).collect();
    let mut names: Vec<String> = Vec::new();
    let mut current = String::new();
    for c in rest.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            current.push(c.to_ascii_uppercase());
            continue;
        }
        if c == '('
            && current.starts_with(|first: char| first.is_ascii_alphabetic())
            && !names.contains(&current)
        {
            names.push(current.clone());
        }
        current.clear();
    }
    names
}

pub fn shown_entry(entry: &str) -> String {
    if !entry.starts_with('=') {
        return format!("\"{}\"", shown_text(entry));
    }
    let total = entry.chars().count();
    if total <= SHOWN_CHARS {
        return t!("plan.formula", text = shown_text(entry));
    }
    let hidden = functions_after_cut(entry);
    let listed: Vec<String> = hidden.iter().take(LISTED_FUNCTIONS).cloned().collect();
    let more = if hidden.len() > LISTED_FUNCTIONS {
        ", …"
    } else {
        ""
    };
    let uses = if listed.is_empty() {
        String::new()
    } else {
        t!("plan.formula_calls", names = listed.join(", "), more = more)
    };
    t!(
        "plan.formula_long",
        text = shown_text(entry),
        total = total,
        uses = uses
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaks_and_direction_overrides_are_shown_escaped() {
        assert_eq!(shown_text("a\nb"), "a\\nb");
        assert_eq!(shown_text("x\u{202E}cod.exe"), "x\\u{202E}cod.exe");
        assert_eq!(shown_text("\u{200B}"), "\\u{200B}");
        assert_eq!(shown_text("Año"), "Año");
    }

    #[test]
    fn long_formulas_name_what_they_call_past_the_cut() {
        let formula =
            "=IF(A1>0,\"positive and quite a long text here\",VLOOKUP(A1,B:C,2,FALSE)+SUM(D1:D9))";
        let shown = shown_entry(formula);
        assert!(shown.starts_with("formula =IF(A1>0,"), "{shown}");
        assert!(shown.contains(&format!("({} characters", formula.chars().count())));
        assert!(shown.contains("VLOOKUP, SUM"), "{shown}");
        assert_eq!(shown_entry("=A1*2"), "formula =A1*2");
    }
}
