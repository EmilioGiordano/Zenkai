use std::borrow::Cow;

use gpui_kit::assets::AllAssets;
use gpui_kit::*;

pub const CLAUDE_MARK: &str = "brands/claude.svg";
pub const GEMINI_MARK: &str = "brands/gemini.svg";

const BRAND_MARKS: [(&str, &[u8]); 2] = [
    (CLAUDE_MARK, include_bytes!("../assets/brands/claude.svg")),
    (GEMINI_MARK, include_bytes!("../assets/brands/gemini.svg")),
];

// The component icons plus the few brand marks the AI settings show.
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
