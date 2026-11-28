use gpui_kit::{Hsla, Rgba};

fn channel(value: f32) -> f32 {
    if value <= 0.03928 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(color: Rgba) -> f32 {
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

fn over(foreground: Hsla, background: Hsla) -> Rgba {
    let fg = foreground.to_rgb();
    let bg = background.to_rgb();
    let mix = |f: f32, b: f32| f * fg.a + b * (1.0 - fg.a);
    Rgba {
        r: mix(fg.r, bg.r),
        g: mix(fg.g, bg.g),
        b: mix(fg.b, bg.b),
        a: 1.0,
    }
}

// WCAG's contrast ratio of `text` drawn over `background`.
pub fn ratio(text: Hsla, background: Hsla) -> f32 {
    let a = luminance(over(text, background)) + 0.05;
    let b = luminance(background.to_rgb()) + 0.05;
    a.max(b) / a.min(b)
}

const STEPS: u8 = 100;

// `color` as it is when it reads at `minimum` on `background`, else mixed toward `toward`
// by the least amount that does. This keeps a theme's own red where it is readable and
// darkens or lightens it only as far as the theme needs.
pub fn readable(color: Hsla, background: Hsla, toward: Hsla, minimum: f32) -> Hsla {
    if ratio(color, background) >= minimum {
        return color;
    }
    let (from, to) = (color.to_rgb(), toward.to_rgb());
    let mixed = |step: u8| -> Hsla {
        let at = f32::from(step) / f32::from(STEPS);
        let part = |a: f32, b: f32| a + (b - a) * at;
        Rgba {
            r: part(from.r, to.r),
            g: part(from.g, to.g),
            b: part(from.b, to.b),
            a: 1.0,
        }
        .into()
    };
    (1..=STEPS)
        .map(mixed)
        .find(|candidate| ratio(*candidate, background) >= minimum)
        .unwrap_or(toward)
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Hsla, rgb};

    use super::{ratio, readable};

    fn hex(value: u32) -> Hsla {
        rgb(value).into()
    }

    #[test]
    fn black_on_white_is_21_to_1() {
        assert!((ratio(hex(0x000000), hex(0xFFFFFF)) - 21.0).abs() < 0.01);
    }

    #[test]
    fn a_colour_that_already_reads_is_left_alone() {
        let red = hex(0xF87171);
        assert_eq!(readable(red, hex(0x0A0A0A), hex(0xFAFAFA), 4.5), red);
    }

    #[test]
    fn the_stock_red_on_white_is_darkened_to_4_5_to_1_and_no_more_than_needed() {
        let stock = hex(0xEF4444);
        let white = hex(0xFFFFFF);
        assert!(ratio(stock, white) < 4.5);
        let text = readable(stock, white, hex(0x0A0A0A), 4.5);
        assert!(ratio(text, white) >= 4.5);
        let rgb = text.to_rgb();
        assert!(rgb.r > 0.7 && rgb.g < 0.35, "{rgb:?}");
    }

    #[test]
    fn a_colour_that_cannot_reach_the_ratio_becomes_the_one_it_is_pulled_toward() {
        let grey = hex(0x808080);
        let result = readable(grey, hex(0x888888), hex(0x000000), 21.0);
        assert_eq!(result.to_rgb().r, 0.0);
    }
}
