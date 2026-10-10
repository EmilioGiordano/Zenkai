use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::chat::state::{Choice, ConfigKind, Select};
use zenkai_agent::settings::AgentId;
use zenkai_i18n::t;

use super::access::Access;
use super::{ChatPanel, Link, Menu};

const MODEL_MENU_WIDTH: f32 = 340.0;
const ACCESS_MENU_WIDTH: f32 = 300.0;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Agent(AgentId),
    Manage,
    Model(String),
    Effort,
    Access(Access),
}

fn heading(text: impl AsRef<str>, cx: &App) -> Div {
    div()
        .px_2p5()
        .pt_1p5()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text.as_ref().to_uppercase())
}

pub(super) fn rule(cx: &App) -> Div {
    div().h(px(1.0)).mx_1().my_1p5().bg(cx.theme().border)
}

pub(super) fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px_2p5()
        .pb_1p5()
        .text_xs()
        .line_height(relative(1.4))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

pub(super) fn popover(
    id: &'static str,
    label: &'static str,
    width: f32,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    div()
        .id(id)
        .role(Role::Menu)
        .aria_label(label)
        .occlude()
        .w(px(width))
        .p_1p5()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .shadow_lg()
}

pub(super) struct Item {
    pub id: SharedString,
    pub name: SharedString,
    pub note: Option<SharedString>,
    pub checked: bool,
    pub highlighted: bool,
}

pub(super) fn item(item: Item, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(item.id)
        .role(Role::MenuItemRadio)
        .aria_label(item.name.clone())
        .gap_2p5()
        .items_start()
        .px_2p5()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        .hover(|row| row.bg(theme.accent))
        .when(item.highlighted, |row| {
            row.bg(theme.accent).text_color(theme.accent_foreground)
        })
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().child(item.name))
                .when_some(item.note, |column, text| {
                    column.child(
                        div()
                            .text_xs()
                            .line_height(relative(1.4))
                            .text_color(theme.muted_foreground)
                            .child(text),
                    )
                }),
        )
        .child(
            div()
                .w(px(14.0))
                .flex_shrink_0()
                .when(item.checked, |slot| {
                    slot.child(Icon::new(IconName::Check).size_3p5())
                }),
        )
}

impl ChatPanel {
    pub(super) fn available_access(&self) -> Vec<Access> {
        match self.state.select(ConfigKind::Mode) {
            Some(modes) => Access::ALL
                .into_iter()
                .filter(|access| access.session_mode(modes).is_some())
                .collect(),
            None => Access::ALL.to_vec(),
        }
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        match self.menu {
            Some(Menu::Model) => {
                let mut rows: Vec<Row> = self
                    .state
                    .select(ConfigKind::Model)
                    .map(|models| {
                        models
                            .choices
                            .iter()
                            .map(|choice| Row::Model(choice.value.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                if self.state.select(ConfigKind::Effort).is_some() {
                    rows.push(Row::Effort);
                }
                if rows.is_empty() && self.failed_to_start() {
                    rows.push(Row::Manage);
                }
                rows
            }
            Some(Menu::Agent) => {
                let mut rows: Vec<Row> = self
                    .agent_rows(cx)
                    .into_iter()
                    .map(|option| Row::Agent(option.id))
                    .collect();
                rows.push(Row::Manage);
                rows
            }
            Some(Menu::Access) => self
                .available_access()
                .into_iter()
                .map(Row::Access)
                .collect(),
            None => Vec::new(),
        }
    }

    fn is_checked(&self, row: &Row, cx: &App) -> bool {
        match row {
            Row::Agent(id) => self.agent_rows(cx).iter().any(|o| o.active && o.id == *id),
            Row::Manage => false,
            Row::Model(value) => self
                .state
                .select(ConfigKind::Model)
                .is_some_and(|models| models.current == *value),
            Row::Effort => false,
            Row::Access(access) => *access == self.access,
        }
    }

    pub(super) fn toggle_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        if self.menu == Some(menu) {
            self.menu = None;
        } else {
            self.menu = Some(menu);
            self.menu_index = self
                .rows(cx)
                .iter()
                .position(|row| self.is_checked(row, cx))
                .unwrap_or(0);
        }
        cx.notify();
    }

    pub(crate) fn toggle_model_menu(&mut self, cx: &mut Context<Self>) {
        self.toggle_menu(Menu::Model, cx);
    }

    pub(crate) fn toggle_access_menu(&mut self, cx: &mut Context<Self>) {
        self.toggle_menu(Menu::Access, cx);
    }

    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
    }

    pub(crate) fn menu_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.rows(cx).len();
        if count == 0 {
            return;
        }
        self.menu_index = if forward {
            (self.menu_index + 1) % count
        } else {
            (self.menu_index + count - 1) % count
        };
        cx.notify();
    }

    pub(crate) fn menu_accept(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        let Some(row) = rows.get(self.menu_index.min(rows.len().saturating_sub(1))) else {
            return;
        };
        match row {
            Row::Agent(id) => self.pick_agent(id.clone(), cx),
            Row::Manage => self.open_agent_settings(cx),
            Row::Model(value) => self.pick(ConfigKind::Model, value, cx),
            Row::Effort => self.step_effort(cx),
            Row::Access(access) => {
                self.set_access(*access, cx);
                self.menu = None;
            }
        }
        cx.notify();
    }

    pub(super) fn pick(&mut self, kind: ConfigKind, value: &str, cx: &mut Context<Self>) {
        match kind {
            ConfigKind::Model => self.model_choice = Some(value.to_string()),
            ConfigKind::Effort => self.effort_choice = Some(value.to_string()),
            ConfigKind::Mode | ConfigKind::Other => {}
        }
        self.choose(kind, value, cx);
    }

    fn step_effort(&mut self, cx: &mut Context<Self>) {
        let Some(effort) = self.state.select(ConfigKind::Effort) else {
            return;
        };
        let position = effort
            .choices
            .iter()
            .position(|choice| choice.value == effort.current)
            .unwrap_or(0);
        let next = effort
            .choices
            .get((position + 1) % effort.choices.len().max(1))
            .map(|choice| choice.value.clone());
        if let Some(next) = next {
            self.pick(ConfigKind::Effort, &next, cx);
        }
    }

    pub(super) fn choose(&mut self, kind: ConfigKind, value: &str, cx: &mut Context<Self>) {
        let Some(select) = self.state.select(kind) else {
            return;
        };
        if !select.offers(value) || select.current == value {
            return;
        }
        if let Some(live) = &self.live {
            live.handle
                .set_config(select.id.clone(), select.source, value.to_string());
        }
        cx.notify();
    }

    pub(super) fn set_access(&mut self, access: Access, cx: &mut Context<Self>) {
        self.access = access;
        let wanted = access.setting();
        if cx
            .global::<crate::agent_settings::AgentConfig>()
            .state
            .current
            .agents
            .permission
            != wanted
        {
            crate::settings_window::set_permission(wanted, cx);
        }
        self.sync_mode(cx);
        cx.notify();
    }

    fn sync_mode(&mut self, cx: &mut Context<Self>) {
        let mode = self
            .state
            .select(ConfigKind::Mode)
            .and_then(|modes| self.access.session_mode(modes));
        if let Some(mode) = mode {
            self.choose(ConfigKind::Mode, &mode, cx);
        }
    }

    // The Settings page can change what agents may do while the chat is open.
    pub(super) fn follow_setting(&mut self, cx: &mut Context<Self>) {
        let setting = cx
            .global::<crate::agent_settings::AgentConfig>()
            .state
            .current
            .agents
            .permission;
        if self.access.setting() != setting {
            self.access = Access::from_setting(setting);
            self.sync_mode(cx);
        }
    }

    pub(super) fn cycle_access(&mut self, cx: &mut Context<Self>) {
        let available = self.available_access();
        let position = available.iter().position(|access| *access == self.access);
        let next = match position {
            Some(index) => available[(index + 1) % available.len()],
            None => available.first().copied().unwrap_or(self.access),
        };
        self.set_access(next, cx);
    }

    // What the user picked before the session existed is sent once the agent is ready.
    pub(super) fn apply_choices(&mut self, cx: &mut Context<Self>) {
        self.sync_mode(cx);
        if let Some(model) = self.model_choice.clone() {
            self.choose(ConfigKind::Model, &model, cx);
        }
        if let Some(effort) = self.effort_choice.clone() {
            self.choose(ConfigKind::Effort, &effort, cx);
        }
        self.mode_synced = true;
    }

    // The label follows the agent's own mode changes, such as leaving plan mode once the user
    // approved its plan.
    pub(super) fn follow_agent_mode(&mut self, cx: &mut Context<Self>) {
        if !self.mode_synced {
            return;
        }
        let reported = self
            .state
            .select(ConfigKind::Mode)
            .and_then(|modes| Access::from_session_mode(&modes.current));
        if let Some(access) = reported.filter(|access| *access != self.access) {
            self.set_access(access, cx);
        }
    }

    pub(super) fn model_label(&self) -> String {
        let model = self.state.select(ConfigKind::Model).map_or_else(
            || t!("chat.model.default").to_string(),
            |select| select.current_label().to_string(),
        );
        match self.state.select(ConfigKind::Effort) {
            Some(effort) => format!("{model}, {}", effort.current_label()),
            None => model,
        }
    }

    fn failed_to_start(&self) -> bool {
        self.problem.is_some() && !matches!(self.link, Link::Ready(_))
    }

    fn absent_note(&self, kind: ConfigKind, cx: &App) -> String {
        if self.failed_to_start() {
            return t!("chat.menu.not_running", agent = self.agent_name(cx));
        }
        if matches!(
            self.link,
            Link::Preparing | Link::Installing | Link::Starting
        ) {
            return t!("chat.menu.starting", agent = self.agent_name(cx));
        }
        if !matches!(self.link, Link::Ready(_)) {
            return t!("chat.menu.pending").to_string();
        }
        let text = match kind {
            ConfigKind::Model => t!("chat.model.unreported"),
            ConfigKind::Effort => t!("chat.effort.unreported"),
            ConfigKind::Mode | ConfigKind::Other => t!("chat.access.no_modes"),
        };
        text.to_string()
    }

    fn model_row(
        &self,
        index: usize,
        choice: &Choice,
        current: &str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let value = choice.value.clone();
        item(
            Item {
                id: SharedString::from(format!("model-{}", choice.value)),
                name: choice.label.clone().into(),
                note: choice.description.clone().map(Into::into),
                checked: current == choice.value,
                highlighted: self.menu_index == index,
            },
            cx,
        )
        .on_click(cx.listener(move |this, _, _, cx| this.pick(ConfigKind::Model, &value, cx)))
    }

    fn effort_control(
        &self,
        effort: &Select,
        highlighted: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let (background, ring, accent, accent_foreground, muted) = (
            theme.background,
            theme.ring,
            theme.accent,
            theme.accent_foreground,
            theme.muted_foreground,
        );
        h_flex()
            .id("chat-effort")
            .role(Role::RadioGroup)
            .aria_label(t!("chat.effort.heading"))
            .mx_1p5()
            .mb_1p5()
            .p_0p5()
            .gap_0p5()
            .rounded_lg()
            .bg(background)
            .border_1()
            .border_color(if highlighted { ring } else { background })
            .children(effort.choices.iter().map(|choice| {
                let value = choice.value.clone();
                let current = effort.current == choice.value;
                div()
                    .id(SharedString::from(format!("effort-{}", choice.value)))
                    .role(Role::RadioButton)
                    .aria_label(choice.label.clone())
                    .flex_1()
                    .min_w_0()
                    .py_1()
                    .rounded_md()
                    .text_center()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(if current { accent_foreground } else { muted })
                    .when(current, |segment| segment.bg(accent))
                    .child(choice.label.clone())
                    .on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.pick(ConfigKind::Effort, &value, cx)
                        }),
                    )
            }))
    }

    pub(super) fn render_model_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.menu != Some(Menu::Model) {
            return None;
        }
        let models = self.state.select(ConfigKind::Model).cloned();
        let effort = self.state.select(ConfigKind::Effort).cloned();
        let model_count = models.as_ref().map_or(0, |models| models.choices.len());
        if models.is_none() && effort.is_none() {
            return Some(self.unavailable_menu(cx));
        }
        let mut menu = popover(
            "chat-model-menu",
            t!("chat.model.heading"),
            MODEL_MENU_WIDTH,
            cx,
        )
        .child(heading(t!("chat.model.heading"), cx));
        match &models {
            Some(models) => {
                for (index, choice) in models.choices.iter().enumerate() {
                    menu = menu.child(self.model_row(index, choice, &models.current, cx));
                }
            }
            None => menu = menu.child(note(self.absent_note(ConfigKind::Model, cx), cx)),
        }
        menu = menu.child(heading(t!("chat.effort.heading"), cx));
        menu = match &effort {
            Some(effort) => {
                menu.child(self.effort_control(effort, self.menu_index == model_count, cx))
            }
            None => menu.child(note(self.absent_note(ConfigKind::Effort, cx), cx)),
        };
        Some(menu.into_any_element())
    }

    // One line instead of empty sections while the lists are not known.
    fn unavailable_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let menu = popover(
            "chat-model-menu",
            t!("chat.model.heading"),
            MODEL_MENU_WIDTH,
            cx,
        )
        .child(note(self.absent_note(ConfigKind::Model, cx), cx));
        if !self.failed_to_start() {
            return menu.into_any_element();
        }
        menu.child(rule(cx))
            .child(
                item(
                    Item {
                        id: "model-manage".into(),
                        name: t!("chat.agent.manage").into(),
                        note: None,
                        checked: false,
                        highlighted: self.menu_index == 0,
                    },
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_agent_settings(cx))),
            )
            .into_any_element()
    }

    pub(super) fn render_access_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.menu != Some(Menu::Access) {
            return None;
        }
        let mut menu = popover(
            "chat-access-menu",
            t!("chat.access.heading"),
            ACCESS_MENU_WIDTH,
            cx,
        )
        .child(heading(t!("chat.access.heading"), cx));
        for (index, access) in self.available_access().into_iter().enumerate() {
            menu = menu.child(
                item(
                    Item {
                        id: SharedString::from(format!("access-{index}")),
                        name: access.label().into(),
                        note: Some(access.note().into()),
                        checked: access == self.access,
                        highlighted: self.menu_index == index,
                    },
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_access(access, cx);
                    this.menu = None;
                    cx.notify();
                })),
            );
        }
        menu = menu
            .child(rule(cx))
            .child(note(t!("chat.access.footer"), cx));
        if self.state.select(ConfigKind::Mode).is_none() {
            menu = menu.child(note(self.absent_note(ConfigKind::Mode, cx), cx));
        }
        Some(menu.into_any_element())
    }
}
