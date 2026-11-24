use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::*;
use zenkai_i18n::t;

use super::draw::{Look, draw_block};
use super::links::{MAX_LINKS, NoteLink, Target};
use super::plan::{Body, Line, SectionPlan};
use super::{CONTEXT, NotesEvent, NotesView};
use crate::actions::{CheckForUpdates, UpdateNow};
use crate::interface_scale::rem_px;
use crate::keymap;
use crate::updates::{RELEASES_PAGE, Version};

const COLUMN_WIDTH: f32 = 640.0;
const TITLE_SIZE: f32 = 20.0;
const VERSION_SIZE: f32 = 15.0;

impl NotesView {
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
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(NotesEvent::OpenLink(RELEASES_PAGE.to_owned()))
                            })),
                    )
                    .child(
                        Button::new("whats-new-update")
                            .primary()
                            .small()
                            .label(t!("update.now"))
                            .tooltip(keymap::labeled(cx, t!("update.now"), &UpdateNow))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(NotesEvent::UpdateNow))),
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
                    .on_click(|_, _, cx| crate::updates::check_now(cx)),
            )
    }

    fn render_section(
        &self,
        index: usize,
        section: &SectionPlan,
        texts: &[SharedString],
        look: &Look,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let factor = self.zoom.factor();
        let muted = cx.theme().muted_foreground;
        let text = match &section.blocks {
            Some(blocks) => v_flex()
                .gap_1p5()
                .children(
                    blocks
                        .iter()
                        .zip(texts)
                        .map(|(block, text)| draw_block(block, text, look)),
                )
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
                Button::new(("whats-new-link", index * MAX_LINKS + position))
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
        let look = Look::new(window, cx, factor);
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
                        let texts = shown.texts.get(index).map_or(&[][..], Vec::as_slice);
                        column =
                            column.child(self.render_section(index, section, texts, &look, cx));
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
