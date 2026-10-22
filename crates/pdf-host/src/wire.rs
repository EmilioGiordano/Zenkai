use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use async_channel::Sender;
use serde::{Deserialize, Serialize};

// Every number the PDF host may hand the app is bounded by one of these. They are the limits
// of the viewer too: a file past them is refused or cut, whatever the host says.
pub const MAX_PAGES: usize = 100_000;
pub const MAX_RENDER_PIXELS: u64 = 16_200_000;
pub const MAX_OUTLINE_ITEMS: usize = 2000;
pub const MAX_OUTLINE_DEPTH: u8 = 8;
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_CHARS: usize = 200_000;
pub const MAX_LINKS: usize = 2000;
pub const MAX_MATCHES: usize = 5000;
pub const MAX_URI_BYTES: usize = 2048;
pub const MAX_MESSAGE_CHARS: usize = 1000;
pub const MAX_PAGE_POINTS: f32 = 200_000.0;
pub const MAX_RECT_POINTS: f32 = 400_000.0;
pub const MAX_QUERY_CHARS: usize = 1000;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PageIdx(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

impl PageSize {
    pub const LETTER: PageSize = PageSize {
        width: 612.0,
        height: 792.0,
    };

    // A size the viewer can lay out: a side that is not a positive number becomes a letter
    // side, and one past the limit is cut to it.
    pub fn sanitized(self) -> PageSize {
        let side = |value: f32, fallback: f32| {
            if value.is_finite() && value > 0.0 {
                value.min(MAX_PAGE_POINTS)
            } else {
                fallback
            }
        };
        PageSize {
            width: side(self.width, PageSize::LETTER.width),
            height: side(self.height, PageSize::LETTER.height),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenError {
    LibraryMissing,
    LibraryFailed(String),
    HostMissing,
    HostFailed(String),
    HostStopped,
    PasswordRequired,
    WrongPassword,
    Broken(BrokenFile),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrokenFile {
    NotAPdf,
    Unreadable(String),
    NoPages,
    TooManyPages,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutlineItem {
    pub title: String,
    pub depth: u8,
    pub page: PageIdx,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Opened {
    pub page_count: usize,
    pub first_page: PageSize,
    pub outline: Vec<OutlineItem>,
}

// A rectangle in PDF points measured from the top left of the page as displayed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PageRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl PageRect {
    // Keeps a box that pdfium got wrong from reaching the app: a coordinate that is not a
    // number is zero, and one past the limit is cut to it.
    pub fn sanitized(self) -> PageRect {
        let tame = |value: f32| {
            if value.is_finite() {
                value.clamp(-MAX_RECT_POINTS, MAX_RECT_POINTS)
            } else {
                0.0
            }
        };
        PageRect {
            left: tame(self.left),
            top: tame(self.top),
            right: tame(self.right),
            bottom: tame(self.bottom),
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }

    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    None,
    Quarter,
    Half,
    ThreeQuarters,
}

// Pdfium reports text and link boxes in the page's own space: origin at the bottom left of
// the crop box, not turned. `shown` is the size after the page's rotation.
pub fn to_page_rect(
    (left, bottom, right, top): (f32, f32, f32, f32),
    origin: (f32, f32),
    shown: PageSize,
    turn: Turn,
) -> PageRect {
    let (left, right) = (left - origin.0, right - origin.0);
    let (bottom, top) = (bottom - origin.1, top - origin.1);
    let (unturned_width, unturned_height) = match turn {
        Turn::None | Turn::Half => (shown.width, shown.height),
        Turn::Quarter | Turn::ThreeQuarters => (shown.height, shown.width),
    };
    let (x_low, x_high, y_low, y_high) = match turn {
        Turn::None => (left, right, unturned_height - top, unturned_height - bottom),
        Turn::Quarter => (bottom, top, left, right),
        Turn::Half => (unturned_width - right, unturned_width - left, bottom, top),
        Turn::ThreeQuarters => (
            unturned_height - top,
            unturned_height - bottom,
            unturned_width - right,
            unturned_width - left,
        ),
    };
    PageRect {
        left: x_low.min(x_high),
        right: x_low.max(x_high),
        top: y_low.min(y_high),
        bottom: y_low.max(y_high),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharBox {
    pub ch: char,
    pub rect: PageRect,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LinkTarget {
    Page(PageIdx),
    Web(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageLink {
    pub rect: PageRect,
    pub target: LinkTarget,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PageContent {
    pub chars: Vec<CharBox>,
    pub links: Vec<PageLink>,
    // The page had more chars than are kept, so search and copy miss the rest.
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharSpan {
    pub start: usize,
    pub len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindMatch {
    pub page: PageIdx,
    pub span: CharSpan,
}

#[derive(Clone)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl std::fmt::Debug for Bitmap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Bitmap({}x{})", self.width, self.height)
    }
}

// A request to the host. A render carries a counter shared with the view: when the view
// moves on, the counter changes and a render still queued is dropped before it is sent.
pub struct Request {
    pub doc: DocId,
    pub reply: Sender<Reply>,
    pub job: Job,
    pub token: Option<Arc<AtomicU64>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Job {
    Open {
        path: PathBuf,
        password: Option<String>,
    },
    Sizes,
    Render {
        page: PageIdx,
        pixel_width: u32,
        pixel_height: u32,
        thumbnail: bool,
        generation: u64,
    },
    Content {
        page: PageIdx,
    },
    Find {
        search: u64,
        query: String,
        first: PageIdx,
        count: usize,
    },
    Close,
    // Makes the host abort, to test that the app survives it. The host honours it only when
    // its environment says it runs under a test.
    Abort,
}

#[derive(Clone, Debug)]
pub enum Reply {
    Opened(Result<Opened, OpenError>),
    Sizes(Vec<PageSize>),
    Rendered {
        page: PageIdx,
        pixel_width: u32,
        thumbnail: bool,
        generation: u64,
        bitmap: Option<Bitmap>,
    },
    Content {
        page: PageIdx,
        content: PageContent,
    },
    Found {
        search: u64,
        matches: Vec<FindMatch>,
        next: Option<PageIdx>,
    },
    // The host is gone; every open document is lost with it.
    Stopped,
}

// A page number from pdfium, if it names a page of a document of `count` pages.
pub fn page_within(index: i64, count: usize) -> Option<PageIdx> {
    usize::try_from(index)
        .ok()
        .filter(|index| *index < count)
        .map(PageIdx)
}

// Messages that come from the system are cut so they fit what the app accepts.
pub fn short_message(text: String) -> String {
    if text.chars().count() <= MAX_MESSAGE_CHARS {
        text
    } else {
        text.chars().take(MAX_MESSAGE_CHARS).collect()
    }
}

// The only links a PDF may ask to open: plain web addresses, never a file, script or app.
pub fn web_url(uri: &str) -> Option<String> {
    let uri = uri.trim();
    let lowered = uri.to_ascii_lowercase();
    let web = lowered.starts_with("http://") || lowered.starts_with("https://");
    (web && uri.len() <= MAX_URI_BYTES && !uri.chars().any(char::is_control))
        .then(|| uri.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOWN: PageSize = PageSize {
        width: 200.0,
        height: 100.0,
    };

    fn rect(values: (f32, f32, f32, f32), turn: Turn, shown: PageSize) -> PageRect {
        to_page_rect(values, (0.0, 0.0), shown, turn)
    }

    #[test]
    fn an_unturned_page_flips_the_vertical_axis() {
        let found = rect((10.0, 20.0, 50.0, 40.0), Turn::None, SHOWN);
        assert_eq!(
            found,
            PageRect {
                left: 10.0,
                right: 50.0,
                top: 60.0,
                bottom: 80.0
            }
        );
    }

    #[test]
    fn the_crop_box_origin_is_taken_off_first() {
        let found = to_page_rect((15.0, 25.0, 55.0, 45.0), (5.0, 5.0), SHOWN, Turn::None);
        assert_eq!(found, rect((10.0, 20.0, 50.0, 40.0), Turn::None, SHOWN));
    }

    #[test]
    fn a_quarter_turn_swaps_the_axes() {
        let shown = PageSize {
            width: 100.0,
            height: 200.0,
        };
        let found = rect((10.0, 20.0, 50.0, 40.0), Turn::Quarter, shown);
        assert_eq!(
            found,
            PageRect {
                left: 20.0,
                right: 40.0,
                top: 10.0,
                bottom: 50.0
            }
        );
    }

    #[test]
    fn a_half_turn_mirrors_the_horizontal_axis() {
        let found = rect((10.0, 20.0, 50.0, 40.0), Turn::Half, SHOWN);
        assert_eq!(
            found,
            PageRect {
                left: 150.0,
                right: 190.0,
                top: 20.0,
                bottom: 40.0
            }
        );
    }

    #[test]
    fn three_quarters_turn_stays_inside_the_shown_page() {
        let shown = PageSize {
            width: 100.0,
            height: 200.0,
        };
        let found = rect((10.0, 20.0, 50.0, 40.0), Turn::ThreeQuarters, shown);
        assert!(found.left >= 0.0 && found.right <= shown.width);
        assert!(found.top >= 0.0 && found.bottom <= shown.height);
        assert!(!found.is_empty());
    }

    #[test]
    fn a_rect_contains_its_edges_and_is_empty_when_flat() {
        let found = PageRect {
            left: 1.0,
            top: 1.0,
            right: 3.0,
            bottom: 2.0,
        };
        assert!(found.contains(1.0, 2.0));
        assert!(!found.contains(3.1, 1.5));
        assert!(PageRect::default().is_empty());
    }

    #[test]
    fn a_page_size_or_box_from_pdfium_is_made_usable() {
        let size = PageSize {
            width: 0.0,
            height: f32::MAX,
        }
        .sanitized();
        assert_eq!(size.width, PageSize::LETTER.width);
        assert_eq!(size.height, MAX_PAGE_POINTS);
        let nan = PageSize {
            width: f32::NAN,
            height: -3.0,
        }
        .sanitized();
        assert_eq!(nan, PageSize::LETTER);
        let rect = PageRect {
            left: f32::NAN,
            top: -1e9,
            right: 1e9,
            bottom: f32::INFINITY,
        }
        .sanitized();
        assert_eq!(rect.left, 0.0);
        assert_eq!(rect.top, -MAX_RECT_POINTS);
        assert_eq!(rect.right, MAX_RECT_POINTS);
        assert_eq!(rect.bottom, 0.0);
    }

    #[test]
    fn a_page_number_must_name_a_page_of_the_document() {
        assert_eq!(page_within(0, 3), Some(PageIdx(0)));
        assert_eq!(page_within(2, 3), Some(PageIdx(2)));
        assert_eq!(page_within(3, 3), None);
        assert_eq!(page_within(-1, 3), None);
        assert_eq!(page_within(i64::MAX, 3), None);
    }

    #[test]
    fn a_long_system_message_is_cut_to_the_limit() {
        let long = "x".repeat(MAX_MESSAGE_CHARS + 50);
        assert_eq!(short_message(long).chars().count(), MAX_MESSAGE_CHARS);
        assert_eq!(short_message("short".to_string()), "short");
    }

    #[test]
    fn only_plain_web_addresses_are_offered_to_the_browser() {
        assert_eq!(
            web_url(" https://example.com/a?b=1 "),
            Some("https://example.com/a?b=1".to_string())
        );
        assert!(web_url("HTTP://example.com").is_some());
        for refused in [
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "mailto:a@b.c",
            "C:\\a.exe",
            "https://exa\nmple.com",
            "",
        ] {
            assert_eq!(web_url(refused), None, "{refused}");
        }
        let long = format!("https://a.com/{}", "x".repeat(MAX_URI_BYTES));
        assert_eq!(web_url(&long), None);
    }
}
