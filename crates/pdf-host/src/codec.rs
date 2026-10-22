use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::frame::Frame;
use crate::wire::{
    Bitmap, BrokenFile, CharBox, DocId, FindMatch, Job, LinkTarget, MAX_CHARS, MAX_LINKS,
    MAX_MATCHES, MAX_MESSAGE_CHARS, MAX_OUTLINE_DEPTH, MAX_OUTLINE_ITEMS, MAX_PAGE_POINTS,
    MAX_PAGES, MAX_RENDER_PIXELS, MAX_TITLE_CHARS, OpenError, Opened, OutlineItem, PageContent,
    PageIdx, PageRect, PageSize, Reply, web_url,
};

const MAX_PATH_CHARS: usize = 32_768;
const MAX_QUERY_CHARS: usize = 1000;
const MAX_PAGES_PER_FIND: usize = 1000;
const BYTES_PER_PIXEL: usize = 4;

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("the frame could not be read: {0}")]
    Format(#[from] serde_json::Error),
    #[error("the frame breaks a rule: {0}")]
    Rule(&'static str),
}

#[derive(Serialize, Deserialize)]
struct RequestHead {
    id: u64,
    doc: u64,
    job: Job,
}

pub fn encode_request(id: u64, doc: DocId, job: &Job) -> Result<Frame, CodecError> {
    let head = serde_json::to_vec(&RequestHead {
        id,
        doc: doc.0,
        job: job.clone(),
    })?;
    Ok(Frame {
        head,
        payload: Vec::new(),
    })
}

// The host also treats what the app sends as input to check, not to trust.
pub fn decode_request(frame: &Frame) -> Result<(u64, DocId, Job), CodecError> {
    let head: RequestHead = serde_json::from_slice(&frame.head)?;
    if !frame.payload.is_empty() {
        return Err(CodecError::Rule("a request has no payload"));
    }
    match &head.job {
        Job::Open { path, password } => {
            let path_ok = path
                .to_str()
                .is_some_and(|text| text.chars().count() <= MAX_PATH_CHARS);
            let password_ok = password
                .as_ref()
                .is_none_or(|text| text.chars().count() <= MAX_MESSAGE_CHARS);
            if !path_ok || !password_ok {
                return Err(CodecError::Rule("an open request is too long"));
            }
        }
        Job::Render {
            pixel_width,
            pixel_height,
            ..
        } => {
            let pixels = u64::from(*pixel_width) * u64::from(*pixel_height);
            if pixels == 0 || pixels > MAX_RENDER_PIXELS {
                return Err(CodecError::Rule("a render asks for a bad size"));
            }
        }
        Job::Find { query, count, .. } => {
            if query.chars().count() > MAX_QUERY_CHARS || *count > MAX_PAGES_PER_FIND {
                return Err(CodecError::Rule("a find request is too large"));
            }
        }
        Job::Sizes | Job::Content { .. } | Job::Close | Job::Abort => {}
    }
    Ok((head.id, DocId(head.doc), head.job))
}

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

pub fn encode_reply(id: u64, reply: Option<&Reply>) -> Result<Frame, CodecError> {
    let (head, payload) = match reply {
        None | Some(Reply::Stopped) => (ReplyHead::Done, Vec::new()),
        Some(Reply::Opened(Err(failure))) => (
            ReplyHead::Opened {
                failure: Some(failure.clone()),
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
        Some(Reply::Sizes(sizes)) => (ReplyHead::Sizes, serde_json::to_vec(sizes)?),
        Some(Reply::Rendered {
            page,
            pixel_width,
            thumbnail,
            generation,
            bitmap,
        }) => {
            let (width, height, bytes) = match bitmap {
                Some(bitmap) => (bitmap.width, bitmap.height, bitmap.bgra.clone()),
                None => (0, 0, Vec::new()),
            };
            (
                ReplyHead::Rendered {
                    page: *page,
                    pixel_width: *pixel_width,
                    thumbnail: *thumbnail,
                    generation: *generation,
                    width,
                    height,
                },
                bytes,
            )
        }
        Some(Reply::Content { page, content }) => (
            ReplyHead::Content { page: *page },
            serde_json::to_vec(content)?,
        ),
        Some(Reply::Found {
            search,
            matches,
            next,
        }) => (
            ReplyHead::Found {
                search: *search,
                next: *next,
            },
            serde_json::to_vec(matches)?,
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
    let limit = MAX_PAGE_POINTS * 2.0;
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
pub fn decode_reply(frame: &Frame, expect: &Expect) -> Result<Option<Reply>, CodecError> {
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
            no_payload(frame)?;
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
                no_payload(frame)?;
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
                no_payload(frame)?;
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
                    bgra: frame.payload.clone(),
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
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::wire::{CharSpan, PageLink};

    fn round_trip(reply: &Reply, expect: &Expect) -> Result<Option<Reply>, CodecError> {
        let frame = encode_reply(7, Some(reply)).unwrap();
        assert_eq!(reply_id(&frame).unwrap(), 7);
        decode_reply(&frame, expect)
    }

    fn render_expect() -> Expect {
        Expect::Render {
            page: PageIdx(2),
            pixel_width: 2,
            thumbnail: false,
            generation: 5,
        }
    }

    fn rendered(width: u32, height: u32, bytes: usize) -> Reply {
        Reply::Rendered {
            page: PageIdx(2),
            pixel_width: 2,
            thumbnail: false,
            generation: 5,
            bitmap: Some(Bitmap {
                width,
                height,
                bgra: vec![9; bytes],
            }),
        }
    }

    #[test]
    fn every_job_survives_the_request_frame() {
        let jobs = [
            Job::Open {
                path: PathBuf::from("C:\\docs\\a.pdf"),
                password: Some("secret".to_string()),
            },
            Job::Sizes,
            Job::Render {
                page: PageIdx(3),
                pixel_width: 100,
                pixel_height: 200,
                thumbnail: true,
                generation: 8,
            },
            Job::Content { page: PageIdx(1) },
            Job::Find {
                search: 4,
                query: "hello".to_string(),
                first: PageIdx(0),
                count: 24,
            },
            Job::Close,
            Job::Abort,
        ];
        for job in jobs {
            let frame = encode_request(11, DocId(3), &job).unwrap();
            assert_eq!(decode_request(&frame).unwrap(), (11, DocId(3), job));
        }
    }

    #[test]
    fn the_host_refuses_requests_that_break_the_limits() {
        let big_render = Job::Render {
            page: PageIdx(0),
            pixel_width: 100_000,
            pixel_height: 100_000,
            thumbnail: false,
            generation: 0,
        };
        let empty_render = Job::Render {
            page: PageIdx(0),
            pixel_width: 0,
            pixel_height: 5,
            thumbnail: false,
            generation: 0,
        };
        let long_query = Job::Find {
            search: 0,
            query: "x".repeat(MAX_QUERY_CHARS + 1),
            first: PageIdx(0),
            count: 1,
        };
        for job in [big_render, empty_render, long_query] {
            let frame = encode_request(1, DocId(1), &job).unwrap();
            assert!(decode_request(&frame).is_err(), "{job:?}");
        }
        let garbage = Frame {
            head: b"not json".to_vec(),
            payload: Vec::new(),
        };
        assert!(decode_request(&garbage).is_err());
    }

    #[test]
    fn an_opened_document_and_its_outline_round_trip() {
        let opened = Opened {
            page_count: 10,
            first_page: PageSize::LETTER,
            outline: vec![OutlineItem {
                title: "Intro".to_string(),
                depth: 1,
                page: PageIdx(4),
            }],
        };
        let back = round_trip(&Reply::Opened(Ok(opened.clone())), &Expect::Open).unwrap();
        assert!(matches!(back, Some(Reply::Opened(Ok(got))) if got == opened));
        let failure = Reply::Opened(Err(OpenError::PasswordRequired));
        let back = round_trip(&failure, &Expect::Open).unwrap();
        assert!(matches!(
            back,
            Some(Reply::Opened(Err(OpenError::PasswordRequired)))
        ));
    }

    #[test]
    fn an_opened_document_with_bad_numbers_is_refused() {
        let bad = |opened: Opened| round_trip(&Reply::Opened(Ok(opened)), &Expect::Open);
        let base = Opened {
            page_count: 10,
            first_page: PageSize::LETTER,
            outline: Vec::new(),
        };
        assert!(
            bad(Opened {
                page_count: MAX_PAGES + 1,
                ..base.clone()
            })
            .is_err()
        );
        assert!(
            bad(Opened {
                page_count: 0,
                ..base.clone()
            })
            .is_err()
        );
        assert!(
            bad(Opened {
                first_page: PageSize {
                    width: f32::MAX,
                    height: 1.0,
                },
                ..base.clone()
            })
            .is_err()
        );
        let item = |title: String, depth: u8, page: usize| OutlineItem {
            title,
            depth,
            page: PageIdx(page),
        };
        for outline in [
            vec![item("x".repeat(MAX_TITLE_CHARS + 1), 0, 0)],
            vec![item("a".to_string(), MAX_OUTLINE_DEPTH + 1, 0)],
            vec![item("a".to_string(), 0, 10)],
            (0..=MAX_OUTLINE_ITEMS)
                .map(|_| item("a".to_string(), 0, 0))
                .collect(),
        ] {
            assert!(
                bad(Opened {
                    outline,
                    ..base.clone()
                })
                .is_err()
            );
        }
    }

    #[test]
    fn a_host_cannot_claim_to_be_missing_or_stopped() {
        for failure in [
            OpenError::HostMissing,
            OpenError::HostStopped,
            OpenError::HostFailed("x".into()),
            OpenError::LibraryFailed("x".repeat(MAX_MESSAGE_CHARS + 1)),
        ] {
            let reply = Reply::Opened(Err(failure));
            assert!(round_trip(&reply, &Expect::Open).is_err());
        }
    }

    #[test]
    fn a_bitmap_round_trips_with_its_raw_bytes() {
        let back = round_trip(&rendered(2, 3, 24), &render_expect()).unwrap();
        let Some(Reply::Rendered {
            bitmap: Some(bitmap),
            ..
        }) = back
        else {
            panic!("a bitmap expected");
        };
        assert_eq!((bitmap.width, bitmap.height), (2, 3));
        assert_eq!(bitmap.bgra, vec![9; 24]);
    }

    #[test]
    fn a_failed_render_has_no_bitmap_and_no_payload() {
        let failed = Reply::Rendered {
            page: PageIdx(2),
            pixel_width: 2,
            thumbnail: false,
            generation: 5,
            bitmap: None,
        };
        let back = round_trip(&failed, &render_expect()).unwrap();
        assert!(matches!(back, Some(Reply::Rendered { bitmap: None, .. })));
    }

    #[test]
    fn a_bitmap_that_lies_about_its_size_is_refused() {
        let expect = render_expect();
        assert!(round_trip(&rendered(2, 3, 23), &expect).is_err());
        assert!(round_trip(&rendered(2, 3, 25), &expect).is_err());
        assert!(round_trip(&rendered(0, 3, 0), &expect).is_err());
        assert!(round_trip(&rendered(u32::MAX, u32::MAX, 4), &expect).is_err());
        let wrong_page = Expect::Render {
            page: PageIdx(9),
            pixel_width: 2,
            thumbnail: false,
            generation: 5,
        };
        assert!(round_trip(&rendered(2, 3, 24), &wrong_page).is_err());
        let wrong_generation = Expect::Render {
            page: PageIdx(2),
            pixel_width: 2,
            thumbnail: false,
            generation: 6,
        };
        assert!(round_trip(&rendered(2, 3, 24), &wrong_generation).is_err());
    }

    #[test]
    fn page_sizes_must_be_finite_and_positive() {
        let good = Reply::Sizes(vec![PageSize::LETTER; 3]);
        assert!(round_trip(&good, &Expect::Sizes).unwrap().is_some());
        for size in [
            PageSize {
                width: 0.0,
                height: 5.0,
            },
            PageSize {
                width: -1.0,
                height: 5.0,
            },
            PageSize {
                width: 5.0,
                height: MAX_PAGE_POINTS * 2.0,
            },
        ] {
            assert!(round_trip(&Reply::Sizes(vec![size]), &Expect::Sizes).is_err());
        }
    }

    #[test]
    fn page_text_round_trips_and_its_links_are_rechecked() {
        let rect = PageRect {
            left: 1.0,
            top: 2.0,
            right: 3.0,
            bottom: 4.0,
        };
        let content = |target: LinkTarget, chars: usize| PageContent {
            chars: vec![CharBox { ch: 'a', rect }; chars],
            links: vec![PageLink { rect, target }],
            truncated: false,
        };
        let expect = Expect::Content { page: PageIdx(1) };
        let send = |content| {
            round_trip(
                &Reply::Content {
                    page: PageIdx(1),
                    content,
                },
                &expect,
            )
        };
        let ok = content(LinkTarget::Web("https://example.com".into()), 3);
        assert!(matches!(
            send(ok.clone()).unwrap(),
            Some(Reply::Content { content, .. }) if content == ok
        ));
        assert!(send(content(LinkTarget::Web("file:///c:/x".into()), 1)).is_err());
        assert!(send(content(LinkTarget::Page(PageIdx(MAX_PAGES)), 1)).is_err());
        assert!(send(content(LinkTarget::Page(PageIdx(1)), MAX_CHARS + 1)).is_err());
        let mut bad_rect = content(LinkTarget::Page(PageIdx(1)), 1);
        bad_rect.chars[0].rect.left = f32::INFINITY;
        assert!(send(bad_rect).is_err());
    }

    #[test]
    fn search_matches_must_belong_to_the_pages_asked() {
        let expect = Expect::Find {
            search: 3,
            first: PageIdx(10),
            count: 5,
        };
        let found = |page: usize, start: usize, len: usize, next: Option<usize>| Reply::Found {
            search: 3,
            matches: vec![FindMatch {
                page: PageIdx(page),
                span: CharSpan { start, len },
            }],
            next: next.map(PageIdx),
        };
        assert!(
            round_trip(&found(12, 0, 3, Some(15)), &expect)
                .unwrap()
                .is_some()
        );
        assert!(round_trip(&found(9, 0, 3, None), &expect).is_err());
        assert!(round_trip(&found(15, 0, 3, None), &expect).is_err());
        assert!(round_trip(&found(12, 0, 0, None), &expect).is_err());
        assert!(round_trip(&found(12, MAX_CHARS, 1, None), &expect).is_err());
        assert!(round_trip(&found(12, 0, 3, Some(99)), &expect).is_err());
    }

    #[test]
    fn a_reply_of_the_wrong_kind_or_with_no_match_is_refused() {
        let sizes = Reply::Sizes(vec![PageSize::LETTER]);
        assert!(round_trip(&sizes, &Expect::Open).is_err());
        assert!(round_trip(&sizes, &render_expect()).is_err());
        let done = encode_reply(1, None).unwrap();
        assert!(decode_reply(&done, &Expect::Open).is_err());
        assert!(decode_reply(&done, &Expect::Sizes).unwrap().is_none());
        let garbage = Frame {
            head: b"{\"id\": \"no\"}".to_vec(),
            payload: Vec::new(),
        };
        assert!(decode_reply(&garbage, &Expect::Sizes).is_err());
    }
}
