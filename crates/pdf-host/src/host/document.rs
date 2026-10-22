use std::collections::HashMap;

use pdfium_render::prelude::*;

use super::outline::read_outline;
use super::page_text::{read_content, search_text};
use crate::textmatch::find_in_page;
use crate::wire::{
    Bitmap, BrokenFile, DocId, FindMatch, Job, MAX_MATCHES, MAX_PAGES, OpenError, Opened, PageIdx,
    PageSize, Reply,
};

const BYTES_PER_PIXEL: usize = 4;

pub type Documents<'a> = HashMap<DocId, PdfDocument<'a>>;

pub fn failure_reply(job: &Job) -> Option<Reply> {
    match job {
        Job::Open { .. } => Some(Reply::Opened(Err(OpenError::Broken(BrokenFile::Other)))),
        Job::Render {
            page,
            pixel_width,
            thumbnail,
            generation,
            ..
        } => Some(Reply::Rendered {
            page: *page,
            pixel_width: *pixel_width,
            thumbnail: *thumbnail,
            generation: *generation,
            bitmap: None,
        }),
        Job::Sizes | Job::Content { .. } | Job::Find { .. } | Job::Close | Job::Abort => None,
    }
}

pub fn process<'a>(
    pdfium: &'a Pdfium,
    documents: &mut Documents<'a>,
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
            generation,
        } => {
            let document = documents.get(&doc)?;
            Some(Reply::Rendered {
                page: *page,
                pixel_width: *pixel_width,
                thumbnail: *thumbnail,
                generation: *generation,
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
            let end = first.0.saturating_add(*count).min(total);
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
                if matches.len() >= MAX_MATCHES {
                    matches.truncate(MAX_MATCHES);
                    break;
                }
            }
            // The app expects the chunk it asked for, so the next page is where that chunk
            // ends, even when the matches stopped early at the cap.
            let asked_end = first.0.saturating_add(*count);
            Some(Reply::Found {
                search: *search,
                matches,
                next: (asked_end < total).then_some(PageIdx(asked_end)),
            })
        }
        Job::Abort => None,
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

pub(super) fn describe(document: &PdfDocument<'_>) -> Result<Opened, OpenError> {
    let pages = document.pages();
    let first = pages
        .page_size(0)
        .map_err(|_| OpenError::Broken(BrokenFile::NoPages))?;
    let page_count = usize::try_from(pages.len()).unwrap_or(0);
    if page_count > MAX_PAGES {
        return Err(OpenError::Broken(BrokenFile::TooManyPages));
    }
    Ok(Opened {
        page_count,
        first_page: page_size(first),
        outline: read_outline(document),
    })
}

pub(super) fn render(
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
