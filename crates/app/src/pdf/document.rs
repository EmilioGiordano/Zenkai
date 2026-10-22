use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

use async_channel::{Receiver, Sender};
use pdfium_render::prelude::*;

use super::find::{FindMatch, find_in_page};
use super::layout::{PageIdx, PageSize};
use super::library;
use super::outline::read_outline;
use super::page_text::{read_content, search_text};
use super::protocol::{Bitmap, BrokenFile, DocId, Job, OpenError, Opened, Reply, Request};

const BYTES_PER_PIXEL: usize = 4;

// Pdfium is not safe to call from several threads, so one thread owns the library and every
// open document; the rest of the app talks to it through this channel.
#[derive(Clone)]
pub struct PdfWorker {
    requests: Sender<Request>,
}

static SHARED: OnceLock<Result<PdfWorker, OpenError>> = OnceLock::new();

impl PdfWorker {
    // One worker per process: pdfium binds to its library once, so a second worker could not.
    pub fn shared() -> Result<PdfWorker, OpenError> {
        SHARED.get_or_init(PdfWorker::start).clone()
    }

    fn start() -> Result<PdfWorker, OpenError> {
        let (requests, receiver) = async_channel::unbounded();
        std::thread::Builder::new()
            .name("pdfium".to_string())
            .spawn(move || run(&receiver))
            .map_err(|error| OpenError::LibraryFailed(error.to_string()))?;
        Ok(PdfWorker { requests })
    }

    pub fn submit(&self, request: Request) -> bool {
        self.requests.try_send(request).is_ok()
    }
}

fn run(requests: &Receiver<Request>) {
    match library::bind_next_to_exe() {
        Ok(pdfium) => serve(&pdfium, requests),
        Err(error) => refuse(&error, requests),
    }
}

fn refuse(error: &OpenError, requests: &Receiver<Request>) {
    while let Ok(request) = requests.recv_blocking() {
        if matches!(request.job, Job::Open { .. }) {
            let reply = Reply::Opened(Err(error.clone()));
            if request.reply.try_send(reply).is_err() {
                tracing::debug!("the PDF view closed before the library error reached it");
            }
        }
    }
}

fn serve(pdfium: &Pdfium, requests: &Receiver<Request>) {
    let mut documents: HashMap<DocId, PdfDocument<'_>> = HashMap::new();
    while let Ok(request) = requests.recv_blocking() {
        let Request { doc, reply, job } = request;
        let answer = catch_unwind(AssertUnwindSafe(|| {
            process(pdfium, &mut documents, doc, &job)
        }))
        .unwrap_or_else(|_| {
            tracing::error!("pdfium panicked while serving a request");
            failure_reply(&job)
        });
        if let Some(answer) = answer
            && reply.try_send(answer).is_err()
        {
            tracing::debug!("the PDF view closed before its answer arrived");
        }
    }
}

fn failure_reply(job: &Job) -> Option<Reply> {
    match job {
        Job::Open { .. } => Some(Reply::Opened(Err(OpenError::Broken(BrokenFile::Other)))),
        Job::Render {
            page,
            pixel_width,
            thumbnail,
            ..
        } => Some(Reply::Rendered {
            page: *page,
            pixel_width: *pixel_width,
            thumbnail: *thumbnail,
            bitmap: None,
        }),
        Job::Sizes | Job::Content { .. } | Job::Find { .. } | Job::Close => None,
    }
}

fn process<'a>(
    pdfium: &'a Pdfium,
    documents: &mut HashMap<DocId, PdfDocument<'a>>,
    doc: DocId,
    job: &Job,
) -> Option<Reply> {
    match job {
        Job::Open { path, password } => {
            documents.remove(&doc);
            let opened = pdfium
                .load_pdf_from_file(path, password.as_deref())
                .map_err(|error| open_error(&error, password.is_some()))
                .and_then(|document| {
                    let opened = describe(&document)?;
                    documents.insert(doc, document);
                    Ok(opened)
                });
            Some(Reply::Opened(opened))
        }
        Job::Close => {
            documents.remove(&doc);
            None
        }
        Job::Sizes => {
            let document = documents.get(&doc)?;
            let sizes = document.pages().page_sizes().ok()?;
            Some(Reply::Sizes(sizes.into_iter().map(page_size).collect()))
        }
        Job::Render {
            page,
            pixel_width,
            pixel_height,
            thumbnail,
            current,
            generation,
        } => {
            if current.load(Ordering::Relaxed) != *generation {
                return None;
            }
            let document = documents.get(&doc)?;
            Some(Reply::Rendered {
                page: *page,
                pixel_width: *pixel_width,
                thumbnail: *thumbnail,
                bitmap: render(document, *page, *pixel_width, *pixel_height),
            })
        }
        Job::Content { page } => {
            let document = documents.get(&doc)?;
            let content = read_content(&document.pages().get(page_index(*page)?).ok()?);
            Some(Reply::Content {
                page: *page,
                content,
            })
        }
        Job::Find {
            search,
            query,
            first,
            count,
        } => {
            let document = documents.get(&doc)?;
            let total = usize::try_from(document.pages().len()).unwrap_or(0);
            let end = (first.0 + count).min(total);
            let mut matches = Vec::new();
            for index in first.0..end {
                let Some(chars) = page_index(PageIdx(index))
                    .and_then(|index| document.pages().get(index).ok())
                    .and_then(|page| search_text(&page))
                else {
                    continue;
                };
                matches.extend(
                    find_in_page(&chars, query)
                        .into_iter()
                        .map(|span| FindMatch {
                            page: PageIdx(index),
                            span,
                        }),
                );
            }
            Some(Reply::Found {
                search: *search,
                matches,
                next: (end < total).then_some(PageIdx(end)),
            })
        }
    }
}

pub(super) fn page_index(page: PageIdx) -> Option<PdfPageIndex> {
    PdfPageIndex::try_from(page.0).ok()
}

fn page_size(rect: PdfRect) -> PageSize {
    PageSize {
        width: rect.right().value,
        height: rect.top().value,
    }
}

fn open_error(error: &PdfiumError, had_password: bool) -> OpenError {
    match error {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            if had_password {
                OpenError::WrongPassword
            } else {
                OpenError::PasswordRequired
            }
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError) => {
            OpenError::Broken(BrokenFile::NotAPdf)
        }
        PdfiumError::IoError(error) => OpenError::Broken(BrokenFile::Unreadable(error.to_string())),
        _ => OpenError::Broken(BrokenFile::Other),
    }
}

fn describe(document: &PdfDocument<'_>) -> Result<Opened, OpenError> {
    let pages = document.pages();
    let first = pages
        .page_size(0)
        .map_err(|_| OpenError::Broken(BrokenFile::NoPages))?;
    Ok(Opened {
        page_count: usize::try_from(pages.len()).unwrap_or(0),
        first_page: page_size(first),
        outline: read_outline(document),
    })
}

fn render(
    document: &PdfDocument<'_>,
    page: PageIdx,
    pixel_width: u32,
    pixel_height: u32,
) -> Option<Bitmap> {
    let width = i32::try_from(pixel_width).ok()?;
    let height = i32::try_from(pixel_height).ok()?;
    let page = document.pages().get(page_index(page)?).ok()?;
    let config = PdfRenderConfig::new()
        .set_target_size(width, height)
        .set_format(PdfBitmapFormat::BGRA)
        .set_clear_color(PdfColor::WHITE)
        .render_form_data(false);
    let bitmap = page.render_with_config(&config).ok()?;
    let bgra = bitmap.as_raw_bytes();
    let width = u32::try_from(bitmap.width()).ok()?;
    let height = u32::try_from(bitmap.height()).ok()?;
    let exact = usize::try_from(width).ok()? * usize::try_from(height).ok()? * BYTES_PER_PIXEL;
    (bgra.len() == exact).then_some(Bitmap {
        width,
        height,
        bgra,
    })
}

#[cfg(test)]
mod tests;
