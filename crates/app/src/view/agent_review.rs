use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::*;
use zenkai_agent::tools::{shown_entry, shown_text};
use zenkai_grid::PENDING_COLOR;
use zenkai_types::{CellRef, Range, SheetId, WorkbookId};

use super::Workspace;
use crate::actions::*;
use crate::agent_review::{CellChange, reject_unchanged};

const POPOVER_WIDTH: f32 = 300.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Resolution {
    Keep,
    Reject,
}

pub(super) struct AgentWrite {
    pub id: WorkbookId,
    pub generation: u64,
    pub agent: String,
    pub description: String,
    pub sheet_name: String,
}

fn cells_noun(count: usize) -> &'static str {
    if count == 1 { "cell" } else { "cells" }
}

fn shown_input(input: &str) -> String {
    if input.is_empty() {
        "(empty)".to_string()
    } else {
        shown_entry(input)
    }
}

impl Workspace {
    pub(super) fn record_agent_review(
        &mut self,
        write: AgentWrite,
        changes: Vec<CellChange>,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self
            .documents
            .get_mut(write.id)
            .filter(|document| document.is_current(write.generation))
        else {
            return;
        };
        document
            .review
            .record(write.agent, write.sheet_name, write.description, changes);
        self.sync_review_marks(cx);
    }

    pub(super) fn sync_review_marks(&mut self, cx: &mut Context<Self>) {
        let marks = self
            .documents
            .active()
            .map(|document| document.review.marks(document.sheet))
            .unwrap_or_default();
        self.grid
            .update(cx, |grid, cx| grid.set_review_marks(marks, cx));
        cx.notify();
    }

    pub(super) fn follow_review(&mut self, cx: &mut Context<Self>) {
        let active = self.grid.read(cx).selection().active;
        let Some(document) = self.documents.active_mut() else {
            return;
        };
        if document.review.is_empty() {
            return;
        }
        let cell = CellRef {
            sheet: document.sheet,
            pos: active,
        };
        document.review.follow(cell);
        cx.notify();
    }

    pub(super) fn forget_review_range(
        &mut self,
        sheet: SheetId,
        range: Range,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.documents.active_mut() else {
            return;
        };
        if document.review.is_empty() {
            return;
        }
        document.review.forget_in(sheet, range);
        self.sync_review_marks(cx);
    }

    pub(super) fn step_review(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.typing_in_a_field(cx) {
            return;
        }
        let active = self.grid.read(cx).selection().active;
        let Some(document) = self.documents.active_mut() else {
            return;
        };
        let at_cursor = document.review.cursor_cell()
            == Some(CellRef {
                sheet: document.sheet,
                pos: active,
            });
        let target = if at_cursor {
            document.review.step(forward)
        } else {
            document.review.cursor_cell()
        };
        let Some(target) = target else {
            return;
        };
        if target.sheet != document.sheet {
            self.switch_sheet(target.sheet, window, cx);
        }
        self.grid
            .update(cx, |grid, cx| grid.select(target.pos, target.pos, cx));
        cx.notify();
    }

    fn resolve_review_cell(
        &mut self,
        resolution: Resolution,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.typing_in_a_field(cx) {
            return;
        }
        let active = self.grid.read(cx).selection().active;
        let Some(document) = self.documents.active_mut() else {
            return;
        };
        let cell = CellRef {
            sheet: document.sheet,
            pos: active,
        };
        let Some(change) = document.review.forget(cell) else {
            return;
        };
        self.finish_resolution(resolution, vec![change], window, cx);
    }

    fn resolve_all_review(
        &mut self,
        resolution: Resolution,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.typing_in_a_field(cx) {
            return;
        }
        let Some(document) = self.documents.active_mut() else {
            return;
        };
        let changes = document.review.take_all();
        self.finish_resolution(resolution, changes, window, cx);
    }

    fn finish_resolution(
        &mut self,
        resolution: Resolution,
        changes: Vec<CellChange>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if resolution == Resolution::Reject && !changes.is_empty() {
            self.edit(window, cx, move |workbook| {
                reject_unchanged(workbook, &changes)
            });
        }
        self.sync_review_marks(cx);
    }

    pub(super) fn keep_review_cell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resolve_review_cell(Resolution::Keep, window, cx)
    }

    pub(super) fn reject_review_cell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resolve_review_cell(Resolution::Reject, window, cx)
    }

    pub(super) fn keep_all_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resolve_all_review(Resolution::Keep, window, cx)
    }

    pub(super) fn reject_all_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resolve_all_review(Resolution::Reject, window, cx)
    }

    pub(super) fn render_agent_review(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let document = self.documents.active()?;
        let batch = document.review.current()?;
        let position = document.review.position()?;
        let total = batch.cells().len();
        let writes = document.review.batch_count();
        let progress = if writes > 1 {
            format!("{position} of {total}, {writes} writes waiting")
        } else {
            format!("{position} of {total}")
        };
        let theme = cx.theme();
        let amber: Hsla = rgb(PENDING_COLOR).into();
        Some(
            h_flex()
                .id("agent-review")
                .role(Role::Region)
                .aria_label("Agent changes")
                .flex_wrap()
                .items_center()
                .gap_3()
                .mx_3()
                .mt_2()
                .px_3()
                .py_2()
                .rounded_md()
                .border_l_4()
                .border_color(amber)
                .bg(theme.secondary)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                            "{} changed {total} {} in {}",
                            batch.agent,
                            cells_noun(total),
                            shown_text(&batch.sheet_name)
                        )))
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(format!("{}. Ctrl+Z undoes it.", batch.description)),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(progress),
                )
                .child(
                    Button::new("review-previous")
                        .label("Previous (Alt+[)")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(PreviousAgentChange), cx)
                        }),
                )
                .child(Button::new("review-next").label("Next (Alt+])").on_click(
                    |_, window, cx| window.dispatch_action(Box::new(NextAgentChange), cx),
                ))
                .child(
                    Button::new("review-reject-all")
                        .label("Reject all (Alt+Shift+J)")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(RejectAllAgentChanges), cx)
                        }),
                )
                .child(
                    Button::new("review-keep-all")
                        .primary()
                        .label("Keep all (Alt+Shift+K)")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(KeepAllAgentChanges), cx)
                        }),
                ),
        )
    }

    pub(super) fn render_review_popover(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let document = self.documents.active()?;
        if self.user_is_editing(cx) {
            return None;
        }
        let grid = self.grid.read(cx);
        let cell = CellRef {
            sheet: document.sheet,
            pos: grid.selection().active,
        };
        let (batch, change) = document.review.find(cell)?;
        let corner = grid.active_cell_corner()?;
        let agent = batch.agent.clone();
        let theme = cx.theme();
        Some(
            v_flex()
                .id("review-popover")
                .role(Role::Dialog)
                .aria_label(format!("Change {}", cell.pos))
                .absolute()
                .left(corner.x)
                .top(corner.y + px(2.0))
                .w(px(POPOVER_WIDTH))
                .p_3()
                .gap_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .shadow_md()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("{}, changed by {agent}", cell.pos)),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .font_family(theme.mono_font_family.clone())
                        .text_sm()
                        .child(
                            div()
                                .text_color(theme.danger)
                                .line_through()
                                .child(format!("Was: {}", shown_input(&change.old))),
                        )
                        .child(
                            div()
                                .text_color(theme.success)
                                .child(format!("Now: {}", shown_input(&change.new))),
                        ),
                )
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("review-reject")
                                .label("Reject (Alt+J)")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(RejectAgentChange), cx)
                                }),
                        )
                        .child(
                            Button::new("review-keep")
                                .primary()
                                .label("Keep (Alt+K)")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(KeepAgentChange), cx)
                                }),
                        ),
                ),
        )
    }
}
