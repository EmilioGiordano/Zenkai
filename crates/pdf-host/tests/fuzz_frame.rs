// Structured fuzzing of what the app reads from the PDF host (and the host from the app): a
// stream of valid frames is damaged at random and read back, and the head and payload of
// valid messages are damaged and decoded. Every answer is a value or an error, quickly, and
// a frame never holds more bytes than the stream had.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../test-support/json_mutation.rs"]
mod json_mutation;

use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use serde_json::Value;
use zenkai_pdf_host::codec::{
    Expect, check_request, decode_reply, decode_request, encode_reply, encode_request, reply_id,
};
use zenkai_pdf_host::frame::{
    Frame, MAX_HEADER_BYTES, MAX_PAYLOAD_BYTES, read_frame, write_frame,
};
use zenkai_pdf_host::wire::{
    Bitmap, BrokenFile, CharBox, CharSpan, DocId, FindMatch, Job, LinkTarget, MAX_CHARS,
    MAX_MATCHES, MAX_PAGES, MAX_RENDER_PIXELS, OpenError, Opened, OutlineItem, PageContent,
    PageIdx, PageLink, PageRect, PageSize, Reply,
};

const TIME_LIMIT: Duration = Duration::from_secs(5);
const PREFIX_BYTES: usize = 12;

fn jobs() -> Vec<Job> {
    vec![
        Job::Open {
            path: PathBuf::from("C:\\Docs\\a.pdf"),
            password: Some("secret".to_string()),
        },
        Job::Sizes,
        Job::Render {
            page: PageIdx(2),
            pixel_width: 2,
            pixel_height: 2,
            thumbnail: false,
            generation: 5,
        },
        Job::Content { page: PageIdx(2) },
        Job::Find {
            search: 3,
            query: "needle".to_string(),
            first: PageIdx(0),
            count: 5,
        },
        Job::Close,
    ]
}

fn expectations() -> Vec<Expect> {
    jobs().iter().map(Expect::of).collect()
}

fn rect() -> PageRect {
    PageRect {
        left: 1.0,
        top: 2.0,
        right: 30.0,
        bottom: 40.0,
    }
}

fn replies() -> Vec<Option<Reply>> {
    vec![
        None,
        Some(Reply::Stopped),
        Some(Reply::Opened(Ok(Opened {
            page_count: 10,
            first_page: PageSize::LETTER,
            outline: vec![
                OutlineItem {
                    title: "Intro".to_string(),
                    depth: 0,
                    page: PageIdx(1),
                },
                OutlineItem {
                    title: "Detail".to_string(),
                    depth: 1,
                    page: PageIdx(4),
                },
            ],
        }))),
        Some(Reply::Opened(Err(OpenError::PasswordRequired))),
        Some(Reply::Opened(Err(OpenError::Broken(
            BrokenFile::Unreadable("bad xref".to_string()),
        )))),
        Some(Reply::Sizes(vec![PageSize::LETTER; 3])),
        Some(Reply::Rendered {
            page: PageIdx(2),
            pixel_width: 2,
            thumbnail: false,
            generation: 5,
            bitmap: Some(Bitmap {
                width: 2,
                height: 2,
                bgra: (0..16).collect(),
            }),
        }),
        Some(Reply::Rendered {
            page: PageIdx(2),
            pixel_width: 2,
            thumbnail: false,
            generation: 5,
            bitmap: None,
        }),
        Some(Reply::Content {
            page: PageIdx(2),
            content: PageContent {
                chars: vec![
                    CharBox {
                        ch: 'a',
                        rect: rect(),
                    },
                    CharBox {
                        ch: 'ñ',
                        rect: rect(),
                    },
                ],
                links: vec![
                    PageLink {
                        rect: rect(),
                        target: LinkTarget::Page(PageIdx(3)),
                    },
                    PageLink {
                        rect: rect(),
                        target: LinkTarget::Web("https://example.com/".to_string()),
                    },
                ],
                truncated: false,
            },
        }),
        Some(Reply::Found {
            search: 3,
            matches: vec![FindMatch {
                page: PageIdx(1),
                span: CharSpan { start: 4, len: 6 },
            }],
            next: Some(PageIdx(5)),
        }),
    ]
}

fn valid_frames() -> Vec<Frame> {
    let mut frames: Vec<Frame> = jobs()
        .iter()
        .enumerate()
        .map(|(id, job)| encode_request(id as u64, DocId(1), job).unwrap())
        .collect();
    frames.extend(
        replies()
            .into_iter()
            .enumerate()
            .map(|(id, reply)| encode_reply(id as u64, reply).unwrap()),
    );
    frames
}

fn stream(frames: &[Frame]) -> (Vec<u8>, Vec<usize>) {
    let mut bytes = Vec::new();
    let mut starts = Vec::new();
    for frame in frames {
        starts.push(bytes.len());
        write_frame(&mut bytes, frame).unwrap();
    }
    (bytes, starts)
}

#[derive(Clone, Debug)]
enum Damage {
    Flip(prop::sample::Index, u8),
    Truncate(prop::sample::Index),
    Insert(prop::sample::Index, Vec<u8>),
    Duplicate(prop::sample::Index, usize),
    Length(prop::sample::Index, bool, prop::sample::Index),
}

fn damage() -> impl Strategy<Value = Damage> {
    prop_oneof![
        (any::<prop::sample::Index>(), 0u8..8).prop_map(|(at, bit)| Damage::Flip(at, bit)),
        any::<prop::sample::Index>().prop_map(Damage::Truncate),
        (
            any::<prop::sample::Index>(),
            prop::collection::vec(any::<u8>(), 1..24)
        )
            .prop_map(|(at, bytes)| Damage::Insert(at, bytes)),
        (any::<prop::sample::Index>(), 1usize..200)
            .prop_map(|(at, length)| Damage::Duplicate(at, length)),
        (
            any::<prop::sample::Index>(),
            any::<bool>(),
            any::<prop::sample::Index>()
        )
            .prop_map(|(frame, in_header, value)| Damage::Length(frame, in_header, value)),
    ]
}

fn boundary_lengths() -> [u32; 12] {
    [
        0,
        1,
        MAX_HEADER_BYTES as u32 - 1,
        MAX_HEADER_BYTES as u32,
        MAX_HEADER_BYTES as u32 + 1,
        MAX_PAYLOAD_BYTES as u32 - 1,
        MAX_PAYLOAD_BYTES as u32,
        MAX_PAYLOAD_BYTES as u32 + 1,
        i32::MAX as u32,
        1 << 31,
        u32::MAX - 1,
        u32::MAX,
    ]
}

fn apply(mut bytes: Vec<u8>, starts: &[usize], damage: &Damage) -> Vec<u8> {
    if bytes.is_empty() {
        return bytes;
    }
    match damage {
        Damage::Flip(at, bit) => {
            let at = at.index(bytes.len());
            bytes[at] ^= 1 << bit;
        }
        Damage::Truncate(at) => bytes.truncate(at.index(bytes.len())),
        Damage::Insert(at, extra) => {
            let at = at.index(bytes.len() + 1);
            bytes.splice(at..at, extra.iter().copied());
        }
        Damage::Duplicate(at, length) => {
            let at = at.index(bytes.len());
            let end = (at + length).min(bytes.len());
            let copy = bytes[at..end].to_vec();
            bytes.splice(at..at, copy);
        }
        Damage::Length(frame, in_header, value) => {
            let start = starts[frame.index(starts.len())];
            let field = start + if *in_header { 4 } else { 8 };
            let lengths = boundary_lengths();
            let length = lengths[value.index(lengths.len())];
            if field + 4 <= bytes.len() {
                bytes[field..field + 4].copy_from_slice(&length.to_le_bytes());
            }
        }
    }
    bytes
}

// Hands out one to three bytes a read and an interruption now and then, like a slow pipe.
struct Trickle<'a> {
    bytes: &'a [u8],
    turn: usize,
}

impl Read for Trickle<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.turn += 1;
        if self.turn.is_multiple_of(7) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let count = (self.turn % 3 + 1).min(buffer.len()).min(self.bytes.len());
        buffer[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}

// Fails once `good` bytes were read, like a pipe whose other end was killed.
struct Breaking<'a> {
    bytes: &'a [u8],
    good: usize,
}

impl Read for Breaking<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.good == 0 {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let count = buffer.len().min(self.bytes.len()).min(self.good);
        buffer[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        self.good -= count;
        Ok(count)
    }
}

fn read_all(mut reader: impl Read, length: usize) -> Vec<Frame> {
    let mut frames = Vec::new();
    // Every frame takes at least its prefix, so more reads than that is a loop.
    for _ in 0..=length / PREFIX_BYTES + 1 {
        match read_frame(&mut reader) {
            Ok(Some(frame)) => frames.push(frame),
            Ok(None) | Err(_) => return frames,
        }
    }
    panic!("the reader kept producing frames out of {length} bytes");
}

fn decode_everything(frame: &Frame) {
    if let Ok((_, _, job)) = decode_request(frame) {
        let _ = check_request(&job);
    }
    let _ = reply_id(frame);
    for expect in expectations() {
        if let Ok(Some(reply)) = decode_reply(frame.clone(), &expect) {
            check_reply(&reply);
        }
    }
}

fn check_reply(reply: &Reply) {
    match reply {
        Reply::Opened(Ok(opened)) => {
            assert!((1..=MAX_PAGES).contains(&opened.page_count));
            assert!(
                opened
                    .outline
                    .iter()
                    .all(|item| item.page.0 < opened.page_count)
            );
        }
        Reply::Sizes(sizes) => {
            assert!(sizes.len() <= MAX_PAGES);
            assert!(
                sizes
                    .iter()
                    .all(|size| size.width > 0.0 && size.height > 0.0)
            );
        }
        Reply::Rendered {
            bitmap: Some(bitmap),
            ..
        } => {
            let pixels = u64::from(bitmap.width) * u64::from(bitmap.height);
            assert!(pixels > 0 && pixels <= MAX_RENDER_PIXELS);
            assert_eq!(bitmap.bgra.len() as u64, pixels * 4);
        }
        Reply::Content { content, .. } => assert!(content.chars.len() <= MAX_CHARS),
        Reply::Found { matches, .. } => assert!(matches.len() <= MAX_MATCHES),
        _ => {}
    }
}

fn check_stream(bytes: &[u8]) {
    let started = Instant::now();
    let whole = read_all(bytes, bytes.len());
    let held: usize = whole
        .iter()
        .map(|frame| PREFIX_BYTES + frame.head.len() + frame.payload.len())
        .sum();
    assert!(
        held <= bytes.len(),
        "frames hold {held} of {} bytes",
        bytes.len()
    );
    for frame in &whole {
        assert!(frame.head.len() <= MAX_HEADER_BYTES);
        assert!(frame.payload.len() <= MAX_PAYLOAD_BYTES);
        decode_everything(frame);
    }
    let slow = read_all(Trickle { bytes, turn: 0 }, bytes.len());
    assert_eq!(slow, whole, "a slow pipe reads other frames");
    let broken = read_all(
        Breaking {
            bytes,
            good: bytes.len() / 2,
        },
        bytes.len(),
    );
    assert!(broken.len() <= whole.len());
    assert!(
        started.elapsed() < TIME_LIMIT,
        "reading took {:?}",
        started.elapsed()
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    #[test]
    fn a_damaged_frame_stream_reads_or_fails(
        damages in prop::collection::vec(damage(), 1..5),
    ) {
        let (mut bytes, starts) = stream(&valid_frames());
        for step in &damages {
            bytes = apply(bytes, &starts, step);
        }
        check_stream(&bytes);
    }
}

fn byte_damage() -> impl Strategy<Value = (prop::sample::Index, u8, u8)> {
    (any::<prop::sample::Index>(), 0u8..4, any::<u8>())
}

fn damage_bytes(mut bytes: Vec<u8>, (at, how, byte): (prop::sample::Index, u8, u8)) -> Vec<u8> {
    if bytes.is_empty() {
        return vec![byte];
    }
    let at = at.index(bytes.len());
    match how {
        0 => bytes.truncate(at),
        1 => {
            bytes.remove(at);
        }
        2 => bytes.insert(at, byte),
        _ => bytes[at] = byte,
    }
    bytes
}

fn variants(original: Vec<u8>) -> BoxedStrategy<Vec<u8>> {
    let raw = byte_damage()
        .prop_map({
            let original = original.clone();
            move |damage| damage_bytes(original.clone(), damage)
        })
        .boxed();
    match serde_json::from_slice::<Value>(&original) {
        Ok(value) => prop_oneof![Just(original), json_mutation::mutated(value), raw].boxed(),
        Err(_) => prop_oneof![Just(original), raw].boxed(),
    }
}

fn damaged_message() -> impl Strategy<Value = Frame> {
    let one_of_each: Vec<BoxedStrategy<Frame>> = valid_frames()
        .into_iter()
        .map(|frame| {
            (variants(frame.head), variants(frame.payload))
                .prop_map(|(head, payload)| Frame { head, payload })
                .boxed()
        })
        .collect();
    prop::strategy::Union::new(one_of_each)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    #[test]
    fn a_damaged_message_decodes_or_is_refused(frame in damaged_message()) {
        let started = Instant::now();
        decode_everything(&frame);
        prop_assert!(started.elapsed() < TIME_LIMIT);
    }
}

#[test]
fn a_frame_announcing_the_most_it_may_but_ending_early_is_truncated() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"ZPH1");
    bytes.extend_from_slice(&(MAX_HEADER_BYTES as u32).to_le_bytes());
    bytes.extend_from_slice(&(MAX_PAYLOAD_BYTES as u32).to_le_bytes());
    bytes.extend_from_slice(&[7; 100]);
    assert!(read_frame(&mut bytes.as_slice()).is_err());
}
