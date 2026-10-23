use super::*;
use crate::wire::{CharSpan, PageLink};

fn round_trip(reply: &Reply, expect: &Expect) -> Result<Option<Reply>, CodecError> {
    let frame = encode_reply(7, Some(reply.clone())).unwrap();
    assert_eq!(reply_id(&frame).unwrap(), 7);
    decode_reply(frame, expect)
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
    assert!(decode_reply(done.clone(), &Expect::Open).is_err());
    assert!(
        decode_reply(done.clone(), &Expect::Sizes)
            .unwrap()
            .is_none()
    );
    let garbage = Frame {
        head: b"{\"id\": \"no\"}".to_vec(),
        payload: Vec::new(),
    };
    assert!(decode_reply(garbage, &Expect::Sizes).is_err());
}

// What the host makes of values pdfium got wrong has to pass the checks of the app, or one
// damaged file would stop the shared host.
#[test]
fn values_out_of_range_on_the_host_side_still_make_a_valid_reply() {
    let sizes = Reply::Sizes(
        [
            (0.0, 5.0),
            (f32::MAX, f32::MAX),
            (f32::NAN, -2.0),
            (f32::INFINITY, 10.0),
        ]
        .map(|(width, height)| PageSize { width, height }.sanitized())
        .to_vec(),
    );
    assert!(round_trip(&sizes, &Expect::Sizes).unwrap().is_some());

    let wild = PageRect {
        left: f32::NAN,
        top: -1e12,
        right: 1e12,
        bottom: f32::NEG_INFINITY,
    }
    .sanitized();
    let content = PageContent {
        chars: vec![CharBox {
            ch: 'a',
            rect: wild,
        }],
        links: vec![PageLink {
            rect: wild,
            target: LinkTarget::Page(crate::wire::page_within(7, 10).unwrap()),
        }],
        truncated: false,
    };
    let expect = Expect::Content { page: PageIdx(0) };
    let reply = Reply::Content {
        page: PageIdx(0),
        content,
    };
    assert!(round_trip(&reply, &expect).unwrap().is_some());

    let opened = Opened {
        page_count: 10,
        first_page: PageSize {
            width: 0.0,
            height: 0.0,
        }
        .sanitized(),
        outline: [3_i64, 99, -1]
            .into_iter()
            .filter_map(|index| crate::wire::page_within(index, 10))
            .map(|page| OutlineItem {
                title: "t".into(),
                depth: 0,
                page,
            })
            .collect(),
    };
    assert_eq!(opened.outline.len(), 1);
    assert!(
        round_trip(&Reply::Opened(Ok(opened)), &Expect::Open)
            .unwrap()
            .is_some()
    );
}
