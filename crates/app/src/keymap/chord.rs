use gpui_kit::Keystroke;
use zenkai_i18n::t;

const MODIFIER_KEYS: [&str; 5] = ["shift", "control", "alt", "platform", "function"];
const NAMED_KEYS: [&str; 16] = [
    "escape",
    "enter",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "home",
    "end",
    "pageup",
    "pagedown",
    "up",
    "down",
    "left",
    "right",
    "menu",
];
const LAST_FUNCTION_KEY: u8 = 24;

// One keystroke in GPUI's own notation, always in the form `Keystroke::unparse` writes, so
// two chords are the same shortcut exactly when they are equal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Chord(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordError {
    Empty,
    Sequence,
    Unreadable,
    UnknownKey,
    OnlyModifiers,
}

impl ChordError {
    pub fn reason(self) -> &'static str {
        match self {
            ChordError::Empty => t!("keymap.reason.empty"),
            ChordError::Sequence => t!("keymap.reason.sequence"),
            ChordError::Unreadable => t!("keymap.reason.unreadable"),
            ChordError::UnknownKey => t!("keymap.reason.unknown_key"),
            ChordError::OnlyModifiers => t!("keymap.reason.only_modifiers"),
        }
    }
}

impl Chord {
    pub fn parse(text: &str) -> Result<Chord, ChordError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ChordError::Empty);
        }
        if text.contains(char::is_whitespace) {
            return Err(ChordError::Sequence);
        }
        let stroke = Keystroke::parse(text).map_err(|_| ChordError::Unreadable)?;
        if stroke.key_char.is_some() {
            return Err(ChordError::Unreadable);
        }
        Chord::from_keystroke(&stroke)
    }

    pub fn from_keystroke(stroke: &Keystroke) -> Result<Chord, ChordError> {
        let key = stroke.key.to_lowercase();
        if MODIFIER_KEYS.contains(&key.as_str()) {
            return Err(ChordError::OnlyModifiers);
        }
        if !is_known_key(&key) {
            return Err(ChordError::UnknownKey);
        }
        let canonical = Keystroke {
            modifiers: stroke.modifiers,
            key,
            key_char: None,
        };
        Ok(Chord(canonical.unparse()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn display(&self) -> String {
        match Keystroke::parse(&self.0) {
            Ok(stroke) => display(&stroke),
            Err(_) => self.0.clone(),
        }
    }
}

fn is_known_key(key: &str) -> bool {
    key.chars().count() == 1
        || NAMED_KEYS.contains(&key)
        || key
            .strip_prefix('f')
            .and_then(|number| number.parse::<u8>().ok())
            .is_some_and(|number| (1..=LAST_FUNCTION_KEY).contains(&number))
}

pub fn display(stroke: &Keystroke) -> String {
    let modifiers = &stroke.modifiers;
    let mut parts: Vec<String> = Vec::new();
    if modifiers.control {
        parts.push("Ctrl".into());
    }
    if modifiers.alt {
        parts.push("Alt".into());
    }
    if modifiers.shift {
        parts.push("Shift".into());
    }
    if modifiers.platform {
        parts.push("Win".into());
    }
    parts.push(match stroke.key.as_str() {
        "escape" => "Esc".into(),
        "pagedown" => "PgDn".into(),
        "pageup" => "PgUp".into(),
        "delete" => "Del".into(),
        key if key.chars().count() == 1 => key.to_uppercase(),
        key => {
            let mut chars = key.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    });
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings_of_one_shortcut_are_equal() {
        assert_eq!(Chord::parse("ctrl-shift-s"), Chord::parse("shift-ctrl-S"));
        assert_eq!(Chord::parse("ctrl-b").unwrap().as_str(), "ctrl-b");
    }

    #[test]
    fn symbols_and_named_keys_are_accepted() {
        for text in [
            "ctrl--",
            "ctrl->",
            "ctrl-`",
            "f12",
            "ctrl-alt-pagedown",
            "alt-=",
        ] {
            assert!(Chord::parse(text).is_ok(), "{text}");
        }
    }

    #[test]
    fn bad_text_is_refused_with_a_reason() {
        assert_eq!(Chord::parse("  "), Err(ChordError::Empty));
        assert_eq!(Chord::parse("ctrl-k ctrl-c"), Err(ChordError::Sequence));
        assert_eq!(Chord::parse("ctrl-bogus"), Err(ChordError::UnknownKey));
        assert_eq!(Chord::parse("ctrl-f99"), Err(ChordError::UnknownKey));
        assert_eq!(Chord::parse("ctrl-shift"), Err(ChordError::OnlyModifiers));
        assert_eq!(Chord::parse("ctrl-a->b"), Err(ChordError::Unreadable));
    }

    #[test]
    fn display_follows_excel() {
        assert_eq!(
            Chord::parse("ctrl-shift-pagedown").unwrap().display(),
            "Ctrl+Shift+PgDn"
        );
        assert_eq!(Chord::parse("f2").unwrap().display(), "F2");
        assert_eq!(Chord::parse("alt-=").unwrap().display(), "Alt+=");
    }
}
