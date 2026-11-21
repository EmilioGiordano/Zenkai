mod links;
mod plan;

use std::sync::Arc;

use gpui_kit::base::{TextView, h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_i18n::t;

use crate::actions::{CheckForUpdates, UpdateNow};
use crate::interface_scale::rem_px;
use crate::keymap;
use crate::markdown_pane::zoom_style::zoomed;
use crate::markdown_pane::{blocked_image, restricted_extensions};
use crate::text_zoom::TextZoom;
use crate::updates::{self, Notes, RELEASES_PAGE, Status, Updates, Version};
use links::{NoteLink, Target};
use plan::{Body, Line, Plan, SectionPlan};

pub const CONTEXT: &str = "WhatsNew";

const COLUMN_WIDTH: f32 = 640.0;
const TITLE_SIZE: f32 = 20.0;
const VERSION_SIZE: f32 = 15.0;
const SCROLL_LINE: f32 = 40.0;

pub enum NotesEvent {
    OpenLink(String),
}

struct Shown {
    plan: Plan,
    bodies: Vec<Option<Entity<TextViewState>>>,
}

// The What's new tab: the notes the update check fetched, read-only. Each version is a heading
// the tab draws and a body the Markdown text view draws in its restricted mode.
pub struct NotesView {
    shown: Option<Shown>,
    seen: Option<(Arc<Notes>, Status)>,
    zoom: TextZoom,
    focus: FocusHandle,
    scroll: ScrollHandle,
    refresh_task: Option<Task<()>>,
    _updates: Subscription,
}

impl EventEmitter<NotesEvent> for NotesView {}

impl NotesView {
    pub fn new(cx: &mut Context<Self>) -> NotesView {
        let updates = cx.observe_global::<Updates>(|this, cx| this.refresh(cx));
        let mut view = NotesView {
            shown: None,
            seen: None,
            zoom: TextZoom::default(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            refresh_task: None,
            _updates: updates,
        };
        view.refresh(cx);
        view
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    pub fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.zoom = self.zoom.larger();
        cx.notify();
    }

    pub fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.zoom = self.zoom.smaller();
        cx.notify();
    }

    pub fn zoom_reset(&mut self, cx: &mut Context<Self>) {
        self.zoom = TextZoom::default();
        cx.notify();
    }

    // Every change to the update state lands here; only a new status or new notes cost work,
    // and that work runs in the background.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let status = updates::status(cx);
        let notes = updates::notes(cx);
        let unchanged = self
            .seen
            .as_ref()
            .is_some_and(|(seen, was)| Arc::ptr_eq(seen, &notes) && *was == status);
        if unchanged {
            return;
        }
        let Some(installed) = Version::installed() else {
            tracing::debug!("the What's new tab needs the installed version");
            return;
        };
        self.seen = Some((notes.clone(), status));
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let plan = cx
                .background_executor()
                .spawn(async move { plan::plan(status, &notes, installed) })
                .await;
            if let Err(error) = this.update(cx, |this, cx| this.show(plan, cx)) {
                tracing::debug!(%error, "the What's new tab closed before its notes were ready");
            }
        }));
    }

    fn show(&mut self, plan: Plan, cx: &mut Context<Self>) {
        let bodies = match &plan.body {
            Body::Sections { sections, .. } => sections
                .iter()
                .map(|section| {
                    section
                        .text
                        .as_ref()
                        .map(|text| cx.new(|cx| TextViewState::markdown(text, cx)))
                })
                .collect(),
            Body::UpToDate | Body::NoNotes | Body::Nothing => Vec::new(),
        };
        self.shown = Some(Shown { plan, bodies });
        cx.notify();
    }

    // The arrow, page and home keys scroll the notes wherever the focus is inside the tab.
    fn scroll_by_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.shift {
            return;
        }
        let offset = self.scroll.offset();
        let furthest = self.scroll.max_offset().y;
        let page = self.scroll.bounds().size.height;
        let line = px(SCROLL_LINE);
        let target = match event.keystroke.key.as_str() {
            "down" => offset.y - line,
            "up" => offset.y + line,
            "pagedown" => offset.y - page,
            "pageup" => offset.y + page,
            "home" => px(0.),
            "end" => -furthest,
            _ => return,
        };
        self.scroll
            .set_offset(point(offset.x, target.max(-furthest).min(px(0.))));
        cx.stop_propagation();
        cx.notify();
    }

    fn render_banner(&self, version: Version, cx: &mut Context<Self>) -> impl IntoElement {
        let (border, background) = {
            let theme = cx.theme();
            (theme.border, theme.secondary)
        };
        h_flex()
            .w_full()
            .px_4()
            .py_2()
            .gap_4()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(border)
            .bg(background)
            .child(div().min_w_0().child(t!(
                "update.available",
                version = version,
                installed = env!("CARGO_PKG_VERSION")
            )))
            .child(
                h_flex()
                    .flex_none()
                    .gap_3()
                    .items_center()
                    .child(
                        Button::new("whats-new-github")
                            .link()
                            .label(t!("whats_new.view_on_github"))
                            .on_click(|_, _, cx| cx.open_url(RELEASES_PAGE)),
                    )
                    .child(
                        Button::new("whats-new-update")
                            .primary()
                            .small()
                            .label(t!("update.now"))
                            .tooltip(keymap::labeled(cx, t!("update.now"), &UpdateNow))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(UpdateNow), cx)
                            }),
                    ),
            )
    }

    fn render_line(&self, line: Line, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (mark, text, color) = match line {
            Line::Checking => ("", t!("whats_new.checking"), theme.muted_foreground),
            Line::CheckFailed => ("\u{26a0}", t!("whats_new.check_failed"), theme.warning),
        };
        h_flex()
            .gap_2()
            .items_start()
            .text_color(color)
            .child(div().flex_none().child(mark))
            .child(div().min_w_0().child(text))
    }

    fn render_message(&self, text: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .items_center()
            .py_8()
            .child(div().text_color(muted).child(text))
            .child(
                Button::new("whats-new-check")
                    .small()
                    .label(t!("settings.update.check_button"))
                    .tooltip(keymap::labeled(
                        cx,
                        t!("palette.check_for_updates"),
                        &CheckForUpdates,
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(CheckForUpdates), cx)
                    }),
            )
    }

    fn render_section(
        &self,
        index: usize,
        section: &SectionPlan,
        body: Option<&Entity<TextViewState>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let factor = self.zoom.factor();
        let muted = cx.theme().muted_foreground;
        let style = (!self.zoom.is_default()).then(|| zoomed(cx.theme(), factor));
        let weak = cx.entity().downgrade();
        let text = match body {
            Some(state) => TextView::new(state)
                .when_some(style, |view, style| view.style(style))
                .markdown_extensions(restricted_extensions())
                .selectable(true)
                .image_source(|_| blocked_image())
                .on_link_click(move |url, _, _, cx| {
                    let target = url.to_string();
                    if let Err(error) =
                        weak.update(cx, |_, cx| cx.emit(NotesEvent::OpenLink(target)))
                    {
                        tracing::debug!(%error, "the What's new tab closed on a link click");
                    }
                })
                .into_any_element(),
            None => div()
                .text_color(muted)
                .child(t!("whats_new.not_shown"))
                .into_any_element(),
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_baseline()
                    .child(
                        div()
                            .text_size(rem_px(VERSION_SIZE * factor))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(section.version.to_string()),
                    )
                    .children(section.published.map(|day| {
                        div()
                            .text_size(rem_px(VERSION_SIZE * factor))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(muted)
                            .child(day.to_string())
                    })),
            )
            .child(text)
            .child(self.render_links(index, section, cx))
    }

    // Every link of a version as a button, so each one has a Tab stop and a focus ring.
    fn render_links(
        &self,
        index: usize,
        section: &SectionPlan,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(("whats-new-links", index))
            .aria_label(t!("whats_new.links", version = section.version))
            .flex_wrap()
            .gap_x_2()
            .children(section.links.iter().enumerate().map(|(position, link)| {
                let address = link.url.clone();
                Button::new(("whats-new-link", index * 100 + position))
                    .link()
                    .small()
                    .label(label_of(link))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(NotesEvent::OpenLink(address.clone()))
                    }))
            }))
    }

    fn render_older(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_1()
            .child(div().text_color(muted).child(t!("whats_new.older")))
            .child(
                h_flex().child(
                    Button::new("whats-new-all")
                        .link()
                        .small()
                        .label(t!("whats_new.all_releases"))
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(NotesEvent::OpenLink(RELEASES_PAGE.to_owned()))
                        })),
                ),
            )
    }
}

fn label_of(link: &NoteLink) -> String {
    match &link.target {
        Target::Release(version) => t!("whats_new.link.release", version = version),
        Target::Issue(number) => t!("whats_new.link.issue", number = number),
        Target::Pull(number) => t!("whats_new.link.pull", number = number),
        Target::Other => link.text.clone(),
    }
}

impl Render for NotesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (background, ring) = {
            let theme = cx.theme();
            (theme.background, theme.ring)
        };
        let ring = if self.focus.is_focused(window) {
            ring
        } else {
            background
        };
        let factor = self.zoom.factor();
        let mut banner = None;
        let mut column = v_flex()
            .w_full()
            .max_w(rem_px(COLUMN_WIDTH))
            .mx_auto()
            .px_6()
            .py_5()
            .gap_4()
            .child(
                div()
                    .text_size(rem_px(TITLE_SIZE * factor))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("whats_new.name")),
            );
        if let Some(shown) = &self.shown {
            banner = shown
                .plan
                .banner
                .map(|version| self.render_banner(version, cx).into_any_element());
            if let Some(line) = shown.plan.line {
                column = column.child(self.render_line(line, cx));
            }
            match &shown.plan.body {
                Body::Sections {
                    sections,
                    older_exist,
                } => {
                    for (index, section) in sections.iter().enumerate() {
                        let body = shown.bodies.get(index).and_then(Option::as_ref);
                        column = column.child(self.render_section(index, section, body, cx));
                    }
                    if *older_exist {
                        column = column.child(self.render_older(cx));
                    }
                }
                Body::UpToDate => {
                    column = column.child(self.render_message(t!("whats_new.up_to_date"), cx));
                }
                Body::NoNotes => {
                    column = column.child(self.render_message(t!("whats_new.no_notes"), cx));
                }
                Body::Nothing => {}
            }
        }
        v_flex()
            .id("whats-new")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.scroll_by_key(event, cx)))
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(background)
            .border_2()
            .border_color(ring)
            .children(banner)
            .child(
                div()
                    .id("whats-new-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(column),
            )
    }
}
