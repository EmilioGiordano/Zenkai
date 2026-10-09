use gpui_kit::base::{Disableable, Selectable, h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use super::SettingsWindow;
use super::controls::{card, group_title};
use crate::actions::{RecordShortcutKeys, ResetAllShortcuts};
use crate::keymap::{self, Chord, Conflict, KeymapState, Model, Proposal};

const MODIFIER_KEYS: [&str; 5] = ["shift", "control", "alt", "platform", "function"];

pub struct Shortcut {
    pub index: usize,
    pub group: &'static str,
    pub label: &'static str,
    pub command: &'static str,
    pub keys: Vec<String>,
    pub modified: bool,
    pub fixed: bool,
}

pub fn collect(cx: &App) -> Vec<Shortcut> {
    let model = Model::build();
    let overrides = &cx.global::<KeymapState>().overrides;
    model
        .commands
        .iter()
        .enumerate()
        .map(|(index, command)| Shortcut {
            index,
            group: command.group,
            label: command.label,
            command: command.name,
            keys: model
                .keys(command, overrides)
                .iter()
                .map(Chord::display)
                .collect(),
            modified: overrides.get(command.name).is_some(),
            fixed: command.home().is_none(),
        })
        .collect()
}

pub fn modified_count(cx: &App) -> usize {
    cx.global::<KeymapState>().overrides.len()
}

pub fn matching<'a>(
    shortcuts: &'a [Shortcut],
    query: &str,
    pressed: Option<&str>,
    modified_only: bool,
) -> Vec<&'a Shortcut> {
    let query = query.trim().to_lowercase();
    shortcuts
        .iter()
        .filter(|shortcut| !modified_only || shortcut.modified)
        .filter(|shortcut| match pressed {
            Some(pressed) => shortcut.keys.iter().any(|keys| keys == pressed),
            None => {
                query.is_empty()
                    || shortcut.label.to_lowercase().contains(&query)
                    || shortcut.group.to_lowercase().contains(&query)
                    || shortcut
                        .keys
                        .iter()
                        .any(|keys| keys.to_lowercase().contains(&query))
            }
        })
        .collect()
}

#[derive(Default)]
enum Capture {
    #[default]
    Idle,
    Recording {
        command: &'static str,
        _grab: Subscription,
    },
    Finding {
        _grab: Subscription,
    },
}

#[derive(Default)]
pub struct KeyboardState {
    capture: Capture,
    proposal: Option<Proposal>,
    pub pressed: Option<String>,
}

impl KeyboardState {
    pub fn cancel(&mut self) -> bool {
        let busy = !matches!(self.capture, Capture::Idle) || self.proposal.is_some();
        self.capture = Capture::Idle;
        self.proposal = None;
        busy
    }
}

fn chips(keys: &[String], cx: &App) -> Div {
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

impl SettingsWindow {
    pub(super) fn start_recording(
        &mut self,
        command: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.keyboard.cancel();
        let grab = self.grab(window, cx);
        self.keyboard.capture = Capture::Recording {
            command,
            _grab: grab,
        };
        cx.notify();
    }

    pub(super) fn start_finding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_section(super::rows::Section::Keyboard, window, cx);
        let grab = self.grab(window, cx);
        self.keyboard.capture = Capture::Finding { _grab: grab };
        cx.notify();
    }

    // Keystrokes of this window go to the capture first and never reach the shortcuts they
    // would trigger. Lone modifier presses pass: they are only part of a combination.
    fn grab(&self, window: &Window, cx: &mut Context<Self>) -> Subscription {
        let this = cx.entity().downgrade();
        let own = window.window_handle();
        cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != own
                || MODIFIER_KEYS.contains(&event.keystroke.key.as_str())
            {
                return;
            }
            cx.stop_propagation();
            let keystroke = event.keystroke.clone();
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.captured(keystroke, window, cx));
            }
        })
    }

    fn captured(&mut self, keystroke: Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let bare = keystroke.modifiers == Modifiers::none();
        if bare && keystroke.key == "escape" {
            self.keyboard.cancel();
            cx.notify();
            return;
        }
        match self.keyboard.capture {
            Capture::Idle => {}
            Capture::Recording { command, .. } => {
                if bare && matches!(keystroke.key.as_str(), "backspace" | "delete") {
                    self.keyboard.cancel();
                    keymap::remove(cx, command);
                } else if let Ok(chord) = Chord::from_keystroke(&keystroke) {
                    self.keyboard.cancel();
                    let proposal = keymap::propose(cx, command, chord);
                    if proposal.conflict.is_some() {
                        self.keyboard.proposal = Some(proposal);
                    } else {
                        keymap::commit(cx, &proposal);
                    }
                }
            }
            Capture::Finding { .. } => {
                if let Ok(chord) = Chord::from_keystroke(&keystroke) {
                    self.keyboard.cancel();
                    let shown = chord.display();
                    self.query = shown.clone();
                    self.keyboard.pressed = Some(shown.clone());
                    self.search
                        .update(cx, |search, cx| search.set_value(shown, window, cx));
                }
            }
        }
        cx.notify();
    }

    fn replace_proposed(&mut self, cx: &mut Context<Self>) {
        if let Some(proposal) = self.keyboard.proposal.take() {
            keymap::commit(cx, &proposal);
        }
        cx.notify();
    }

    pub(super) fn shortcut_tools(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let finding = matches!(self.keyboard.capture, Capture::Finding { .. });
        let customized = modified_count(cx) > 0;
        let find = Button::new("record-keys")
            .selected(finding)
            .label(if finding {
                t!("settings.shortcut.press_keys")
            } else {
                t!("settings.shortcut.record_keys")
            })
            .accessibility_label(t!("settings.shortcut.record_keys"))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(RecordShortcutKeys), cx));
        let reset = Button::new("reset-all-shortcuts")
            .label(t!("settings.shortcut.reset_all"))
            .disabled(!customized)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ResetAllShortcuts), cx));
        let card_rows = vec![
            h_flex()
                .w_full()
                .gap_4()
                .px_4()
                .py_3p5()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(t!("settings.shortcut.find_title")),
                        )
                        .child(div().text_xs().text_color(theme.muted_foreground).child(
                            if finding {
                                t!("settings.shortcut.finding_hint")
                            } else {
                                t!("settings.shortcut.find_lead")
                            },
                        )),
                )
                .child(h_flex().gap_2().child(find).child(reset))
                .into_any_element(),
        ];
        v_flex()
            .gap_2()
            .child(group_title(t!("settings.shortcut.search_group"), cx))
            .child(card(card_rows, cx))
            .into_any_element()
    }

    pub(super) fn shortcut_blocks(
        &self,
        shortcuts: &[&Shortcut],
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let mut groups: Vec<(&'static str, Vec<AnyElement>)> = Vec::new();
        for shortcut in shortcuts {
            let first = groups
                .last()
                .is_none_or(|(group, _)| *group != shortcut.group);
            let row = self.shortcut_row(shortcut, first, cx);
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

    fn shortcut_row(&self, shortcut: &Shortcut, first: bool, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let command = shortcut.command;
        let recording = matches!(
            self.keyboard.capture,
            Capture::Recording { command: recorded, .. } if recorded == command
        );
        let proposal = self
            .keyboard
            .proposal
            .as_ref()
            .filter(|proposal| proposal.command == command);
        let heading = h_flex()
            .gap_2()
            .items_center()
            .child(div().font_weight(FontWeight::MEDIUM).child(shortcut.label))
            .when(shortcut.modified, |this| {
                this.child(
                    div()
                        .size(px(6.0))
                        .rounded_full()
                        .bg(theme.warning)
                        .flex_shrink_0(),
                )
                .child(
                    Button::new(("reset-shortcut", shortcut.index))
                        .ghost()
                        .compact()
                        .label(t!("settings.reset"))
                        .accessibility_label(t!("settings.reset_row", title = shortcut.label))
                        .on_click(move |_, _, cx| keymap::reset(cx, command)),
                )
            });
        let control =
            if shortcut.fixed {
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t!("settings.shortcut.fixed")),
                    )
                    .child(chips(&shortcut.keys, cx))
                    .into_any_element()
            } else {
                let shown = if recording {
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .bg(theme.success.opacity(0.14))
                        .border_1()
                        .border_color(theme.success)
                        .font_family("monospace")
                        .text_xs()
                        .child(t!("settings.shortcut.press_keys"))
                        .into_any_element()
                } else {
                    chips(&shortcut.keys, cx).into_any_element()
                };
                Button::new(("shortcut", shortcut.index))
                    .ghost()
                    .selected(recording)
                    .accessibility_label(t!(
                        "settings.shortcut.change",
                        command = shortcut.label,
                        keys = if shortcut.keys.is_empty() {
                            t!("settings.no_shortcut").to_string()
                        } else {
                            shortcut.keys.join(", ")
                        }
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_recording(command, window, cx)
                    }))
                    .child(shown)
                    .into_any_element()
            };
        let notice = if recording {
            Some(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("settings.shortcut.recording_hint"))
                    .into_any_element(),
            )
        } else {
            proposal.map(|proposal| self.conflict_notice(proposal, cx))
        };
        v_flex()
            .w_full()
            .gap_2()
            .px_4()
            .py_2p5()
            .when(!first, |this| {
                this.border_t_1().border_color(theme.background)
            })
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(heading))
                    .child(control),
            )
            .children(notice)
            .into_any_element()
    }

    fn conflict_notice(&self, proposal: &Proposal, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let model = Model::build();
        let keys = proposal.chord.display();
        let (text, replaceable) = match &proposal.conflict {
            Some(Conflict::Replaceable(others)) => {
                let other = others
                    .iter()
                    .map(|name| model.label_of(name))
                    .collect::<Vec<_>>()
                    .join(", ");
                (
                    t!(
                        "settings.shortcut.conflict_replaceable",
                        keys = keys,
                        other = other
                    ),
                    true,
                )
            }
            Some(Conflict::Fixed(name)) => (
                t!(
                    "settings.shortcut.conflict_fixed",
                    keys = keys,
                    other = model.label_of(name)
                ),
                false,
            ),
            None => return div().into_any_element(),
        };
        h_flex()
            .gap_3()
            .items_center()
            .text_xs()
            .text_color(theme.warning)
            .child(div().flex_1().min_w_0().child(format!("⚠ {text}")))
            .when(replaceable, |this| {
                this.child(
                    Button::new("replace-shortcut")
                        .compact()
                        .label(t!("settings.shortcut.replace"))
                        .on_click(cx.listener(|this, _, _, cx| this.replace_proposed(cx))),
                )
            })
            .child(
                Button::new("cancel-shortcut")
                    .compact()
                    .ghost()
                    .label(t!("settings.shortcut.cancel"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.keyboard.cancel();
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
