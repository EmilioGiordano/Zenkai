use std::collections::{HashMap, HashSet};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_agent::chat::thread::{Entry, ToolCallId, ToolCard, ToolStatus};
use zenkai_agent::chat::tool_rows::{self, RowText};
use zenkai_agent::settings::ToolCallDisplay;
use zenkai_agent::tools::escaped_multiline;
use zenkai_i18n::t;

use super::{ChatPanel, MONO};
use crate::actions::*;

const SHORTEST_DURATION_SHOWN: std::time::Duration = std::time::Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum RowKey {
    Call(ToolCallId),
    Group(ToolCallId),
}

impl RowKey {
    fn element_id(&self) -> SharedString {
        match self {
            RowKey::Call(id) => SharedString::from(format!("tool-call-{}", id.as_str())),
            RowKey::Group(id) => SharedString::from(format!("tool-group-{}", id.as_str())),
        }
    }
}

#[derive(Default)]
pub(super) struct ToolViews {
    toggled: HashSet<RowKey>,
    focus: HashMap<RowKey, FocusHandle>,
    inputs: HashSet<ToolCallId>,
}

impl ToolViews {
    fn is_open(&self, display: ToolCallDisplay, id: &RowKey) -> bool {
        (display == ToolCallDisplay::Expanded) != self.toggled.contains(id)
    }

    pub(super) fn input_open(&self, id: &ToolCallId) -> bool {
        self.inputs.contains(id)
    }

    pub(super) fn toggle_input(&mut self, id: &ToolCallId) {
        if !self.inputs.remove(id) {
            self.inputs.insert(id.clone());
        }
    }

    fn toggle(&mut self, id: &RowKey) {
        if !self.toggled.remove(id) {
            self.toggled.insert(id.clone());
        }
    }

    fn handle(&mut self, id: &RowKey, cx: &Context<ChatPanel>) -> FocusHandle {
        self.focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    fn existing(&self, id: &ToolCallId) -> Option<&FocusHandle> {
        self.focus.get(&RowKey::Call(id.clone()))
    }

    pub(super) fn keep_only(&mut self, calls: &HashSet<ToolCallId>, groups: &HashSet<ToolCallId>) {
        let live = |key: &RowKey| match key {
            RowKey::Call(id) => calls.contains(id),
            RowKey::Group(id) => groups.contains(id),
        };
        self.toggled.retain(|key| live(key));
        self.focus.retain(|key, _| live(key));
        self.inputs.retain(|id| calls.contains(id));
    }
}

fn status_mark(status: ToolStatus, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mark = match status {
        ToolStatus::Pending | ToolStatus::InProgress => Spinner::new()
            .color(theme.muted_foreground)
            .into_any_element(),
        ToolStatus::WaitingForPermission => Icon::new(IconName::Shield)
            .size_4()
            .text_color(theme.success)
            .into_any_element(),
        ToolStatus::Completed => Icon::new(IconName::CircleCheck)
            .size_4()
            .text_color(theme.success)
            .into_any_element(),
        ToolStatus::Failed => Icon::new(IconName::TriangleAlert)
            .size_4()
            .text_color(theme.danger)
            .into_any_element(),
        ToolStatus::Rejected => Icon::new(IconName::CircleX)
            .size_4()
            .text_color(theme.danger)
            .into_any_element(),
        ToolStatus::Canceled => Icon::new(IconName::Square)
            .size_4()
            .text_color(theme.muted_foreground)
            .into_any_element(),
    };
    div()
        .w_4()
        .flex_shrink_0()
        .flex()
        .justify_center()
        .child(mark)
        .into_any_element()
}

fn meta_text(card: &ToolCard) -> String {
    match card.status {
        ToolStatus::Completed => card
            .took
            .filter(|took| *took >= SHORTEST_DURATION_SHOWN)
            .map(tool_rows::took_label)
            .unwrap_or_default(),
        status => status.label().to_string(),
    }
}

struct Row<'a> {
    key: &'a RowKey,
    mark: AnyElement,
    text: &'a RowText,
    meta: String,
    open: bool,
    aria: String,
}

impl ChatPanel {
    pub(super) fn toggle_tool(&mut self, id: &RowKey, cx: &mut Context<Self>) {
        self.tools.toggle(id);
        cx.notify();
    }

    pub(super) fn copy_tool_result(&self, id: &ToolCallId, cx: &mut Context<Self>) {
        if let Some(card) = self.thread.tool(id).filter(|card| !card.detail.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(escaped_multiline(&card.detail)));
        }
    }

    pub(super) fn focus_tool_calls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let latest = self
            .thread
            .entries()
            .iter()
            .rev()
            .find_map(|entry| match entry {
                Entry::Tool(card) => self.tools.existing(&card.id).cloned(),
                _ => None,
            });
        if let Some(handle) = latest {
            window.focus(&handle, cx);
        }
    }

    pub(super) fn cycle_tool_call_display(&mut self, cx: &mut Context<Self>) {
        let next = cx
            .global::<crate::agent_settings::AgentConfig>()
            .state
            .current
            .agents
            .tool_calls
            .next();
        crate::agent_settings::change(cx, move |settings| settings.agents.tool_calls = next);
    }

    fn render_row(&self, views: &mut ToolViews, row: Row, cx: &mut Context<Self>) -> Stateful<Div> {
        let focus = views.handle(row.key, cx);
        let theme = cx.theme();
        let (foreground, muted, ring, hover) = (
            theme.foreground,
            theme.muted_foreground,
            theme.ring,
            theme.secondary,
        );
        let (on_click, on_toggle, on_copy) = (row.key.clone(), row.key.clone(), row.key.clone());
        let clicked_focus = focus.clone();
        h_flex()
            .id(row.key.element_id())
            .key_context("ToolRow")
            .track_focus(&focus)
            .tab_stop(true)
            .role(Role::Button)
            .aria_label(row.aria)
            .aria_expanded(row.open)
            .gap_2()
            .items_center()
            .px_1()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(gpui_kit::transparent_black())
            .focus_visible(|style| style.border_color(ring))
            .hover(|style| style.bg(hover))
            .cursor_pointer()
            .text_sm()
            .child(row.mark)
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(foreground)
                    .child(row.text.label.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(MONO)
                    .text_xs()
                    .text_color(muted)
                    .child(row.text.target.clone()),
            )
            .when(!row.meta.is_empty(), |line| {
                line.child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(muted)
                        .child(row.meta),
                )
            })
            .child(
                Icon::new(if row.open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size_3()
                .flex_shrink_0()
                .text_color(muted),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&clicked_focus, cx);
                this.toggle_tool(&on_click, cx);
            }))
            .on_action(
                cx.listener(move |this, _: &ToggleToolCall, _, cx| {
                    this.toggle_tool(&on_toggle, cx)
                }),
            )
            .on_action(cx.listener(move |this, _: &CopyToolResult, _, cx| {
                if let RowKey::Call(id) = &on_copy {
                    this.copy_tool_result(id, cx)
                }
            }))
            .on_action(|_: &FocusNextToolCall, window, cx| window.focus_next(cx))
            .on_action(|_: &FocusPreviousToolCall, window, cx| window.focus_prev(cx))
    }

    pub(super) fn render_call(
        &self,
        views: &mut ToolViews,
        display: ToolCallDisplay,
        card: &ToolCard,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if card.status == ToolStatus::WaitingForPermission {
            return self.render_ask(views, card, cx);
        }
        let text = tool_rows::row_text(card);
        let key = RowKey::Call(card.id.clone());
        let open = views.is_open(display, &key);
        let aria = format!("{}: {} {}", card.status.label(), text.label, text.target);
        let mark = status_mark(card.status, cx);
        let meta = meta_text(card);
        let row = self.render_row(
            views,
            Row {
                key: &key,
                mark,
                text: &text,
                meta,
                open,
                aria,
            },
            cx,
        );
        v_flex()
            .child(row)
            .when(open, |call| {
                call.child(self.render_call_body(card, &text, cx))
            })
            .into_any_element()
    }

    pub(super) fn render_group(
        &self,
        views: &mut ToolViews,
        first: &ToolCard,
        cards: &[&ToolCard],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = tool_rows::group_text(cards);
        let key = RowKey::Group(first.id.clone());
        let open = views.is_open(ToolCallDisplay::Collapsed, &key);
        let aria = format!("{} {}", text.label, text.target);
        let mark = status_mark(ToolStatus::Completed, cx);
        let row = self.render_row(
            views,
            Row {
                key: &key,
                mark,
                text: &text,
                meta: String::new(),
                open,
                aria,
            },
            cx,
        );
        let members: Vec<AnyElement> = if open {
            cards
                .iter()
                .map(|card| self.render_call(views, ToolCallDisplay::Collapsed, card, cx))
                .collect()
        } else {
            Vec::new()
        };
        v_flex()
            .child(row)
            .when(open, |group| group.child(v_flex().pl_4().children(members)))
            .into_any_element()
    }

    pub(super) fn render_used(&self, count: usize, cx: &App) -> AnyElement {
        div()
            .px_1()
            .py_1()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(tool_rows::used_label(count))
            .into_any_element()
    }

    fn render_call_body(
        &self,
        card: &ToolCard,
        text: &RowText,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let failed = card.status == ToolStatus::Failed;
        let section = move |label: &'static str, body: String| {
            v_flex()
                .gap_1()
                .child(div().text_xs().text_color(muted).child(label))
                .child(
                    div()
                        .font_family(MONO)
                        .text_xs()
                        .line_height(relative(1.5))
                        .p_2()
                        .rounded_md()
                        .bg(theme.muted)
                        .child(body),
                )
        };
        let preview = tool_rows::result_preview(&card.detail);
        let reason = tool_rows::body_preview(&card.detail);
        let input = tool_rows::body_preview(&tool_rows::pretty_input(&card.input));
        let more_chars = move |count: usize| {
            div()
                .text_xs()
                .text_color(muted)
                .child(t!("chat.tool.more_chars", count = count))
        };
        let can_allow_editing = failed
            && self.editing_blocked.as_ref() == Some(&card.id)
            && zenkai_agent::tools::is_protected_view_refusal(&card.detail);
        let copy_id = card.id.clone();
        v_flex()
            .gap_2()
            .mt_1()
            .p_3()
            .rounded_lg()
            .bg(theme.background)
            .when(failed, |body| {
                body.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_color(theme.danger)
                                        .child(tool_rows::failed_label(&text.label)),
                                )
                                .child(
                                    div()
                                        .font_family(MONO)
                                        .text_xs()
                                        .text_color(muted)
                                        .child(text.target.clone()),
                                ),
                        )
                        .child(
                            div()
                                .line_height(relative(1.45))
                                .child(if card.detail.is_empty() {
                                    t!("chat.tool.no_reason").to_string()
                                } else {
                                    reason.shown.clone()
                                }),
                        )
                        .when(reason.more_chars > 0, |reason_view| {
                            reason_view.child(more_chars(reason.more_chars))
                        }),
                )
            })
            .when(!card.input.is_empty(), |body| {
                body.child(section(t!("chat.tool.input"), input.shown.clone()))
                    .when(input.more_chars > 0, |body| {
                        body.child(more_chars(input.more_chars))
                    })
            })
            .when(!failed && !card.detail.is_empty(), |body| {
                body.child(section(t!("chat.tool.result"), preview.shown.clone()))
                    .when(preview.more_lines > 0, |body| {
                        body.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t!("chat.tool.more_lines", count = preview.more_lines)),
                        )
                    })
            })
            .when(!card.detail.is_empty(), |body| {
                body.child(
                    h_flex().child(
                        Button::new(SharedString::from(format!(
                            "copy-result-{}",
                            card.id.as_str()
                        )))
                        .ghost()
                        .small()
                        .label(t!("chat.tool.copy_result"))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.copy_tool_result(&copy_id, cx)),
                        ),
                    ),
                )
            })
            .when(can_allow_editing, |body| {
                body.child(
                    h_flex().child(
                        Button::new(SharedString::from(format!(
                            "let-agents-edit-{}",
                            card.id.as_str()
                        )))
                        .outline()
                        .small()
                        .label(t!("chat.tool.let_agents_edit"))
                        .on_click(cx.listener(|this, _, _, cx| this.allow_agent_editing(cx))),
                    ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use zenkai_agent::chat::thread::ToolCallId;

    use super::RowKey;

    #[test]
    fn a_call_and_a_group_row_never_share_a_key() {
        let call = RowKey::Call(ToolCallId::new("x"));
        let group = RowKey::Group(ToolCallId::new("x"));
        let lookalike = RowKey::Call(ToolCallId::new("group:x"));
        assert_ne!(call, group);
        assert_ne!(group, lookalike);
        assert_ne!(group.element_id(), lookalike.element_id());
        assert_ne!(group.element_id(), call.element_id());
    }
}
