use std::borrow::Cow;

use gpui_kit::assets::AllAssets;
use gpui_kit::*;

pub const CLAUDE_MARK: &str = "brands/claude.svg";
pub const GEMINI_MARK: &str = "brands/gemini.svg";
pub const LOCK_MARK: &str = "marks/lock.svg";
pub const LOCK_OPEN_MARK: &str = "marks/lock-open.svg";
pub const CODEX_AGENT_MARK: &str = "marks/agents/codex.svg";
pub const OPENCODE_MARK: &str = "marks/agents/opencode.svg";
pub const GROK_MARK: &str = "marks/agents/grok.svg";
pub const WORKBOOK_REFERENCE_MARK: &str = "marks/references/workbook.svg";
pub const SHEET_REFERENCE_MARK: &str = "marks/references/sheet.svg";
pub const RANGE_REFERENCE_MARK: &str = "marks/references/range.svg";

const BRAND_MARKS: [(&str, &[u8]); 10] = [
    (CLAUDE_MARK, include_bytes!("../assets/brands/claude.svg")),
    (GEMINI_MARK, include_bytes!("../assets/brands/gemini.svg")),
    (LOCK_MARK, include_bytes!("../assets/marks/lock.svg")),
    (
        LOCK_OPEN_MARK,
        include_bytes!("../assets/marks/lock-open.svg"),
    ),
    (
        CODEX_AGENT_MARK,
        include_bytes!("../assets/marks/agents/codex.svg"),
    ),
    (
        OPENCODE_MARK,
        include_bytes!("../assets/marks/agents/opencode.svg"),
    ),
    (GROK_MARK, include_bytes!("../assets/marks/agents/grok.svg")),
    (
        WORKBOOK_REFERENCE_MARK,
        include_bytes!("../assets/marks/references/workbook.svg"),
    ),
    (
        SHEET_REFERENCE_MARK,
        include_bytes!("../assets/marks/references/sheet.svg"),
    ),
    (
        RANGE_REFERENCE_MARK,
        include_bytes!("../assets/marks/references/range.svg"),
    ),
];

// The component icons plus the brand and agent marks the AI settings and chat show.
pub struct ZenkaiAssets;

impl AssetSource for ZenkaiAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match BRAND_MARKS.iter().find(|(name, _)| *name == path) {
            Some((_, bytes)) => Ok(Some(Cow::Borrowed(*bytes))),
            None => AllAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut listed = AllAssets.list(path)?;
        listed.extend(
            BRAND_MARKS
                .iter()
                .filter(|(name, _)| name.starts_with(path))
                .map(|(name, _)| SharedString::from(*name)),
        );
        Ok(listed)
    }
}
