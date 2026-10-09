mod agents;
mod ai;
mod brand;
mod choices;
mod controls;
mod info;
mod keyboard;
mod nav;
mod page;
mod rows;

use std::collections::BTreeMap;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::command::CommandState;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, TitleBar};
use gpui_kit::*;
use zenkai_agent::settings::{AgentId, HeldChange, SecretName};
use zenkai_i18n::t;

use crate::actions::*;
use crate::agent_settings::{self, HeldDecision};
use crate::keymap::KeymapState;
use crate::space_settings;
use keyboard::{KeyboardState, Shortcut};
use rows::{RowId, Section, Values};

const DEFAULT_SIZE: (f32, f32) = (1120.0, 820.0);
const MIN_SIZE: (f32, f32) = (760.0, 520.0);

pub use agents::register as register_agent_actions;

struct OpenWindow {
    handle: AnyWindowHandle,
    window: WeakEntity<SettingsWindow>,
}

impl Global for OpenWindow {}

pub fn open(cx: &mut App) {
    show(cx);
}

pub fn open_to_find_shortcut(cx: &mut App) {
    let Some((handle, window)) = show(cx) else {
        return;
    };
    if let Err(error) = handle.update(cx, |_, native, cx| {
        window.update(cx, |this, cx| this.start_finding(native, cx))
    }) {
        tracing::debug!(%error, "the settings window is already being updated");
    }
}

fn show(cx: &mut App) -> Option<(AnyWindowHandle, Entity<SettingsWindow>)> {
    let existing = cx
        .try_global::<OpenWindow>()
        .filter(|open| cx.windows().contains(&open.handle))
        .and_then(|open| Some((open.handle, open.window.upgrade()?)));
    if let Some((handle, window)) = existing {
        // Fails when the request comes from inside the settings window, which is already
        // in front.
        if let Err(error) = handle.update(cx, |_, window, _| window.activate_window()) {
            tracing::debug!(%error, "the settings window is already being updated");
        }
        return Some((handle, window));
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(
            size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)),
            cx,
        )),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        ..TitleBar::window_options()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        window.set_window_title(t!("settings.window_title"));
        cx.new(|cx| SettingsWindow::new(window, cx))
    }) {
        Ok((handle, window)) => {
            cx.set_global(OpenWindow {
                handle,
                window: window.downgrade(),
            });
            Some((handle, window))
        }
        Err(error) => {
            tracing::error!(%error, "could not open the settings window");
            None
        }
    }
}

// Callbacks of widgets outlive a render, so they reach the window through a weak handle.
fn update(
    window: &WeakEntity<SettingsWindow>,
    cx: &mut App,
    change: impl FnOnce(&mut SettingsWindow, &mut Context<SettingsWindow>),
) {
    if let Err(error) = window.update(cx, change) {
        tracing::debug!(%error, "the settings window closed");
    }
}

struct OpenDropdown {
    row: RowId,
    list: Entity<CommandState>,
}

pub struct SettingsWindow {
    focus: FocusHandle,
    section: Section,
    search: Entity<InputState>,
    query: String,
    modified_only: bool,
    dropdown: Option<OpenDropdown>,
    configuring: Option<AgentId>,
    secret_inputs: BTreeMap<SecretName, Entity<InputState>>,
    claude_command: Option<String>,
    shortcuts: Vec<Shortcut>,
    keyboard: KeyboardState,
    shown_held: Option<HeldChange>,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> SettingsWindow {
        agent_settings::detect_agents(cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("settings.search")));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let subscriptions = vec![
            cx.subscribe(&search, |this, search, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = search.read(cx).value().to_string();
                    if this.keyboard.pressed.as_deref() != Some(&this.query) {
                        this.keyboard.pressed = None;
                    }
                    this.dropdown = None;
                    this.scroll.set_offset(point(px(0.0), px(0.0)));
                    cx.notify();
                }
            }),
            cx.observe_global::<agent_settings::AgentConfig>(|_, cx| cx.notify()),
            cx.observe_global::<KeymapState>(|this, cx| {
                this.shortcuts = keyboard::collect(cx);
                cx.notify();
            }),
            cx.observe_global::<crate::space_appearance::SpaceAppearance>(|_, cx| cx.notify()),
        ];
        SettingsWindow {
            focus,
            section: Section::General,
            search,
            query: String::new(),
            modified_only: false,
            dropdown: None,
            configuring: None,
            secret_inputs: BTreeMap::new(),
            claude_command: agents::claude_command(),
            shortcuts: keyboard::collect(cx),
            keyboard: KeyboardState::default(),
            shown_held: None,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.keyboard.cancel() {
            cx.notify();
        } else if self.dropdown.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        } else {
            window.remove_window();
        }
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        self.query.clear();
        self.keyboard.pressed = None;
    }

    fn select_section(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_search(window, cx);
        self.section = section;
        self.modified_only = false;
        self.dropdown = None;
        self.scroll.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    fn step_section(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let all = Section::ALL;
        let at = all.iter().position(|s| *s == self.section).unwrap_or(0);
        let next = if forward {
            (at + 1) % all.len()
        } else {
            (at + all.len() - 1) % all.len()
        };
        self.select_section(all[next], window, cx);
    }

    fn toggle_modified_only(&mut self, cx: &mut Context<Self>) {
        self.modified_only = !self.modified_only;
        self.dropdown = None;
        self.scroll.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    fn values<'a>(
        settings: &'a zenkai_agent::settings::Settings,
        spaces: &'a crate::space_appearance::SpaceAppearance,
        shortcuts: usize,
    ) -> Values<'a> {
        Values {
            settings,
            spaces,
            shortcuts,
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = agent_settings::settings(cx).clone();
        self.shown_held = cx
            .global::<agent_settings::AgentConfig>()
            .state
            .held
            .clone();
        let spaces = space_settings::current(cx);
        let values = Self::values(&settings, &spaces, keyboard::modified_count(cx));
        let nav = self.nav(values, cx);
        let content = self.content(values, window, cx);
        let theme = cx.theme();
        v_flex()
            .key_context(if self.shown_held.is_some() {
                "SettingsWindow HeldPending"
            } else {
                "SettingsWindow"
            })
            .track_focus(&self.focus)
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|this, _: &CloseSettings, window, cx| this.close(window, cx)))
            .on_action(|_: &OpenSettings, window, _| window.activate_window())
            .on_action(cx.listener(|this, _: &RecordShortcutKeys, window, cx| {
                this.start_finding(window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusSettingsSearch, window, cx| {
                this.search
                    .update(cx, |search, cx| search.focus(window, cx));
            }))
            .on_action(
                cx.listener(|this, _: &ToggleModifiedOnly, _, cx| this.toggle_modified_only(cx)),
            )
            .on_action(cx.listener(|this, _: &NextSettingsSection, window, cx| {
                this.step_section(true, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &PreviousSettingsSection, window, cx| {
                    this.step_section(false, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ApplyHeldSettings, _, cx| {
                agent_settings::decide_held(cx, this.shown_held.clone(), HeldDecision::Apply)
            }))
            .on_action(cx.listener(|this, _: &KeepCurrentSettings, _, cx| {
                agent_settings::decide_held(cx, this.shown_held.clone(), HeldDecision::Keep)
            }))
            .on_action(|_: &FocusNextControl, window, cx| window.focus_next(cx))
            .on_action(|_: &FocusPreviousControl, window, cx| window.focus_prev(cx))
            .on_action(
                cx.listener(|this, _: &SaveSecrets, window, cx| {
                    this.save_typed_secrets(window, cx)
                }),
            )
            .child(
                TitleBar::new().child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(t!("settings.window_title")),
                ),
            )
            .child(h_flex().flex_1().min_h_0().child(nav).child(content))
    }
}
