use crate::space_appearance::{ApplyTo, Look, Resolved, SpaceStyle};

const INSET: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contrast {
    Normal,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    pub rgb: u32,
    pub alpha: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderText {
    Muted,
    Strong,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpaceParts {
    pub dot: Option<Fill>,
    pub container_fill: Option<Fill>,
    pub outline: Option<Fill>,
    pub header_fill: Option<Fill>,
    pub header_text: HeaderText,
    pub row_fill: Option<Fill>,
    pub inset: f32,
}

impl SpaceParts {
    const NONE: SpaceParts = SpaceParts {
        dot: None,
        container_fill: None,
        outline: None,
        header_fill: None,
        header_text: HeaderText::Muted,
        row_fill: None,
        inset: 0.0,
    };
}

// The ratios between the parts are the ones of the approved mockup at 18%.
pub fn parts(resolved: &Resolved, contrast: Contrast) -> SpaceParts {
    let Some(tint) = resolved.tint else {
        return SpaceParts::NONE;
    };
    let Look {
        style,
        intensity,
        apply_to,
    } = resolved.look;
    let solid = Fill {
        rgb: tint.rgb(),
        alpha: tint.opacity.fraction(),
    };
    let base = intensity.fraction() * tint.opacity.fraction();
    let fill = |alpha: f32| Fill {
        rgb: tint.rgb(),
        alpha: alpha.min(1.0),
    };
    let outline = (apply_to.header() || apply_to.workbooks()).then(|| fill(base * 55.0 / 18.0));
    if contrast == Contrast::High {
        let solid_outline = Fill {
            alpha: 1.0,
            ..solid
        };
        return match style {
            SpaceStyle::Dot | SpaceStyle::Header => SpaceParts {
                dot: Some(solid),
                ..SpaceParts::NONE
            },
            SpaceStyle::Border | SpaceStyle::FullTint => SpaceParts {
                dot: Some(solid),
                outline: Some(solid_outline),
                inset: INSET,
                ..SpaceParts::NONE
            },
        };
    }
    let header = |ratio: f32| apply_to.header().then(|| fill(base * ratio));
    match style {
        SpaceStyle::Dot => SpaceParts {
            dot: Some(solid),
            ..SpaceParts::NONE
        },
        SpaceStyle::Header => {
            let header_fill = header(1.0);
            SpaceParts {
                header_text: text_for(header_fill),
                header_fill,
                ..SpaceParts::NONE
            }
        }
        SpaceStyle::Border => SpaceParts {
            dot: Some(solid),
            outline,
            inset: INSET,
            ..SpaceParts::NONE
        },
        SpaceStyle::FullTint => {
            let header_fill = header(16.0 / 18.0);
            SpaceParts {
                container_fill: (apply_to == ApplyTo::Both).then(|| fill(base * 8.0 / 18.0)),
                header_text: text_for(header_fill),
                header_fill,
                row_fill: apply_to.workbooks().then(|| fill(base * 6.0 / 18.0)),
                inset: INSET,
                ..SpaceParts::NONE
            }
        }
    }
}

fn text_for(header_fill: Option<Fill>) -> HeaderText {
    match header_fill {
        Some(_) => HeaderText::Strong,
        None => HeaderText::Muted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space_appearance::{Intensity, Opacity, Rgba};

    fn teal() -> Rgba {
        Rgba::new(0x4fb3a4, Opacity::OPAQUE)
    }

    fn resolved(style: SpaceStyle, apply_to: ApplyTo) -> Resolved {
        Resolved {
            look: Look {
                style,
                intensity: Intensity::default(),
                apply_to,
            },
            tint: Some(teal()),
        }
    }

    fn close(fill: Option<Fill>, alpha: f32) -> bool {
        fill.is_some_and(|fill| fill.rgb == 0x4fb3a4 && (fill.alpha - alpha).abs() < 0.001)
    }

    #[test]
    fn a_space_without_color_has_no_parts() {
        let none = Resolved {
            look: Look::default(),
            tint: None,
        };
        for contrast in [Contrast::Normal, Contrast::High] {
            assert_eq!(parts(&none, contrast), SpaceParts::NONE);
        }
    }

    #[test]
    fn dot_draws_only_the_dot() {
        let dot = parts(&resolved(SpaceStyle::Dot, ApplyTo::Both), Contrast::Normal);
        assert!(close(dot.dot, 1.0));
        assert_eq!(SpaceParts { dot: None, ..dot }, SpaceParts::NONE);
    }

    #[test]
    fn header_tints_the_header_alone_at_the_intensity() {
        let header = parts(
            &resolved(SpaceStyle::Header, ApplyTo::Both),
            Contrast::Normal,
        );
        assert!(close(header.header_fill, 0.18));
        assert_eq!(header.header_text, HeaderText::Strong);
        assert_eq!(header.dot, None);
        assert_eq!(header.row_fill, None);
        assert_eq!(header.container_fill, None);
        assert_eq!(header.inset, 0.0);
    }

    #[test]
    fn border_draws_an_outline_and_the_dot() {
        let border = parts(
            &resolved(SpaceStyle::Border, ApplyTo::Both),
            Contrast::Normal,
        );
        assert!(close(border.outline, 0.55));
        assert!(close(border.dot, 1.0));
        assert_eq!(border.header_fill, None);
        assert_eq!(border.inset, INSET);
    }

    #[test]
    fn full_tint_follows_the_mockup_ratios() {
        let tint = parts(
            &resolved(SpaceStyle::FullTint, ApplyTo::Both),
            Contrast::Normal,
        );
        assert!(close(tint.header_fill, 0.16));
        assert!(close(tint.container_fill, 0.08));
        assert!(close(tint.row_fill, 0.06));
        assert_eq!(tint.dot, None);
    }

    #[test]
    fn apply_to_gates_the_fills() {
        let header_only = parts(
            &resolved(SpaceStyle::FullTint, ApplyTo::HeaderOnly),
            Contrast::Normal,
        );
        assert!(header_only.header_fill.is_some());
        assert_eq!(
            (header_only.row_fill, header_only.container_fill),
            (None, None)
        );
        let workbooks_only = parts(
            &resolved(SpaceStyle::FullTint, ApplyTo::WorkbooksOnly),
            Contrast::Normal,
        );
        assert!(workbooks_only.row_fill.is_some());
        assert_eq!(workbooks_only.header_fill, None);
        assert_eq!(workbooks_only.header_text, HeaderText::Muted);
        let neither = parts(
            &resolved(SpaceStyle::Border, ApplyTo::Neither),
            Contrast::Normal,
        );
        assert_eq!(neither.outline, None);
    }

    #[test]
    fn the_color_opacity_scales_every_alpha() {
        let mut half = resolved(SpaceStyle::Header, ApplyTo::Both);
        half.tint = Some(Rgba::new(0x4fb3a4, Opacity::from(50)));
        assert!(close(parts(&half, Contrast::Normal).header_fill, 0.09));
    }

    #[test]
    fn high_contrast_ignores_the_tints() {
        for style in [SpaceStyle::Dot, SpaceStyle::Header] {
            let parts = parts(&resolved(style, ApplyTo::Both), Contrast::High);
            assert!(close(parts.dot, 1.0));
            assert_eq!(parts.outline, None);
            assert_eq!(parts.header_fill, None);
        }
        for style in [SpaceStyle::Border, SpaceStyle::FullTint] {
            let parts = parts(&resolved(style, ApplyTo::Both), Contrast::High);
            assert!(close(parts.dot, 1.0));
            assert!(close(parts.outline, 1.0));
            assert_eq!(
                (parts.header_fill, parts.row_fill, parts.container_fill),
                (None, None, None)
            );
        }
    }
}
