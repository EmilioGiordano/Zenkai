use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use zenkai_i18n::t;

use super::controls::{card, group_title};
use crate::palette;

pub struct Shortcut {
    pub group: &'static str,
    pub label: &'static str,
    pub keys: Vec<String>,
}

pub fn collect(cx: &App) -> Vec<Shortcut> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    palette::table()
        .into_iter()
        .flat_map(|group| {
            let label = group.label;
            group.entries.into_iter().map(move |entry| (label, entry))
        })
        .map(|(group, entry)| {
            let mut keys: Vec<String> = Vec::new();
            for binding in keymap.bindings_for_action(entry.action.as_ref()) {
                let Some(stroke) = binding.keystrokes().first() else {
                    continue;
                };
                let text = pretty(stroke.inner());
                if !keys.contains(&text) {
                    keys.push(text);
                }
            }
            Shortcut {
                group,
                label: entry.label,
                keys,
            }
        })
        .collect()
}

fn pretty(stroke: &Keystroke) -> String {
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

pub fn matching<'a>(shortcuts: &'a [Shortcut], query: &str) -> Vec<&'a Shortcut> {
    let query = query.trim().to_lowercase();
    shortcuts
        .iter()
        .filter(|shortcut| {
            query.is_empty()
                || shortcut.label.to_lowercase().contains(&query)
                || shortcut.group.to_lowercase().contains(&query)
                || shortcut
                    .keys
                    .iter()
                    .any(|keys| keys.to_lowercase().contains(&query))
        })
        .collect()
}

fn keys_cell(keys: &[String], cx: &App) -> Div {
    let theme = cx.theme();
    if keys.is_empty() {
        return div()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(t!("settings.no_shortcut"));
    }
    h_flex().gap_1p5().children(keys.iter().map(|keys| {
        div()
            .px_1p5()
            .py_0p5()
            .rounded_md()
            .bg(theme.secondary)
            .font_family("monospace")
            .text_xs()
            .child(keys.clone())
    }))
}

pub fn render(shortcuts: &[&Shortcut], cx: &App) -> Vec<AnyElement> {
    let mut groups: Vec<(&'static str, Vec<AnyElement>)> = Vec::new();
    for shortcut in shortcuts {
        let row = h_flex()
            .w_full()
            .gap_4()
            .px_4()
            .py_2()
            .items_center()
            .child(div().flex_1().min_w_0().child(shortcut.label))
            .child(keys_cell(&shortcut.keys, cx))
            .into_any_element();
        match groups.last_mut() {
            Some((group, rows)) if *group == shortcut.group => rows.push(row),
            _ => groups.push((shortcut.group, vec![row])),
        }
    }
    groups
        .into_iter()
        .map(|(group, rows)| {
            v_flex()
                .gap_2()
                .child(group_title(group.to_uppercase(), cx))
                .child(card(rows, cx))
                .into_any_element()
        })
        .collect()
}
