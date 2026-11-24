use std::path::Path;
use std::sync::Arc;

use gpui_kit::*;
use zenkai_agent::chat::thread::FileCard;
use zenkai_agent::protected_view::FileOrigin;
use zenkai_agent::settings::PermissionMode;
use zenkai_agent::tools::{
    self, InsidePath, NewWorkbook, OpenedWorkbook, Opening, ReadOnlyReason, ToolCall, ToolError,
    ToolReply, WorkbookId, WorkingFolder,
};
use zenkai_engine::{Engine, XlsxReader};
use zenkai_i18n::t;

use super::agent_calls::Change;
use super::{Workspace, file_label};
use crate::agent_folder::FoundFile;
use crate::agent_settings;
use crate::csv_io;
use crate::document::Document;
use crate::entry::{CsvLoad, display_name};
use crate::failure::open_failure;
use crate::files::{self, FileLoad, LoadFailure};

fn load_agent_file(path: &Path, reader: XlsxReader) -> Result<FileLoad, LoadFailure> {
    if files::opens_as_itself(path) {
        csv_io::load_csv(path, None)
    } else {
        files::load_workbook(path, reader, &Arc::default())
    }
}

fn apply_loaded_document(
    document: &mut Document,
    read_only: bool,
    origin: FileOrigin,
    csv: Option<CsvLoad>,
) {
    document.read_only = read_only;
    document.origin = origin;
    if let Some(csv) = csv {
        document.bind_csv(csv.options);
        document.guard_file(csv.seen);
    }
}

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
            if self
                .documents
                .entry(id)
                .is_some_and(|entry| entry.kind().is_text() || entry.kind().is_pdf())
            {
                call.respond(Err(ToolError::File(
                    t!("markdown.agent_refused", name = path.relative().display()).to_string(),
                )));
                return;
            }
            self.switch_to(id, window, cx);
            let sheets = sheet_names(self.documents.get(id));
            call.respond(Ok(opened(id, Opening::AlreadyOpen, &path, sheets)));
            return;
        }
        let open = self
            .open_loads
            .start(t!("busy.opening", name = file_label(path.path())));
        cx.notify();
        let reader = files::xlsx_reader(agent_settings::settings(cx).advanced.xlsx_reader);
        cx.spawn_in(window, async move |this, cx| {
            let task_path = path.path().to_path_buf();
            let loaded = cx
                .background_executor()
                .spawn(async move { load_agent_file(&task_path, reader) })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.open_loads.finish(open);
                cx.notify();
                let file = match loaded {
                    Ok(file) => file,
                    Err(failure) => {
                        return call.respond(Err(ToolError::File(
                            open_failure(path.path(), &failure).text,
                        )));
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
            reader_fallback: _,
            csv,
        } = file;
        let id = self.open_document(workbook, Some(path.to_path_buf()), unsupported, window, cx);
        if let Some(document) = self.documents.get_mut(id) {
            apply_loaded_document(document, read_only, origin, csv);
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
        let mode = self.default_text_mode(cx);
        for file in found {
            if self.documents.find_by_path(&file.path).is_some() {
                continue;
            }
            self.documents
                .add_link(file.path.clone(), Some(file.size), mode);
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use zenkai_engine::Workbook;
    use zenkai_formats::{DateOrder, DecimalMark, Delimiter, Encoding, LineEnding};
    use zenkai_types::{CellPos, CellView, Contents, SheetId, ValueKind};

    use super::*;
    use crate::csv_save::{CsvSave, CsvSaveRequest, LargeWrite, save_csv_file};
    use crate::document::ChangedFile;
    use crate::documents::Documents;

    fn cell(workbook: &Workbook, a1: &str) -> CellView {
        workbook.cell(SheetId(0), CellPos::parse_a1(a1).unwrap())
    }

    fn number_at(workbook: &Workbook, a1: &str) -> Option<f64> {
        let pos = CellPos::parse_a1(a1).unwrap();
        match workbook.contents(SheetId(0)).unwrap()(pos) {
            Contents::Number(value) => Some(value),
            _ => None,
        }
    }

    #[test]
    fn agent_load_reads_csv_with_detected_options_and_keeps_signed_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sales.csv");
        std::fs::write(
            &path,
            b"name;date;amount;phone;note\r\nJos\xe9;25/10/2026;1.234,5;+34 600;-abc\r\nM\xe1laga;08/10/2026;-3,5;+5;-\r\n",
        )
        .unwrap();

        let loaded = load_agent_file(&path, XlsxReader::Fast).unwrap();
        assert!(!loaded.read_only);
        assert!(loaded.unsupported.is_empty());
        let csv = loaded.csv.unwrap();
        assert_eq!(csv.options.delimiter, Delimiter::Semicolon);
        assert_eq!(csv.options.encoding, Encoding::Windows1252);
        assert_eq!(csv.options.line_ending, LineEnding::Crlf);
        assert_eq!(csv.options.decimal_mark, DecimalMark::Comma);
        assert_eq!(csv.options.date_order, DateOrder::DayFirst);

        assert_eq!(cell(&loaded.workbook, "A2").text, "José");
        assert_eq!(number_at(&loaded.workbook, "B2"), Some(46320.0));
        assert_eq!(number_at(&loaded.workbook, "C2"), Some(1234.5));
        let phone = cell(&loaded.workbook, "D2");
        assert_eq!(phone.kind, ValueKind::Text);
        assert_eq!(phone.text, "+34 600");
        let note = cell(&loaded.workbook, "E2");
        assert_eq!(note.kind, ValueKind::Text);
        assert_eq!(note.text, "-abc");
        assert_eq!(cell(&loaded.workbook, "A3").text, "Málaga");
        assert_eq!(number_at(&loaded.workbook, "B3"), Some(46303.0));
        assert_eq!(number_at(&loaded.workbook, "C3"), Some(-3.5));
        assert_eq!(number_at(&loaded.workbook, "D3"), Some(5.0));
        let dash = cell(&loaded.workbook, "E3");
        assert_eq!(dash.kind, ValueKind::Text);
        assert_eq!(dash.text, "-");
    }

    #[test]
    fn agent_load_reads_tsv_with_tab_delimiter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contacts.tsv");
        std::fs::write(&path, b"name\tcity\tphone\nAna\tMadrid, ES\t+34 600\n").unwrap();

        let loaded = load_agent_file(&path, XlsxReader::Fast).unwrap();
        let csv = loaded.csv.unwrap();
        assert_eq!(csv.options.delimiter, Delimiter::Tab);
        assert_eq!(csv.options.encoding, Encoding::Utf8);
        assert_eq!(csv.options.line_ending, LineEnding::Lf);
        assert_eq!(cell(&loaded.workbook, "B2").text, "Madrid, ES");
        let phone = cell(&loaded.workbook, "C2");
        assert_eq!(phone.kind, ValueKind::Text);
        assert_eq!(phone.text, "+34 600");
    }

    #[test]
    fn agent_opened_csv_is_bound_to_its_path_and_saves_back_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prices.csv");
        let original = b"item;price;phone\nCaf\xe9;1,5;+34 600\n";
        std::fs::write(&path, original).unwrap();

        let loaded = load_agent_file(&path, XlsxReader::Fast).unwrap();
        let expected_options = loaded.csv.as_ref().unwrap().options;
        let mut documents = Documents::new(Workbook::new_empty().unwrap());
        let id = documents.open(loaded.workbook, Some(path.clone()), loaded.unsupported);
        let document = documents.get_mut(id).unwrap();
        apply_loaded_document(document, loaded.read_only, loaded.origin, loaded.csv);

        assert_eq!(document.path.as_deref(), Some(path.as_path()));
        assert!(!document.read_only);
        assert!(document.file_guard().is_some());
        let binding = document.csv.clone().unwrap();
        assert_eq!(binding.options, Some(expected_options));
        assert!(binding.file_sheet.is_some());

        let saved = save_csv_file(
            &document.shared_workbook(),
            &CsvSaveRequest {
                path: path.clone(),
                binding,
                sheets: document.sheets.len(),
                sheet: document.sheet,
                changed: ChangedFile::Ask,
                large: LargeWrite::Ask,
                guard: document.file_guard(),
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let CsvSave::Written(_, options, _, _) = saved else {
            panic!("expected a clean csv save");
        };
        assert_eq!(options, expected_options);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}
