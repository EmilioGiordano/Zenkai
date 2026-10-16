use gpui_kit::base::v_flex;
use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use zenkai_i18n::t;
use zenkai_types::WorkbookId;

use super::{CloseDialog, button_row};
use crate::close_prompt::{Choices, Purpose};
use crate::document::Document;
use crate::view::Workspace;

impl Workspace {
    pub(in crate::view) fn ask_to_close_tab(
        &mut self,
        id: WorkbookId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(unsaved) = self
            .documents
            .unsaved()
            .into_iter()
            .find(|unsaved| unsaved.id == id)
        else {
            self.finish_close(id, window, cx);
            return;
        };
        let edits = self.documents.get(id).map(Document::edit_count);
        self.open_close_dialog(
            Purpose::Tab(id),
            Choices::new(vec![unsaved], false),
            edits,
            window,
            cx,
        );
    }

    pub(super) fn tab_content(&self, panel: Div, dialog: &CloseDialog, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let name = dialog
            .choices
            .items()
            .first()
            .map(|item| item.name.clone())
            .unwrap_or_default();
        panel
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("close.tab_title", name = name)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(t!("close.tab_detail")),
                    ),
            )
            .child(button_row(t!("close.save").to_string(), true))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("close.tab_hint")),
            )
    }
}
