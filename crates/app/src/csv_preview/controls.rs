use gpui_kit::base::h_flex;
use gpui_kit::component::Theme;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::interface_scale::rem_px;

// The label split around the letter its Alt key is, so the letter can be underlined while Alt
// is held. A key the label does not hold leaves it whole.
pub struct Mnemonic<'a> {
    pub before: &'a str,
    pub letter: &'a str,
    pub after: &'a str,
}

pub fn mnemonic<'a>(label: &'a str, key: &str) -> Mnemonic<'a> {
    let found = key.chars().next().and_then(|key| {
        label
            .char_indices()
            .find(|(_, letter)| letter.eq_ignore_ascii_case(&key))
    });
    match found {
        Some((at, letter)) => {
            let end = at + letter.len_utf8();
            Mnemonic {
                before: &label[..at],
                letter: &label[at..end],
                after: &label[end..],
            }
        }
        None => Mnemonic {
            before: label,
            letter: "",
            after: "",
        },
    }
}

pub fn labelled(label: &str, key: &str, underlined: bool) -> Div {
    let parts = mnemonic(label, key);
    h_flex()
        .whitespace_nowrap()
        .child(parts.before.to_string())
        .child(
            div()
                .when(underlined, |letter| letter.underline())
                .child(parts.letter.to_string()),
        )
        .child(parts.after.to_string())
}

// The focus of a stop that is not a button: 2px of space, a 2px ring, then a 1px line of the
// foreground outside it, so the ring is seen on every palette even where it is weak.
pub fn focus_ring(theme: &Theme) -> Div {
    div()
        .absolute()
        .top(px(-5.0))
        .left(px(-5.0))
        .right(px(-5.0))
        .bottom(px(-5.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(theme.foreground)
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .rounded(px(5.0))
                .border_2()
                .border_color(theme.ring),
        )
}

pub const DISABLED_OPACITY: f32 = 0.55;
const BOX: f32 = 16.0;

pub fn checkbox_box(checked: bool, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(rem_px(BOX))
        .rounded(px(4.0))
        .border_2()
        .border_color(theme.foreground)
        .when(checked, |this| {
            this.bg(theme.primary)
                .text_color(theme.primary_foreground)
                .child("\u{2713}")
        })
}

pub fn radio_box(selected: bool, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(rem_px(BOX))
        .rounded_full()
        .border_2()
        .border_color(theme.foreground)
        .when(selected, |this| {
            this.child(div().size(rem_px(8.0)).rounded_full().bg(theme.foreground))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_letter_of_an_alt_key_is_found_in_the_label_whatever_its_case() {
        let general = mnemonic("General", "G");
        assert_eq!(
            (general.before, general.letter, general.after),
            ("", "G", "eneral")
        );
        let keep = mnemonic("Keep everything as text", "k");
        assert_eq!(keep.letter, "K");
        let text = mnemonic("Texto", "T");
        assert_eq!((text.before, text.letter, text.after), ("", "T", "exto"));
    }

    #[test]
    fn a_label_without_the_letter_stays_whole() {
        let none = mnemonic("Mantener todo como texto", "K");
        assert_eq!(none.before, "Mantener todo como texto");
        assert_eq!(none.letter, "");
        let accented = mnemonic("\u{c1}rbol", "r");
        assert_eq!((accented.before, accented.letter), ("\u{c1}", "r"));
    }
}
