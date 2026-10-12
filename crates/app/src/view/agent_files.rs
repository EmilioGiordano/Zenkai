use std::path::Path;

use gpui_kit::*;
use zenkai_agent::chat::thread::FileCard;
use zenkai_agent::settings::PermissionMode;
use zenkai_agent::tools::{
    self, InsidePath, NewWorkbook, OpenedWorkbook, Opening, ReadOnlyReason, ToolCall, ToolError,
    ToolReply, WorkbookId, WorkingFolder,
};
use zenkai_engine::Engine;

use super::Workspace;
use super::agent_calls::Change;
use super::workbooks::failure_text;
use crate::agent_folder::FoundFile;
use crate::agent_settings;
use crate::document::Document;
use crate::entry::display_name;
use crate::files::{self, FileLoad};

fn sheet_names(document: Option<&Document>) -> Vec<String> {
    document.map_or_else(Vec::new, |document| {
        document
            .sheets
            .iter()
            .map(|info| info.name.clone())
            .collect()
    })
}

impl Workspace {
    pub(super) fn route_agent_create(
        &mut self,
        call: ToolCall,
        new: NewWorkbook,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match Self::permission(cx) {
            PermissionMode::ReadOnly => {
                call.respond(Err(ToolError::ReadOnly(ReadOnlyReason::Settings)))
            }
            PermissionMode::Automatic => self.create_for_agent(call, new, window, cx),
            PermissionMode::AskBeforeWrite => {
                self.ask_for_approval(call, Change::Create(new), window, cx)
            }
        }
    }

    pub(super) fn create_for_agent(
        &mut self,
        call: ToolCall,
        new: NewWorkbook,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let task = new.clone();
            let created = cx
                .background_executor()
                .spawn(async move { tools::create_workbook_file(&task) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                let workbook = match created {
                    Ok(workbook) => workbook,
                    Err(error) => return call.respond(Err(error)),
                };
                let sheets = workbook.sheets().into_iter().map(|s| s.name).collect();
                let path = new.path.path().to_path_buf();
                let id = this.open_document(workbook, Some(path.clone()), Vec::new(), window, cx);
                this.remember_recent(&path, cx);
                this.show_created_in_chat(&new.folder, &new.path, cx);
                call.respond(Ok(ToolReply::Opened(OpenedWorkbook {
                    id,
                    how: Opening::Created,
                    path: tools::relative_text(&new.path),
                    sheets,
                })));
            });
            if update.is_err() {
                tracing::debug!("workspace closed while an agent created a workbook");
            }
        })
        .detach();
    }

    pub(super) fn open_for_agent(
        &mut self,
        call: ToolCall,
        path: InsidePath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.documents.find_by_path(path.path()) {
            self.switch_to(id, window, cx);
            let sheets = sheet_names(self.documents.get(id));
            call.respond(Ok(opened(id, Opening::AlreadyOpen, &path, sheets)));
            return;
        }
        let reader = files::xlsx_reader(agent_settings::settings(cx).advanced.xlsx_reader);
        cx.spawn_in(window, async move |this, cx| {
            let task_path = path.path().to_path_buf();
            let loaded = cx
                .background_executor()
                .spawn(async move { files::load_workbook(&task_path, reader) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                let file = match loaded {
                    Ok(file) => file,
                    Err(failure) => {
                        return call.respond(Err(ToolError::File(failure_text(&failure))));
                    }
                };
                let id = this.install_agent_opened(file, path.path(), window, cx);
                let sheets = sheet_names(this.documents.get(id));
                call.respond(Ok(opened(id, Opening::Opened, &path, sheets)));
            });
            if update.is_err() {
                tracing::debug!("workspace closed while an agent opened a workbook");
            }
        })
        .detach();
    }

    fn install_agent_opened(
        &mut self,
        file: FileLoad,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> WorkbookId {
        if let Some(id) = self.documents.find_by_path(path) {
            self.switch_to(id, window, cx);
            return id;
        }
        let FileLoad {
            workbook,
            unsupported,
            read_only,
            origin,
            ..
        } = file;
        let id = self.open_document(workbook, Some(path.to_path_buf()), unsupported, window, cx);
        if let Some(document) = self.documents.get_mut(id) {
            document.read_only = read_only;
            document.origin = origin;
        }
        self.remember_recent(path, cx);
        id
    }

    // "Q3 close / Finanzas": the space it is listed in, then the subfolder of the working folder.
    fn place_in_space(&self, relative: &Path) -> String {
        let space = self.documents.current_space();
        let mut place = self
            .documents
            .spaces()
            .get(space)
            .map(|space| space.name.clone())
            .unwrap_or_default();
        if let Some(folder) = relative
            .parent()
            .filter(|folder| !folder.as_os_str().is_empty())
        {
            place.push_str(" / ");
            place.push_str(&folder.display().to_string());
        }
        place
    }

    fn show_created_in_chat(
        &mut self,
        folder: &WorkingFolder,
        path: &InsidePath,
        cx: &mut Context<Self>,
    ) {
        let card = FileCard {
            name: display_name(Some(path.path()), 0),
            place: self.place_in_space(path.relative()),
            path: folder.path().join(path.relative()),
        };
        self.add_chat_card(card, cx);
    }

    // Files the agent wrote by other means than the tools are listed in the current space,
    // unloaded, so the user sees them without Zenkai reading them.
    pub(crate) fn list_agent_files(
        &mut self,
        found: Vec<FoundFile>,
        cx: &mut Context<Self>,
    ) -> Vec<FileCard> {
        let mut cards = Vec::new();
        for file in found {
            if self.documents.find_by_path(&file.path).is_some() {
                continue;
            }
            self.documents.add_link(file.path.clone(), Some(file.size));
            cards.push(FileCard {
                name: display_name(Some(&file.path), 0),
                place: self.place_in_space(&file.relative),
                path: file.path,
            });
        }
        if !cards.is_empty() {
            self.persist_session(cx);
            cx.notify();
        }
        cards
    }
}

fn opened(id: WorkbookId, how: Opening, path: &InsidePath, sheets: Vec<String>) -> ToolReply {
    ToolReply::Opened(OpenedWorkbook {
        id,
        how,
        path: tools::relative_text(path),
        sheets,
    })
}
