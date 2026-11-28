use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::*;
use zenkai_agent::protected_view::file_origin;
use zenkai_formats::{apply, parse_csv};
use zenkai_i18n::t;

use super::{Severity, Workspace, file_label};
use crate::csv_io::{self, CsvRead};
use crate::csv_preview::{self, CsvPreview, Import, PreviewEvent, Reading};
use crate::files;

// The preview a CSV is opened through, and the listening for what it reports.
pub(super) struct CsvSession {
    preview: Entity<CsvPreview>,
    _events: Subscription,
}

impl Workspace {
    pub(super) fn import_csv(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let read = self
            .open_loads
            .start(t!("busy.reading", name = file_label(&path)));
        cx.notify();
        let request = self.next_csv_request();
        cx.spawn_in(window, async move |this, cx| {
            let task_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    csv_io::read_csv(&task_path).map(|read| {
                        let CsvRead {
                            bytes,
                            parsed,
                            seen,
                        } = read;
                        (bytes, seen, Reading::of(parsed))
                    })
                })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.open_loads.finish(read);
                cx.notify();
                if this.csv_request != request {
                    return;
                }
                match result {
                    Ok((bytes, seen, reading)) => {
                        let preview =
                            cx.new(|cx| CsvPreview::new(path, Arc::new(bytes), reading, seen, cx));
                        this.show_csv_preview(preview, window, cx);
                    }
                    Err(error) => this.notify(Severity::Error, error, cx),
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed while reading a CSV");
            }
        })
        .detach();
    }

    fn show_csv_preview(
        &mut self,
        preview: Entity<CsvPreview>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let events = cx.subscribe(&preview, |this, _, event: &PreviewEvent, cx| match event {
            PreviewEvent::ReadFailed(error) => this.notify(Severity::Error, error.clone(), cx),
        });
        let focus = preview.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.csv_preview = Some(CsvSession {
            preview,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn confirm_csv_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(CsvSession { preview, .. }) = self.csv_preview.take() else {
            return;
        };
        let request = self.next_csv_request();
        let Import {
            path,
            bytes,
            parsed,
            options,
            seen,
        } = preview.update(cx, |preview, _| preview.take_import());
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let delimiter = csv_preview::delimiter_label(parsed.delimiter);
        let encoding = parsed.encoding.label();
        let summary = t!(
            "csv.imported",
            name = name,
            delimiter = delimiter,
            encoding = encoding
        );
        let untitled_summary = t!(
            "csv.imported_untitled",
            name = name,
            delimiter = delimiter,
            encoding = encoding
        );
        self.busy = Some(t!("busy.importing").into());
        cx.notify();
        let reading = options.clone();
        let mut rows = parsed.rows;
        let origin_path = path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let task_bytes = bytes.clone();
            let (result, origin) = cx
                .background_executor()
                .spawn(async move {
                    let origin = file_origin(&origin_path);
                    apply(
                        reading.decimal_mark,
                        reading.date_order,
                        &reading.text,
                        &mut rows,
                    );
                    // On failure, parse again so the preview comes back as it was.
                    let result = csv_io::workbook_from_rows(rows, &reading.text).map_err(|error| {
                        (error, parse_csv(&task_bytes, Some(reading.delimiter)).ok())
                    });
                    (result, origin)
                })
                .await;
            let update = this.update_in(cx, |this, window, cx| {
                this.clear_busy(t!("busy.importing"), cx);
                match result {
                    Ok(_) if this.csv_request != request => {}
                    Ok(workbook) if files::opens_as_itself(&path) => {
                        this.remember_recent(&path, cx);
                        if let Some(open) = this.documents.find_by_path(&path) {
                            this.switch_to(open, window, cx);
                            return;
                        }
                        let id = this.open_document(workbook, Some(path), Vec::new(), window, cx);
                        if let Some(document) = this.documents.get_mut(id) {
                            document.origin = origin;
                            document.bind_csv(options);
                            document.guard_file(seen);
                        }
                        this.notify(Severity::Info, summary, cx);
                    }
                    Ok(workbook) => {
                        let id = this.open_document(workbook, None, Vec::new(), window, cx);
                        if let Some(document) = this.documents.get_mut(id) {
                            document.origin = origin;
                        }
                        this.notify(Severity::Info, untitled_summary, cx);
                    }
                    Err((error, parsed)) => {
                        if let Some(parsed) = parsed
                            && this.csv_request == request
                        {
                            preview.update(cx, |preview, _| preview.restore(parsed));
                            this.show_csv_preview(preview, window, cx);
                        }
                        this.notify(Severity::Error, error, cx);
                    }
                }
            });
            if let Err(error) = update {
                tracing::debug!(%error, "workspace closed during import");
            }
        })
        .detach();
    }

    pub(super) fn cancel_csv_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.csv_preview = None;
        self.next_csv_request();
        let focus = self.grid.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn next_csv_request(&mut self) -> u64 {
        self.csv_request += 1;
        self.csv_request
    }

    // The palette runs the choices of the preview too, from where it has focus.
    pub(super) fn change_csv_preview(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut CsvPreview, &mut Context<CsvPreview>),
    ) {
        if let Some(session) = &self.csv_preview {
            session.preview.update(cx, change);
        }
    }

    pub(super) fn csv_preview_focus(&self, cx: &App) -> Option<FocusHandle> {
        self.csv_preview
            .as_ref()
            .map(|session| session.preview.read(cx).focus_handle(cx))
    }

    pub(super) fn render_csv_preview(&self) -> Option<Entity<CsvPreview>> {
        self.csv_preview
            .as_ref()
            .map(|session| session.preview.clone())
    }
}
