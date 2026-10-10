use gpui_kit::base::h_flex;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::*;
use zenkai_i18n::t;

use super::Workspace;
use super::agent_access::agents_can_edit;
use crate::assets::{LOCK_MARK, LOCK_OPEN_MARK};

const FILL_OPACITY: f32 = 0.14;
const EDGE_OPACITY: f32 = 0.4;

impl Workspace {
    pub(super) fn render_access_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let document = self
            .documents
            .active()
            .filter(|document| !document.read_only)?;
        let editable = agents_can_edit(document.origin);
        let theme = cx.theme();
        let (color, mark, label) = if editable {
            (theme.success, LOCK_OPEN_MARK, t!("status.agents_can_edit"))
        } else {
            (theme.warning, LOCK_MARK, t!("status.agents_read_only"))
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
