use gpui_kit::base::h_flex;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::*;
use zenkai_agent::settings::PermissionMode;
use zenkai_agent::tools::{AgentAccess, ReadOnlyReason};
use zenkai_i18n::t;

use super::Workspace;
use crate::agent_routing;
use crate::agent_settings::AgentConfig;
use crate::assets::{LOCK_MARK, LOCK_OPEN_MARK};
use crate::document::Document;

const FILL_OPACITY: f32 = 0.14;
const EDGE_OPACITY: f32 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChipState {
    CanEdit,
    ReadOnlyFile,
    ReadOnlyPlan,
}

pub(super) fn chip_state(document: &Document, permission: PermissionMode) -> ChipState {
    match agent_routing::access(document, permission) {
        AgentAccess::Editable => ChipState::CanEdit,
        AgentAccess::ReadOnly(ReadOnlyReason::Settings) => ChipState::ReadOnlyPlan,
        AgentAccess::ReadOnly(_) => ChipState::ReadOnlyFile,
    }
}

impl Workspace {
    pub(super) fn render_access_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let document = self
            .documents
            .active()
            .filter(|document| !document.read_only)?;
        let permission = cx.global::<AgentConfig>().state.current.agents.permission;
        let state = chip_state(document, permission);
        let theme = cx.theme();
        let (color, mark, label) = match state {
            ChipState::CanEdit => (theme.success, LOCK_OPEN_MARK, t!("status.agents_can_edit")),
            ChipState::ReadOnlyFile => (theme.warning, LOCK_MARK, t!("status.agents_read_only")),
            ChipState::ReadOnlyPlan => {
                (theme.warning, LOCK_MARK, t!("status.agents_read_only_plan"))
            }
        };
        Some(
            h_flex()
                .id("agent-access-chip")
                .role(Role::Button)
                .aria_label(label)
                .gap_1()
                .items_center()
                .px_2()
                .rounded_md()
                .border_1()
                .border_color(color.opacity(EDGE_OPACITY))
                .bg(color.opacity(FILL_OPACITY))
                .text_color(color)
                .cursor_pointer()
                .child(Icon::empty().path(mark).size_3())
                .child(label)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_agent_editing(cx)))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use zenkai_agent::protected_view::FileOrigin;
    use zenkai_engine::Workbook;
    use zenkai_types::WorkbookId;

    use super::{ChipState, chip_state};
    use crate::document::Document;
    use crate::spaces::SpaceId;
    use zenkai_agent::settings::PermissionMode;

    fn document(origin: FileOrigin) -> Document {
        let mut document = Document::new(
            WorkbookId(1),
            SpaceId(0),
            1,
            Workbook::new_empty().unwrap(),
            Some(PathBuf::from("book.xlsx")),
            Vec::new(),
        );
        document.origin = origin;
        document
    }

    #[test]
    fn a_local_file_follows_the_permission_setting() {
        let local = document(FileOrigin::Local);
        assert_eq!(
            chip_state(&local, PermissionMode::Automatic),
            ChipState::CanEdit
        );
        assert_eq!(
            chip_state(&local, PermissionMode::AskBeforeWrite),
            ChipState::CanEdit
        );
        assert_eq!(
            chip_state(&local, PermissionMode::ReadOnly),
            ChipState::ReadOnlyPlan
        );
    }

    #[test]
    fn a_downloaded_file_is_read_only_whatever_the_setting() {
        let download = document(FileOrigin::Internet);
        for permission in [
            PermissionMode::Automatic,
            PermissionMode::AskBeforeWrite,
            PermissionMode::ReadOnly,
        ] {
            assert_eq!(chip_state(&download, permission), ChipState::ReadOnlyFile);
        }
    }
}
