use std::sync::PoisonError;
use std::time::UNIX_EPOCH;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use zenkai_engine::{Engine, Workbook};
use zenkai_i18n::t;
use zenkai_types::{CellPos, Range, SheetId};

use super::mention_candidates::{self, Candidate, Group, Hint};
use super::mention_catalog::{Catalog, Used};
use super::mention_parse::{self, ActiveQuery};
use super::mention_prepare::{self, Prepared};
use super::{ChatPanel, MONO, closed};
use crate::agent_folder;
use crate::interface_scale::rem_px;

const ROW_HEIGHT: f32 = 28.0;

#[derive(Default)]
pub(super) struct Mentions {
    // Built when the picker opens and dropped when it closes, so typing never queries documents.
    catalog: Option<Catalog>,
    folder: Vec<String>,
    index: usize,
    // The composer text for which the user closed the picker.
    dismissed: Option<String>,
}

fn group_title(group: Group) -> &'static str {
    match group {
        Group::Selection => t!("chat.mention.group.selection"),
        Group::Sheets => t!("chat.mention.group.sheets"),
        Group::Workbooks => t!("chat.mention.group.workbooks"),
    }
}

fn hint_text(hint: &Hint) -> Option<String> {
    match hint {
        Hint::Pending => None,
        Hint::Address(address) => Some(address.clone()),
        Hint::UsedRange(range) => Some(t!("chat.mention.hint.used_range", range = range)),
        Hint::EmptySheet => Some(t!("chat.mention.hint.empty").to_string()),
        Hint::Open => Some(t!("chat.mention.hint.open").to_string()),
        Hint::WorkingFolder => Some(t!("chat.mention.hint.folder").to_string()),
    }
}

pub(super) fn with_notes(context: String, notes: &[String]) -> String {
    if notes.is_empty() {
        return context;
    }
    format!("{context} Mentions in the message: {}.", notes.join("; "))
}

fn used_range(workbook: &Workbook, sheet: SheetId) -> Used {
    let end = workbook.used_end(sheet);
    if end == CellPos::default() && workbook.input(sheet, end).is_empty() {
        Used::Empty
    } else {
        Used::Range(Range::new(CellPos::default(), end))
    }
}

impl ChatPanel {
    fn mention_query(&self, cx: &App) -> Option<ActiveQuery> {
        let composer = self.composer.read(cx);
        let text = composer.value();
        if self.mentions.dismissed.as_deref() == Some(text.as_ref()) {
            return None;
        }
        mention_parse::active_query(&text, composer.cursor())
    }

    fn mention_candidates(&self, cx: &App) -> Vec<Candidate> {
        let Some(query) = self.mention_query(cx) else {
            return Vec::new();
        };
        match &self.mentions.catalog {
            Some(catalog) => mention_candidates::candidates(&query.text, catalog),
            None => Vec::new(),
        }
    }

    pub(super) fn mention_open(&self, cx: &App) -> bool {
        !self.mention_candidates(cx).is_empty()
    }

    // `None` when the workspace is gone, so there is nothing to mention.
    fn catalog_now(&self, cx: &App) -> Option<Catalog> {
        match self
            .workspace
            .read_with(cx, |workspace, cx| workspace.mention_catalog_for_chat(cx))
        {
            Ok(mut catalog) => {
                catalog.folder = self.mentions.folder.clone();
                Some(catalog)
            }
            Err(error) => {
                tracing::debug!(%error, "the workspace closed before the mentions were read");
                None
            }
        }
    }

    pub(super) fn follow_mention_query(&mut self, cx: &mut Context<Self>) {
        self.mentions.index = 0;
        if self.mention_query(cx).is_none() {
            self.mentions.catalog = None;
        } else if self.mentions.catalog.is_none() {
            self.mentions.catalog = self.catalog_now(cx);
            if self.mentions.catalog.is_some() {
                self.scan_working_folder(cx);
                self.read_used_ranges(cx);
            }
        }
    }

    // Only the folder the live agent works in, which is all it can open.
    fn scan_working_folder(&self, cx: &mut Context<Self>) {
        let Some(live) = &self.live else {
            return;
        };
        let folder = live.folder.path().to_path_buf();
        cx.spawn(async move |this, cx| {
            let scanned = folder.clone();
            let found = cx
                .background_executor()
                .spawn(
                    async move { agent_folder::spreadsheets_written_since(&scanned, UNIX_EPOCH) },
                )
                .await;
            let names: Vec<String> = found
                .into_iter()
                .map(|file| file.relative.to_string_lossy().replace('\\', "/"))
                .collect();
            closed(this.update(cx, |this, cx| {
                let still_there = this
                    .live
                    .as_ref()
                    .is_some_and(|live| live.folder.path() == folder);
                if !still_there {
                    return;
                }
                if let Some(catalog) = this.mentions.catalog.as_mut() {
                    catalog.folder = names.clone();
                }
                this.mentions.folder = names;
                cx.notify();
            }));
        })
        .detach();
    }

    // The sheets show without a hint at once; each hint fills in when the engine has answered.
    fn read_used_ranges(&self, cx: &mut Context<Self>) {
        let Some(active) = self
            .mentions
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.active.as_ref())
        else {
            return;
        };
        let (book, sheets) = (
            active.book.id,
            active
                .sheets
                .iter()
                .map(|sheet| sheet.id)
                .collect::<Vec<_>>(),
        );
        let shared = match self
            .workspace
            .read_with(cx, |workspace, _| workspace.shared_workbook_for_chat(book))
        {
            Ok(Some(shared)) => shared,
            Ok(None) => return,
            Err(error) => {
                tracing::debug!(%error, "the workspace closed before the used ranges were read");
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let used = cx
                .background_executor()
                .spawn(async move {
                    let workbook = shared.read().unwrap_or_else(PoisonError::into_inner);
                    sheets
                        .into_iter()
                        .map(|sheet| (sheet, used_range(&workbook, sheet)))
                        .collect::<Vec<_>>()
                })
                .await;
            closed(this.update(cx, |this, cx| {
                if let Some(catalog) = this.mentions.catalog.as_mut() {
                    catalog.fill_used(book, &used);
                    cx.notify();
                }
            }));
        })
        .detach();
    }

    // Without the workspace the text goes as typed, with nothing resolved.
    pub(super) fn prepare_mentions(&self, text: &str, cx: &App) -> Prepared {
        match self.catalog_now(cx) {
            Some(catalog) => mention_prepare::prepare(text, &catalog),
            None => Prepared {
                text: text.to_string(),
                notes: Vec::new(),
            },
        }
    }

    pub(super) fn close_mentions(&mut self) {
        self.mentions.catalog = None;
        self.mentions.index = 0;
    }

    // Also drops the folder list, which belongs to the agent that was live when it was read.
    pub(super) fn forget_mentions(&mut self) {
        self.close_mentions();
        self.mentions.folder.clear();
    }

    pub(super) fn mention_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.mention_candidates(cx).len();
        if count == 0 {
            return;
        }
        self.mentions.index = if forward {
            (self.mentions.index + 1) % count
        } else {
            (self.mentions.index + count - 1) % count
        };
        cx.notify();
    }

    pub(super) fn mention_close(&mut self, cx: &mut Context<Self>) {
        self.mentions.dismissed = Some(self.composer.read(cx).value().to_string());
        cx.notify();
    }

    pub(super) fn mention_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(query) = self.mention_query(cx) else {
            return;
        };
        let candidates = self.mention_candidates(cx);
        let last = candidates.len().saturating_sub(1);
        let Some(chosen) = candidates.get(self.mentions.index.min(last)) else {
            return;
        };
        let insert = chosen.insert.clone();
        self.close_mentions();
        self.composer.update(cx, |state, cx| {
            state.set_selected_range(query.start..query.end, cx);
            state.replace(insert, window, cx);
        });
        self.focus_composer(window, cx);
        cx.notify();
    }

    pub(super) fn render_mentions(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let candidates = self.mention_candidates(cx);
        if candidates.is_empty() {
            return None;
        }
        let selected = self.mentions.index.min(candidates.len() - 1);
        let theme = cx.theme();
        let (border, popover, muted, accent, accent_foreground) = (
            theme.border,
            theme.popover,
            theme.muted_foreground,
            theme.accent,
            theme.accent_foreground,
        );
        let mut list = v_flex().gap_0p5();
        let mut group = None;
        for (index, candidate) in candidates.iter().enumerate() {
            if group != Some(candidate.group) {
                group = Some(candidate.group);
                list = list.child(
                    div()
                        .px_2p5()
                        .pt_1p5()
                        .pb_1()
                        .text_xs()
                        .text_color(muted)
                        .child(group_title(candidate.group)),
                );
            }
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("mention-{index}")))
                    .role(Role::ListBoxOption)
                    .h(rem_px(ROW_HEIGHT))
                    .px_2p5()
                    .gap_2p5()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .when(index == selected, |row| {
                        row.bg(accent).text_color(accent_foreground)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.mentions.index = index;
                        this.mention_accept(window, cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(MONO)
                            .text_xs()
                            .child(candidate.label.clone()),
                    )
                    .children(hint_text(&candidate.hint).map(|hint| {
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(muted)
                            .child(hint)
                    })),
            );
        }
        Some(
            div()
                .id("chat-mentions")
                .role(Role::ListBox)
                .aria_label(t!("chat.mention.label"))
                .occlude()
                .absolute()
                .left_0()
                .right_0()
                .bottom(relative(1.0))
                .mb_2()
                .p_1p5()
                .rounded_lg()
                .border_1()
                .border_color(border)
                .bg(popover)
                .shadow_lg()
                .child(list)
                .child(
                    div()
                        .px_2p5()
                        .pt_1p5()
                        .pb_0p5()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("chat.mention.footer")),
                )
                .into_any_element(),
        )
    }
}
