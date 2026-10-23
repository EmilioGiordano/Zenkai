use serde::{Deserialize, Serialize};

use super::CodecError;
use crate::frame::Frame;
use crate::wire::{
    Bitmap, BrokenFile, CharBox, FindMatch, Job, LinkTarget, MAX_CHARS, MAX_LINKS, MAX_MATCHES,
    MAX_MESSAGE_CHARS, MAX_OUTLINE_DEPTH, MAX_OUTLINE_ITEMS, MAX_PAGE_POINTS, MAX_PAGES,
    MAX_RECT_POINTS, MAX_RENDER_PIXELS, MAX_TITLE_CHARS, OpenError, Opened, OutlineItem,
    PageContent, PageIdx, PageRect, PageSize, Reply, web_url,
};

const BYTES_PER_PIXEL: usize = 4;

#[derive(Serialize, Deserialize)]
struct ReplyEnvelope {
    id: u64,
    head: ReplyHead,
}

// What fits in a small header. The bodies that can be large travel in the payload.
#[derive(Serialize, Deserialize)]
enum ReplyHead {
    Done,
    Opened {
        failure: Option<OpenError>,
        page_count: usize,
        first_page: PageSize,
    },
    Sizes,
    Rendered {
        page: PageIdx,
        pixel_width: u32,
        thumbnail: bool,
        generation: u64,
        width: u32,
        height: u32,
    },
    Content {
        page: PageIdx,
    },
    Found {
        search: u64,
        next: Option<PageIdx>,
    },
}

// What the app asked, which is all a reply may answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    Open,
    Sizes,
    Render {
        page: PageIdx,
        pixel_width: u32,
        thumbnail: bool,
        generation: u64,
    },
    Content {
        page: PageIdx,
    },
    Find {
        search: u64,
        first: PageIdx,
        count: usize,
    },
    Nothing,
}

impl Expect {
    pub fn of(job: &Job) -> Expect {
        match job {
            Job::Open { .. } => Expect::Open,
            Job::Sizes => Expect::Sizes,
            Job::Render {
                page,
                pixel_width,
                thumbnail,
                generation,
                ..
            } => Expect::Render {
                page: *page,
                pixel_width: *pixel_width,
                thumbnail: *thumbnail,
                generation: *generation,
            },
            Job::Content { page } => Expect::Content { page: *page },
            Job::Find {
                search,
                first,
                count,
                ..
            } => Expect::Find {
                search: *search,
                first: *first,
                count: *count,
            },
            Job::Close | Job::Abort => Expect::Nothing,
        }
    }
}

pub fn encode_reply(id: u64, reply: Option<Reply>) -> Result<Frame, CodecError> {
    let (head, payload) = match reply {
        None | Some(Reply::Stopped) => (ReplyHead::Done, Vec::new()),
        Some(Reply::Opened(Err(failure))) => (
            ReplyHead::Opened {
                failure: Some(failure),
                page_count: 0,
                first_page: PageSize::LETTER,
            },
            Vec::new(),
        ),
        Some(Reply::Opened(Ok(opened))) => (
            ReplyHead::Opened {
                failure: None,
                page_count: opened.page_count,
                first_page: opened.first_page,
            },
            serde_json::to_vec(&opened.outline)?,
        ),
        Some(Reply::Sizes(sizes)) => (ReplyHead::Sizes, serde_json::to_vec(&sizes)?),
        Some(Reply::Rendered {
            page,
            pixel_width,
            thumbnail,
            generation,
            bitmap,
        }) => {
            let (width, height, bytes) = match bitmap {
                Some(bitmap) => (bitmap.width, bitmap.height, bitmap.bgra),
                None => (0, 0, Vec::new()),
            };
            (
                ReplyHead::Rendered {
                    page,
                    pixel_width,
                    thumbnail,
                    generation,
                    width,
                    height,
                },
                bytes,
            )
        }
        Some(Reply::Content { page, content }) => {
            (ReplyHead::Content { page }, serde_json::to_vec(&content)?)
        }
        Some(Reply::Found {
            search,
            matches,
            next,
        }) => (
            ReplyHead::Found { search, next },
            serde_json::to_vec(&matches)?,
        ),
    };
    Ok(Frame {
        head: serde_json::to_vec(&ReplyEnvelope { id, head })?,
        payload,
    })
}

pub fn reply_id(frame: &Frame) -> Result<u64, CodecError> {
    let envelope: ReplyEnvelope = serde_json::from_slice(&frame.head)?;
    Ok(envelope.id)
}

fn sound(value: f32, limit: f32) -> bool {
    value.is_finite() && value.abs() <= limit
}

fn sound_size(size: &PageSize) -> bool {
    sound(size.width, MAX_PAGE_POINTS)
        && sound(size.height, MAX_PAGE_POINTS)
        && size.width > 0.0
        && size.height > 0.0
}

fn sound_rect(rect: &PageRect) -> bool {
    let limit = MAX_RECT_POINTS;
    [rect.left, rect.top, rect.right, rect.bottom]
        .iter()
        .all(|value| sound(*value, limit))
}

fn sound_message(failure: &OpenError) -> bool {
    match failure {
        OpenError::LibraryFailed(text) | OpenError::Broken(BrokenFile::Unreadable(text)) => {
            text.chars().count() <= MAX_MESSAGE_CHARS
        }
        OpenError::LibraryMissing
        | OpenError::PasswordRequired
        | OpenError::WrongPassword
        | OpenError::Broken(_) => true,
        OpenError::HostMissing | OpenError::HostFailed(_) | OpenError::HostStopped => false,
    }
}

// Everything in a reply from the host is checked against the request it answers and the
// limits of the viewer. `None` is the host saying it has nothing to give for that request.
pub fn decode_reply(frame: Frame, expect: &Expect) -> Result<Option<Reply>, CodecError> {
    let envelope: ReplyEnvelope = serde_json::from_slice(&frame.head)?;
    let rule = CodecError::Rule;
    let no_payload = |frame: &Frame| {
        if frame.payload.is_empty() {
            Ok(())
        } else {
            Err(rule("a reply has a payload it should not have"))
        }
    };
    match (envelope.head, expect) {
        (ReplyHead::Done, Expect::Open) => Err(rule("an open was not answered")),
        (ReplyHead::Done, _) => {
            no_payload(&frame)?;
            Ok(None)
        }
        (
            ReplyHead::Opened {
                failure,
                page_count,
                first_page,
            },
            Expect::Open,
        ) => {
            if let Some(failure) = failure {
                no_payload(&frame)?;
                if !sound_message(&failure) {
                    return Err(rule("an open failure is not one the host may report"));
                }
                return Ok(Some(Reply::Opened(Err(failure))));
            }
            if page_count == 0 || page_count > MAX_PAGES || !sound_size(&first_page) {
                return Err(rule("an opened document has bad numbers"));
            }
            let outline: Vec<OutlineItem> = serde_json::from_slice(&frame.payload)?;
            let outline_ok = outline.len() <= MAX_OUTLINE_ITEMS
                && outline.iter().all(|item| {
                    item.title.chars().count() <= MAX_TITLE_CHARS
                        && item.depth <= MAX_OUTLINE_DEPTH
                        && item.page.0 < page_count
                });
            if !outline_ok {
                return Err(rule("an outline is out of bounds"));
            }
            Ok(Some(Reply::Opened(Ok(Opened {
                page_count,
                first_page,
                outline,
            }))))
        }
        (ReplyHead::Sizes, Expect::Sizes) => {
            let sizes: Vec<PageSize> = serde_json::from_slice(&frame.payload)?;
            if sizes.len() > MAX_PAGES || !sizes.iter().all(sound_size) {
                return Err(rule("the page sizes are out of bounds"));
            }
            Ok(Some(Reply::Sizes(sizes)))
        }
        (
            ReplyHead::Rendered {
                page,
                pixel_width,
                thumbnail,
                generation,
                width,
                height,
            },
            Expect::Render {
                page: asked_page,
                pixel_width: asked_width,
                thumbnail: asked_thumbnail,
                generation: asked_generation,
            },
        ) => {
            if page != *asked_page
                || pixel_width != *asked_width
                || thumbnail != *asked_thumbnail
                || generation != *asked_generation
            {
                return Err(rule("a render answers another request"));
            }
            let bitmap = if width == 0 && height == 0 {
                no_payload(&frame)?;
                None
            } else {
                let pixels = u64::from(width) * u64::from(height);
                if width == 0 || height == 0 || pixels > MAX_RENDER_PIXELS {
                    return Err(rule("a bitmap has a bad size"));
                }
                let expected = usize::try_from(pixels)
                    .ok()
                    .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL));
                if expected != Some(frame.payload.len()) {
                    return Err(rule("a bitmap does not have the bytes its size needs"));
                }
                Some(Bitmap {
                    width,
                    height,
                    bgra: frame.payload,
                })
            };
            Ok(Some(Reply::Rendered {
                page,
                pixel_width,
                thumbnail,
                generation,
                bitmap,
            }))
        }
        (ReplyHead::Content { page }, Expect::Content { page: asked }) => {
            if page != *asked {
                return Err(rule("a page text answers another page"));
            }
            let content: PageContent = serde_json::from_slice(&frame.payload)?;
            if content.chars.len() > MAX_CHARS
                || content.links.len() > MAX_LINKS
                || !content.chars.iter().all(sound_char)
                || !content.links.iter().all(sound_link)
            {
                return Err(rule("a page text is out of bounds"));
            }
            Ok(Some(Reply::Content { page, content }))
        }
        (
            ReplyHead::Found { search, next },
            Expect::Find {
                search: asked,
                first,
                count,
            },
        ) => {
            let end = first.0.saturating_add(*count);
            let next_ok = next.is_none_or(|page| page.0 == end);
            if search != *asked || !next_ok {
                return Err(rule("a search answers another request"));
            }
            let matches: Vec<FindMatch> = serde_json::from_slice(&frame.payload)?;
            let matches_ok = matches.len() <= MAX_MATCHES
                && matches.iter().all(|found| {
                    found.page.0 >= first.0
                        && found.page.0 < end
                        && found.span.len > 0
                        && found.span.start.saturating_add(found.span.len) <= MAX_CHARS
                });
            if !matches_ok {
                return Err(rule("search matches are out of bounds"));
            }
            Ok(Some(Reply::Found {
                search,
                matches,
                next,
            }))
        }
        _ => Err(rule("a reply is not of the kind that was asked")),
    }
}

fn sound_char(found: &CharBox) -> bool {
    sound_rect(&found.rect)
}

fn sound_link(link: &crate::wire::PageLink) -> bool {
    sound_rect(&link.rect)
        && match &link.target {
            LinkTarget::Page(page) => page.0 < MAX_PAGES,
            LinkTarget::Web(url) => web_url(url).as_deref() == Some(url.as_str()),
        }
}

#[cfg(test)]
mod tests;
