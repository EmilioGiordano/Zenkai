use gpui_kit::*;
use zenkai_grid::HighContrast;

use crate::space_appearance::SpaceAppearance;
use crate::space_parts::{Contrast, Fill};

impl Global for SpaceAppearance {}

pub fn current(cx: &App) -> SpaceAppearance {
    cx.try_global::<SpaceAppearance>()
        .copied()
        .unwrap_or_default()
}

pub fn change(cx: &mut App, update: impl FnOnce(&mut SpaceAppearance)) {
    let mut appearance = current(cx);
    update(&mut appearance);
    cx.set_global(appearance);
}

pub fn contrast(cx: &App) -> Contrast {
    if cx.try_global::<HighContrast>().is_some_and(|high| high.0) {
        Contrast::High
    } else {
        Contrast::Normal
    }
}

pub fn paint(fill: Fill) -> Hsla {
    Hsla::from(rgb(fill.rgb)).opacity(fill.alpha)
}
