use std::path::PathBuf;
use std::time::SystemTime;

use gpui_kit::*;
use zenkai_agent::chat::thread::FileCard;

use super::{ChatPanel, closed};
use crate::agent_folder;

impl ChatPanel {
    pub(crate) fn file_created(&mut self, card: FileCard, cx: &mut Context<Self>) {
        self.thread.file_created(card);
        self.scroll_to_end();
        cx.notify();
    }

    pub(super) fn open_created(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        closed(self.workspace.update(cx, |workspace, cx| {
            workspace.open_from_chat(path, window, cx)
        }));
    }

    // Spreadsheets the agent wrote in its folder by its own means, not through the tools, are
    // listed in the space after the turn and announced with the same card.
    pub(super) fn list_files_written(&mut self, since: SystemTime, cx: &mut Context<Self>) {
        let Some(folder) = self
            .live
            .as_ref()
            .map(|live| live.folder.path().to_path_buf())
        else {
            return;
        };
        let workspace = self.workspace.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { agent_folder::spreadsheets_written_since(&folder, since) })
                .await;
            if found.is_empty() {
                return;
            }
            let listed =
                workspace.update(cx, |workspace, cx| workspace.list_agent_files(found, cx));
            let cards = match listed {
                Ok(cards) => cards,
                Err(error) => {
                    tracing::debug!(%error, "workspace closed before new files were listed");
                    return;
                }
            };
            closed(this.update(cx, |this, cx| {
                for card in cards {
                    this.file_created(card, cx);
                }
            }));
        })
        .detach();
    }
}
