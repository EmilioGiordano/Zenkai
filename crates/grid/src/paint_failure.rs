use std::sync::atomic::{AtomicBool, Ordering};

// Painting runs every frame and release builds log to a file, so a failure that repeats
// must be logged once per kind, not once per frame.
#[derive(Clone, Copy)]
pub enum PaintFailure {
    CellText,
    WrappedShape,
    WrappedText,
    HeaderLabel,
    EditorText,
}

static CELL_TEXT: AtomicBool = AtomicBool::new(false);
static WRAPPED_SHAPE: AtomicBool = AtomicBool::new(false);
static WRAPPED_TEXT: AtomicBool = AtomicBool::new(false);
static HEADER_LABEL: AtomicBool = AtomicBool::new(false);
static EDITOR_TEXT: AtomicBool = AtomicBool::new(false);

impl PaintFailure {
    pub fn first_occurrence(self) -> bool {
        let reported = match self {
            Self::CellText => &CELL_TEXT,
            Self::WrappedShape => &WRAPPED_SHAPE,
            Self::WrappedText => &WRAPPED_TEXT,
            Self::HeaderLabel => &HEADER_LABEL,
            Self::EditorText => &EDITOR_TEXT,
        };
        !reported.swap(true, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_is_reported_only_the_first_time() {
        assert!(PaintFailure::HeaderLabel.first_occurrence());
        assert!(!PaintFailure::HeaderLabel.first_occurrence());
        assert!(PaintFailure::EditorText.first_occurrence());
        assert!(!PaintFailure::EditorText.first_occurrence());
    }
}
