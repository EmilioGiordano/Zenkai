use serde::{Deserialize, Serialize};

use crate::spaces::SpaceColor;
use zenkai_i18n::t;

pub const MAX_INTENSITY: u8 = 60;
const STEP: u8 = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceStyle {
    Dot,
    #[default]
    Header,
    Border,
    FullTint,
}

impl SpaceStyle {
    pub const ALL: [SpaceStyle; 4] = [
        SpaceStyle::Dot,
        SpaceStyle::Header,
        SpaceStyle::Border,
        SpaceStyle::FullTint,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SpaceStyle::Dot => t!("space.style.dot"),
            SpaceStyle::Header => t!("space.style.header"),
            SpaceStyle::Border => t!("space.style.border"),
            SpaceStyle::FullTint => t!("space.style.full_tint"),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            SpaceStyle::Dot => t!("space.style.dot_description"),
            SpaceStyle::Header => t!("space.style.header_description"),
            SpaceStyle::Border => t!("space.style.border_description"),
            SpaceStyle::FullTint => t!("space.style.full_tint_description"),
        }
    }
}

// Read from a wide number and clamped, so a value out of range never fails the whole session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "i64", into = "u8")]
pub struct Intensity(u8);

impl Intensity {
    pub fn percent(self) -> u8 {
        self.0
    }

    pub fn stepped(self, up: bool) -> Intensity {
        let value = if up {
            self.0.saturating_add(STEP)
        } else {
            self.0.saturating_sub(STEP)
        };
        Intensity::from(i64::from(value))
    }

    pub(crate) fn fraction(self) -> f32 {
        f32::from(self.0) / 100.0
    }
}

impl Default for Intensity {
    fn default() -> Self {
        Intensity(18)
    }
}

impl From<i64> for Intensity {
    fn from(percent: i64) -> Self {
        Intensity(percent.clamp(0, i64::from(MAX_INTENSITY)) as u8)
    }
}

impl From<Intensity> for u8 {
    fn from(intensity: Intensity) -> u8 {
        intensity.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "i64", into = "u8")]
pub struct Opacity(u8);

impl Opacity {
    pub const OPAQUE: Opacity = Opacity(100);

    pub fn percent(self) -> u8 {
        self.0
    }

    pub(crate) fn fraction(self) -> f32 {
        f32::from(self.0) / 100.0
    }
}

impl From<i64> for Opacity {
    fn from(percent: i64) -> Self {
        Opacity(percent.clamp(0, 100) as u8)
    }
}

impl From<Opacity> for u8 {
    fn from(opacity: Opacity) -> u8 {
        opacity.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgba {
    rgb: u32,
    pub opacity: Opacity,
}

impl Rgba {
    pub fn new(rgb: u32, opacity: Opacity) -> Rgba {
        Rgba {
            rgb: rgb & 0xFF_FF_FF,
            opacity,
        }
    }

    pub fn rgb(self) -> u32 {
        self.rgb & 0xFF_FF_FF
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyTo {
    #[default]
    Both,
    HeaderOnly,
    WorkbooksOnly,
    Neither,
}

impl ApplyTo {
    pub fn header(self) -> bool {
        matches!(self, ApplyTo::Both | ApplyTo::HeaderOnly)
    }

    pub fn workbooks(self) -> bool {
        matches!(self, ApplyTo::Both | ApplyTo::WorkbooksOnly)
    }

    pub fn of(header: bool, workbooks: bool) -> ApplyTo {
        match (header, workbooks) {
            (true, true) => ApplyTo::Both,
            (true, false) => ApplyTo::HeaderOnly,
            (false, true) => ApplyTo::WorkbooksOnly,
            (false, false) => ApplyTo::Neither,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewSpaceColor {
    #[default]
    Auto,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Look {
    #[serde(default)]
    pub style: SpaceStyle,
    #[serde(default)]
    pub intensity: Intensity,
    #[serde(default)]
    pub apply_to: ApplyTo,
}

// The default every space follows unless it has its own override.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceAppearance {
    #[serde(default)]
    pub look: Look,
    #[serde(default)]
    pub new_space_color: NewSpaceColor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Custom {
    pub look: Look,
    // None follows the color of the space (the palette).
    pub color: Option<Rgba>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SpaceOverride {
    #[default]
    Default,
    Custom(Custom),
}

impl SpaceOverride {
    pub fn custom(&self) -> Option<&Custom> {
        match self {
            SpaceOverride::Custom(custom) => Some(custom),
            SpaceOverride::Default => None,
        }
    }

    pub fn with_palette_color(self) -> SpaceOverride {
        match self {
            SpaceOverride::Custom(custom) => SpaceOverride::Custom(Custom {
                color: None,
                ..custom
            }),
            SpaceOverride::Default => SpaceOverride::Default,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub look: Look,
    pub tint: Option<Rgba>,
}

pub fn resolve(
    defaults: &SpaceAppearance,
    color: SpaceColor,
    space_override: &SpaceOverride,
) -> Resolved {
    let palette = color.rgb().map(|rgb| Rgba::new(rgb, Opacity::OPAQUE));
    match space_override {
        SpaceOverride::Default => Resolved {
            look: defaults.look,
            tint: palette,
        },
        SpaceOverride::Custom(custom) => Resolved {
            look: custom.look,
            tint: custom.color.or(palette),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn teal() -> Rgba {
        Rgba::new(0x4fb3a4, Opacity::OPAQUE)
    }

    #[test]
    fn intensity_is_clamped_to_the_cap() {
        assert_eq!(Intensity::from(250).percent(), MAX_INTENSITY);
        assert_eq!(Intensity::from(-4).percent(), 0);
        assert_eq!(Intensity::from(33).percent(), 33);
        assert_eq!(Intensity::from(55).stepped(true).percent(), MAX_INTENSITY);
        assert_eq!(Intensity::from(3).stepped(false).percent(), 0);
    }

    #[test]
    fn a_space_follows_the_default_until_it_has_an_override() {
        let defaults = SpaceAppearance {
            look: Look {
                style: SpaceStyle::Border,
                ..Look::default()
            },
            ..SpaceAppearance::default()
        };
        let followed = resolve(&defaults, SpaceColor::Teal, &SpaceOverride::Default);
        assert_eq!(followed.look.style, SpaceStyle::Border);
        assert_eq!(followed.tint, Some(teal()));
        let own = SpaceOverride::Custom(Custom {
            look: Look {
                style: SpaceStyle::FullTint,
                intensity: Intensity::from(40),
                apply_to: ApplyTo::HeaderOnly,
            },
            color: None,
        });
        let custom = resolve(&defaults, SpaceColor::Teal, &own);
        assert_eq!(custom.look.style, SpaceStyle::FullTint);
        assert_eq!(custom.look.intensity.percent(), 40);
        assert_eq!(custom.tint, Some(teal()));
    }

    #[test]
    fn a_custom_color_beats_the_palette_and_dropping_it_restores_the_palette() {
        let free = Rgba::new(0x123456, Opacity::from(50));
        let own = SpaceOverride::Custom(Custom {
            look: Look::default(),
            color: Some(free),
        });
        let defaults = SpaceAppearance::default();
        assert_eq!(resolve(&defaults, SpaceColor::Teal, &own).tint, Some(free));
        let back = own.with_palette_color();
        assert_eq!(
            resolve(&defaults, SpaceColor::Teal, &back).tint,
            Some(teal())
        );
        assert_eq!(
            SpaceOverride::Default.with_palette_color(),
            SpaceOverride::Default
        );
    }

    #[test]
    fn apply_to_round_trips_through_its_two_flags() {
        for apply_to in [
            ApplyTo::Both,
            ApplyTo::HeaderOnly,
            ApplyTo::WorkbooksOnly,
            ApplyTo::Neither,
        ] {
            assert_eq!(
                ApplyTo::of(apply_to.header(), apply_to.workbooks()),
                apply_to
            );
        }
    }

    #[test]
    fn out_of_range_numbers_in_json_are_clamped_not_rejected() {
        let appearance: SpaceAppearance =
            serde_json::from_str(r#"{"look":{"intensity":250}}"#).unwrap();
        assert_eq!(appearance.look.intensity.percent(), MAX_INTENSITY);
        let custom: Custom =
            serde_json::from_str(r#"{"look":{},"color":{"rgb":1193046,"opacity":900}}"#).unwrap();
        assert_eq!(custom.color.unwrap().opacity.percent(), 100);
    }

    #[test]
    fn an_override_survives_the_json_round_trip() {
        for original in [
            SpaceOverride::Default,
            SpaceOverride::Custom(Custom {
                look: Look {
                    style: SpaceStyle::FullTint,
                    intensity: Intensity::from(30),
                    apply_to: ApplyTo::WorkbooksOnly,
                },
                color: Some(Rgba::new(0xabcdef, Opacity::from(70))),
            }),
        ] {
            let text = serde_json::to_string(&original).unwrap();
            assert_eq!(
                serde_json::from_str::<SpaceOverride>(&text).unwrap(),
                original
            );
        }
        let defaults = SpaceAppearance {
            look: Look {
                style: SpaceStyle::Border,
                intensity: Intensity::from(22),
                apply_to: ApplyTo::HeaderOnly,
            },
            new_space_color: NewSpaceColor::None,
        };
        let text = serde_json::to_string(&defaults).unwrap();
        assert_eq!(
            serde_json::from_str::<SpaceAppearance>(&text).unwrap(),
            defaults
        );
    }
}
