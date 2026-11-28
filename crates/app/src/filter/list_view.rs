use std::ops::Range as IndexRange;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarHandle};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Icon, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::{ColIdx, FilterList};

use super::keys::{
    Controls, Enter, Escape, ListKey, ListMove, MenuItem, Stop, enter, escape, list_move,
    next_stop, types_into_search,
};
use super::list_state::{CheckState, ListState, Outcome, Phase};
use super::placement::{self, FULL_ROWS, LIST_TOP, ROW_HEIGHT, Side, WIDTH};
use super::texts::needs_tooltip;
use crate::actions::*;
use crate::focus_ring::{focus_ring, inset_focus_ring};
use crate::interface_scale;

const SPINNER_AFTER: Duration = Duration::from_millis(150);
const MENU_ROW: f32 = 28.0;

pub enum FilterListEvent {
    Sort { descending: bool },
    ClearColumn,
    Search { text: String, request: u64 },
    Retry { request: u64 },
    Apply(Outcome),
    Close,
}

pub struct FilterListView {
    column: ColIdx,
    name: SharedString,
    filtered: bool,
    state: ListState,
    request: u64,
    loading_since: Option<Instant>,
    stop: Stop,
    menu_item: usize,
    // 0 is "(Select All)", then the values.
    cursor: usize,
    focus: FocusHandle,
    search: Entity<InputState>,
    scroll: UniformListScrollHandle,
    anchor: Bounds<Pixels>,
    refocus: bool,
    hover_row: Option<usize>,
    tooltip_dismissed: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FilterListEvent> for FilterListView {}

impl FilterListView {
    pub fn new(
        column: ColIdx,
        name: String,
        filtered: bool,
        anchor: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FilterListView {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("filter.search")));
        let typed = cx.subscribe_in(&search, window, |this, input, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                let text = input.read(cx).value().to_string();
                this.search_changed(text, cx);
            }
        });
        let mut view = FilterListView {
            column,
            name: name.into(),
            filtered,
            state: ListState::loading(),
            request: 0,
            loading_since: None,
            stop: Stop::Search,
            menu_item: 0,
            cursor: 0,
            focus: cx.focus_handle(),
            search,
            scroll: UniformListScrollHandle::new(),
            anchor,
            refocus: true,
            hover_row: None,
            tooltip_dismissed: None,
            _subscriptions: vec![typed],
        };
        view.begin_loading(cx);
        view
    }

    pub fn search_text(&self) -> &str {
        self.state.search()
    }

    pub fn column(&self) -> ColIdx {
        self.column
    }

    pub fn request(&self) -> u64 {
        self.request
    }

    fn controls(&self) -> Controls {
        Controls::of(&self.state, self.filtered)
    }

    fn begin_loading(&mut self, cx: &mut Context<Self>) {
        self.request += 1;
        self.state.start_loading();
        self.loading_since = Some(Instant::now());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SPINNER_AFTER).await;
            if let Err(error) = this.update(cx, |_, cx| cx.notify()) {
                tracing::debug!(%error, "filter list closed before its spinner");
            }
        })
        .detach();
        cx.notify();
    }

    fn search_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if self.state.search() == text {
            return;
        }
        self.state.set_search(&text);
        self.begin_loading(cx);
        cx.emit(FilterListEvent::Search {
            text,
            request: self.request,
        });
    }

    // A result for an older request than the one on screen is dropped.
    pub fn loaded(&mut self, request: u64, result: Option<FilterList>, cx: &mut Context<Self>) {
        if request != self.request {
            return;
        }
        self.loading_since = None;
        let was_in_search = self.stop == Stop::Search;
        match result {
            Some(list) => self.state.load(list),
            None => self.state.fail(),
        }
        self.cursor = self.cursor.min(self.state.rows().len());
        let controls = self.controls();
        let retry_takes_focus = self.state.phase() == Phase::Error && !was_in_search;
        if retry_takes_focus {
            self.stop = Stop::Retry;
            self.refocus = true;
        } else if !controls.stops().contains(&self.stop) {
            self.stop = controls.first_stop();
            self.menu_item = controls.first_menu_item().unwrap_or(0);
            self.refocus = true;
        }
        cx.notify();
    }

    pub fn retry(&mut self, cx: &mut Context<Self>) {
        self.begin_loading(cx);
        cx.emit(FilterListEvent::Retry {
            request: self.request,
        });
    }

    fn stop_focus(&self, cx: &App) -> FocusHandle {
        if self.stop == Stop::Search {
            self.search.focus_handle(cx)
        } else {
            self.focus.clone()
        }
    }

    fn set_stop(&mut self, stop: Stop, window: &mut Window, cx: &mut Context<Self>) {
        self.stop = stop;
        let handle = self.stop_focus(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    fn move_cursor(&mut self, at: usize, cx: &mut Context<Self>) {
        self.cursor = at;
        self.tooltip_dismissed = None;
        self.scroll.scroll_to_item(at, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let controls = self.controls();
        match self.stop {
            Stop::Menu => {
                self.menu_item = controls.step_menu(self.menu_item, false);
                cx.notify();
            }
            Stop::Search => {
                self.menu_item = controls.last_menu_item().unwrap_or(0);
                self.set_stop(Stop::Menu, window, cx);
            }
            Stop::List => {
                let len = self.state.rows().len() + 1;
                match list_move(self.cursor, len, ListKey::Up, FULL_ROWS) {
                    ListMove::To(at) => self.move_cursor(at, cx),
                    ListMove::LeaveUp if controls.search => self.set_stop(Stop::Search, window, cx),
                    ListMove::LeaveUp => {
                        self.menu_item = controls.last_menu_item().unwrap_or(0);
                        self.set_stop(Stop::Menu, window, cx);
                    }
                }
            }
            Stop::Retry | Stop::Ok | Stop::Cancel => {}
        }
    }

    fn down(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let controls = self.controls();
        match self.stop {
            Stop::Menu => {
                self.menu_item = controls.step_menu(self.menu_item, true);
                cx.notify();
            }
            Stop::Search if controls.list => self.set_stop(Stop::List, window, cx),
            Stop::List => self.list_key(ListKey::Down, cx),
            Stop::Search | Stop::Retry | Stop::Ok | Stop::Cancel => {}
        }
    }

    fn list_key(&mut self, key: ListKey, cx: &mut Context<Self>) {
        let len = self.state.rows().len() + 1;
        if let ListMove::To(at) = list_move(self.cursor, len, key, FULL_ROWS) {
            self.move_cursor(at, cx);
        }
    }

    fn toggle_at(&mut self, index: usize, cx: &mut Context<Self>) {
        self.state.toggle_entry(index);
        cx.notify();
    }

    fn run_item(&mut self, item: MenuItem, cx: &mut Context<Self>) {
        cx.emit(match item {
            MenuItem::SortAscending => FilterListEvent::Sort { descending: false },
            MenuItem::SortDescending => FilterListEvent::Sort { descending: true },
            MenuItem::ClearFilter => FilterListEvent::ClearColumn,
        });
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        match self.state.outcome() {
            Outcome::Unchanged => cx.emit(FilterListEvent::Close),
            outcome => cx.emit(FilterListEvent::Apply(outcome)),
        }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        match enter(self.stop, self.menu_item, &self.controls()) {
            Enter::RunItem(item) => self.run_item(item, cx),
            Enter::Apply => self.apply(cx),
            Enter::Retry => self.retry(cx),
            Enter::Close => cx.emit(FilterListEvent::Close),
            Enter::Nothing => {}
        }
    }

    fn step_stop(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let controls = self.controls();
        let stop = next_stop(&controls.stops(), self.stop, backwards);
        if stop == Stop::Menu {
            self.menu_item = controls.first_menu_item().unwrap_or(0);
        }
        self.set_stop(stop, window, cx);
    }

    fn visible_tooltip(&self) -> Option<usize> {
        let row = self
            .hover_row
            .or((self.stop == Stop::List).then_some(self.cursor))?;
        let text = &self.state.rows().get(row.checked_sub(1)?)?.text;
        (needs_tooltip(text) && self.tooltip_dismissed != Some(row)).then_some(row)
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let search_empty = !self.state.searching();
        match escape(self.visible_tooltip().is_some(), self.stop, search_empty) {
            Escape::DismissTooltip => {
                self.tooltip_dismissed = self.visible_tooltip();
                cx.notify();
            }
            Escape::ClearSearch => {
                self.search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.search_changed(String::new(), cx);
            }
            Escape::Close => cx.emit(FilterListEvent::Close),
        }
    }

    fn type_into_search(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if !types_into_search(self.stop)
            || modifiers.control
            || modifiers.alt
            || modifiers.platform
            || modifiers.function
        {
            return;
        }
        let Some(typed) = keystroke.key_char.as_ref().filter(|typed| {
            !typed.is_empty() && !typed.chars().any(char::is_control) && typed.as_str() != " "
        }) else {
            return;
        };
        let text = format!("{}{typed}", self.state.search());
        self.search
            .update(cx, |input, cx| input.set_value(text.clone(), window, cx));
        self.set_stop(Stop::Search, window, cx);
        self.search_changed(text, cx);
        cx.stop_propagation();
    }

    fn check_box(&self, state: CheckState, cx: &App) -> Div {
        let theme = cx.theme();
        let filled = state != CheckState::Unchecked;
        let mark = match state {
            CheckState::Checked => Some(IconName::Check),
            CheckState::Mixed => Some(IconName::Minus),
            CheckState::Unchecked => None,
        };
        div()
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .size(px(14.0))
            .rounded(px(3.0))
            .border_2()
            .border_color(if filled {
                theme.primary
            } else {
                theme.muted_foreground
            })
            .when(filled, |this| {
                this.bg(theme.primary).text_color(theme.primary_foreground)
            })
            .children(mark.map(|icon| Icon::new(icon).size_2p5()))
    }

    fn row_label(&self, index: usize) -> SharedString {
        match index
            .checked_sub(1)
            .and_then(|row| self.state.rows().get(row))
        {
            None if self.state.searching() => t!("filter.select_all_results").into(),
            None => t!("filter.select_all").into(),
            Some(row) if row.blank => t!("filter.blanks").into(),
            Some(row) => row.text.clone().into(),
        }
    }

    fn row_check(&self, index: usize) -> CheckState {
        match index
            .checked_sub(1)
            .and_then(|row| self.state.rows().get(row))
        {
            None => self.state.select_all_state(),
            Some(row) if row.checked => CheckState::Checked,
            Some(_) => CheckState::Unchecked,
        }
    }

    fn render_rows(&self, range: IndexRange<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let scale = interface_scale::shown(cx).factor();
        let (hover, ring_rows) = {
            let theme = cx.theme();
            (theme.accent, inset_focus_ring(theme))
        };
        range
            .map(|index| {
                let label = self.row_label(index);
                let check = self.row_check(index);
                let spoken = match check {
                    CheckState::Checked => t!("filter.aria_checked", value = label),
                    CheckState::Unchecked => t!("filter.aria_unchecked", value = label),
                    CheckState::Mixed => t!("filter.aria_mixed", value = label),
                };
                let focused = self.stop == Stop::List && self.cursor == index;
                h_flex()
                    .id(("filter-row", index))
                    .role(Role::ListBoxOption)
                    .aria_label(spoken)
                    .h(px(ROW_HEIGHT * scale))
                    .px(px(10.0))
                    .gap_2()
                    .items_center()
                    .cursor_pointer()
                    .hover(move |row| row.bg(hover))
                    .when(focused, |row| row.shadow(ring_rows.clone()))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.hover_row = Some(index);
                        } else if this.hover_row == Some(index) {
                            this.hover_row = None;
                        }
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.cursor = index;
                        this.set_stop(Stop::List, window, cx);
                        this.toggle_at(index, cx);
                    }))
                    .child(self.check_box(check, cx))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .into_any_element()
            })
            .collect()
    }

    fn menu_row(
        &self,
        index: usize,
        icon: IconName,
        label: SharedString,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (hover, ring) = {
            let theme = cx.theme();
            (theme.accent, focus_ring(theme))
        };
        let enabled = controls.menu[index];
        let focused = enabled && self.stop == Stop::Menu && self.menu_item == index;
        h_flex()
            .id(("filter-menu", index))
            .role(Role::Button)
            .aria_label(label.clone())
            .h(rem_row(MENU_ROW, cx))
            .mx(px(7.0))
            .px(px(8.0))
            .gap_2()
            .items_center()
            .rounded_md()
            .when(!enabled, |row| row.opacity(0.5))
            .when(enabled, |row| {
                row.cursor_pointer()
                    .hover(move |row| row.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_item = index;
                        if let Some(item) = MenuItem::at(index) {
                            this.run_item(item, cx);
                        }
                    }))
            })
            .when(focused, |row| row.shadow(ring))
            .child(Icon::new(icon).size_4())
            .child(label)
            .into_any_element()
    }

    fn separator(&self, cx: &App) -> AnyElement {
        div()
            .h(px(1.0))
            .my(px(4.0))
            .bg(cx.theme().border)
            .into_any_element()
    }

    fn render_search(&self, controls: &Controls, cx: &App) -> AnyElement {
        let ring = focus_ring(cx.theme());
        let focused = self.stop == Stop::Search;
        div()
            .mx(px(8.0))
            .my(px(6.0))
            .rounded_md()
            .when(!controls.search, |this| this.opacity(0.5))
            .when(focused, |this| this.shadow(ring))
            .child(
                Input::new(&self.search)
                    .small()
                    .cleanable(false)
                    .focus_bordered(false)
                    .disabled(!controls.search),
            )
            .into_any_element()
    }

    fn render_body(&self, scale: f32, rows: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let frame = div()
            .id("filter-list")
            .role(Role::ListBox)
            .aria_label(t!("filter.aria_list", column = self.name))
            .relative()
            .mx(px(8.0))
            .h(px(rows as f32 * ROW_HEIGHT * scale + 2.0))
            .border_1()
            .border_color(border)
            .rounded_md()
            .overflow_hidden();
        let phase = self.state.phase();
        let spinner_due = self
            .loading_since
            .is_some_and(|since| since.elapsed() >= SPINNER_AFTER);
        let message = |text: SharedString| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(muted)
                .child(text)
        };
        match phase {
            Phase::Loading if spinner_due => {
                let indicator = if cx.reduce_motion() {
                    div().into_any_element()
                } else {
                    Spinner::new().color(cx.theme().primary).into_any_element()
                };
                frame
                    .child(
                        message(t!("filter.loading").into())
                            .gap_2()
                            .child(indicator),
                    )
                    .into_any_element()
            }
            Phase::Loading if self.state.rows().is_empty() => frame.into_any_element(),
            Phase::Error => frame
                .child(
                    v_flex()
                        .size_full()
                        .p_3()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t!("filter.error_title")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t!("filter.error_body", column = self.name)),
                        )
                        .child(self.render_retry(cx)),
                )
                .into_any_element(),
            Phase::Foreign => frame
                .child(
                    v_flex()
                        .size_full()
                        .p_3()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t!("filter.foreign_title")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t!("filter.foreign_body")),
                        ),
                )
                .into_any_element(),
            Phase::Ready if self.state.shows_no_matches() => frame
                .child(message(t!("filter.no_matches").into()))
                .into_any_element(),
            Phase::Ready | Phase::Loading => {
                let count = self.state.rows().len() + 1;
                frame
                    .child(
                        uniform_list(
                            "filter-rows",
                            count,
                            cx.processor(
                                |this: &mut FilterListView, range: IndexRange<usize>, _, cx| {
                                    this.render_rows(range, cx)
                                },
                            ),
                        )
                        .track_scroll(&self.scroll)
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .w(px(10.0))
                            .child(Scrollbar::vertical(&self.scroll)),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_retry(&self, cx: &mut Context<Self>) -> AnyElement {
        let (ring, secondary, secondary_foreground, border) = {
            let theme = cx.theme();
            (
                focus_ring(theme),
                theme.secondary,
                theme.secondary_foreground,
                theme.border,
            )
        };
        div()
            .id("filter-retry")
            .role(Role::Button)
            .flex()
            .items_center()
            .justify_center()
            .h(px(MENU_ROW))
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(secondary)
            .text_color(secondary_foreground)
            .cursor_pointer()
            .when(self.stop == Stop::Retry, |this| this.shadow(ring))
            .on_click(cx.listener(|this, _, _, cx| this.retry(cx)))
            .child(t!("filter.retry"))
            .into_any_element()
    }

    fn footer_button(
        &self,
        id: &'static str,
        label: SharedString,
        primary: bool,
        enabled: bool,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (ring, face, text, border) = {
            let theme = cx.theme();
            if primary {
                (
                    focus_ring(theme),
                    theme.primary,
                    theme.primary_foreground,
                    theme.primary,
                )
            } else {
                (
                    focus_ring(theme),
                    theme.secondary,
                    theme.secondary_foreground,
                    theme.border,
                )
            }
        };
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label.clone())
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .h(px(MENU_ROW))
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(face)
            .text_color(text)
            .when(primary, |this| this.font_weight(FontWeight::SEMIBOLD))
            .when(!enabled, |this| this.opacity(0.5))
            .when(enabled, |this| this.cursor_pointer())
            .when(focused && enabled, |this| this.shadow(ring))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !enabled {
                    return;
                }
                if primary {
                    this.apply(cx);
                } else {
                    cx.emit(FilterListEvent::Close);
                }
            }))
            .child(label)
            .into_any_element()
    }
}

fn rem_row(pixels: f32, cx: &App) -> Pixels {
    px(pixels * interface_scale::shown(cx).factor())
}

impl Render for FilterListView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.refocus) {
            let handle = self.stop_focus(cx);
            window.defer(cx, move |window, cx| window.focus(&handle, cx));
        }
        let scale = interface_scale::shown(cx).factor();
        let controls = self.controls();
        let anchor = self.anchor;
        let placed = placement::place(
            f32::from(anchor.top()),
            f32::from(anchor.bottom()),
            f32::from(window.viewport_size().height),
            scale,
        );
        let mut context = KeyContext::default();
        context.add("FilterList");
        if self.stop == Stop::List {
            context.add("FilterRows");
        }
        let (popover, popover_foreground, border, muted) = {
            let theme = cx.theme();
            (
                theme.popover,
                theme.popover_foreground,
                theme.border,
                theme.muted_foreground,
            )
        };
        let sort_up = self.menu_row(
            0,
            IconName::ArrowDownAZ,
            t!("menu.sort_ascending").into(),
            &controls,
            cx,
        );
        let sort_down = self.menu_row(
            1,
            IconName::ArrowDownZA,
            t!("menu.sort_descending").into(),
            &controls,
            cx,
        );
        let clear = self.menu_row(
            2,
            IconName::X,
            t!("filter.clear_column", column = self.name).into(),
            &controls,
            cx,
        );
        let search = self.render_search(&controls, cx);
        let body = self.render_body(scale, placed.rows, cx);
        let ok = self.footer_button(
            "filter-ok",
            t!("filter.ok").into(),
            true,
            controls.ok,
            self.stop == Stop::Ok,
            cx,
        );
        let cancel = self.footer_button(
            "filter-cancel",
            t!("button.cancel").into(),
            false,
            true,
            self.stop == Stop::Cancel,
            cx,
        );
        let separator_a = self.separator(cx);
        let separator_b = self.separator(cx);
        let surface = v_flex()
            .id("filter-popover")
            .role(Role::Dialog)
            .aria_label(t!("filter.aria_list", column = self.name))
            .key_context(context)
            .track_focus(&self.focus)
            .occlude()
            .w(px(WIDTH * scale))
            .py(px(4.0))
            .bg(popover)
            .text_color(popover_foreground)
            .text_sm()
            .border_1()
            .border_color(border)
            .rounded_lg()
            .shadow_lg()
            .on_action(cx.listener(|this, _: &CloseFilterList, window, cx| this.escape(window, cx)))
            .on_action(cx.listener(|this, _: &FilterListUp, window, cx| this.up(window, cx)))
            .on_action(cx.listener(|this, _: &FilterListDown, window, cx| this.down(window, cx)))
            .on_action(
                cx.listener(|this, _: &FilterListHome, _, cx| this.list_key(ListKey::Home, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FilterListEnd, _, cx| this.list_key(ListKey::End, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FilterListPageUp, _, cx| this.list_key(ListKey::PageUp, cx)),
            )
            .on_action(cx.listener(|this, _: &FilterListPageDown, _, cx| {
                this.list_key(ListKey::PageDown, cx)
            }))
            .on_action(cx.listener(|this, _: &FilterListToggle, _, cx| {
                let at = this.cursor;
                this.toggle_at(at, cx);
            }))
            .on_action(cx.listener(|this, _: &FilterListConfirm, _, cx| this.confirm(cx)))
            .on_action(cx.listener(|this, _: &FilterListNextStop, window, cx| {
                this.step_stop(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FilterListPreviousStop, window, cx| {
                this.step_stop(true, window, cx)
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.type_into_search(event, window, cx)
            }))
            .child(sort_up)
            .child(sort_down)
            .child(separator_a)
            .child(clear)
            .child(separator_b)
            .child(search)
            .child(body)
            .child(
                div()
                    .h(px(20.0 * scale))
                    .mx(px(8.0))
                    .px(px(2.0))
                    .pt(px(2.0))
                    .text_xs()
                    .text_color(muted)
                    .when(self.state.truncated(), |note| {
                        note.child(t!("filter.not_all_shown"))
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .px(px(8.0))
                    .pt(px(4.0))
                    .child(ok)
                    .child(cancel),
            );
        let right = anchor.right();
        let (position, corner, offset) = match placed.side {
            Side::Below => (
                point(right, anchor.bottom()),
                Anchor::TopRight,
                point(px(0.0), px(2.0)),
            ),
            Side::Above => (
                point(right, anchor.top()),
                Anchor::BottomRight,
                point(px(0.0), px(-2.0)),
            ),
        };
        let popover_top = match placed.side {
            Side::Below => f32::from(anchor.bottom()) + 2.0,
            Side::Above => f32::from(anchor.top()) - 2.0 - placed.height(scale),
        };
        let tip = self.visible_tooltip().and_then(|row| {
            let text = self.state.rows().get(row.checked_sub(1)?)?.text.clone();
            let scrolled = f32::from(self.scroll.offset().y);
            let top = popover_top + (LIST_TOP + row as f32 * ROW_HEIGHT) * scale + scrolled;
            let left = f32::from(right) - WIDTH * scale - 4.0;
            Some((text, point(px(left), px(top))))
        });
        let viewport = window.viewport_size();
        let backdrop = div()
            .occlude()
            .w(viewport.width)
            .h(viewport.height)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(FilterListEvent::Close)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| cx.emit(FilterListEvent::Close)),
            );
        div()
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopLeft)
                        .position(point(px(0.0), px(0.0)))
                        .child(backdrop),
                )
                .priority(1),
            )
            .child(
                deferred(
                    anchored()
                        .anchor(corner)
                        .position(position)
                        .offset(offset)
                        .snap_to_window_with_margin(px(4.0))
                        .child(surface),
                )
                .priority(2),
            )
            .children(tip.map(|(text, at)| {
                deferred(
                    anchored()
                        .anchor(Anchor::TopRight)
                        .position(at)
                        .snap_to_window_with_margin(px(4.0))
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .max_w(px(360.0))
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .bg(popover)
                                .text_color(popover_foreground)
                                .text_xs()
                                .shadow_md()
                                .child(text),
                        ),
                )
                .priority(3)
            }))
    }
}
