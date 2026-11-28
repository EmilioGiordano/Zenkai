use zenkai_i18n::t;
use zenkai_types::Rgb;

pub(crate) const COLUMNS: usize = 10;
pub(crate) const THEME_ROWS: usize = 6;
pub(crate) const SWATCHES: usize = COLUMNS * (THEME_ROWS + 1);

const BLACK: Rgb = Rgb(0x00_00_00);
const WHITE: Rgb = Rgb(0xFF_FF_FF);

// The Office 2013 to 2022 theme as the board TabColor.html lists it: one row per tint and
// one column per theme color.
const THEME: [[u32; COLUMNS]; THEME_ROWS] = [
    [
        0xFFFFFF, 0x000000, 0xE7E6E6, 0x44546A, 0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5,
        0x70AD47,
    ],
    [
        0xF2F2F2, 0x808080, 0xD0CECE, 0xD6DCE5, 0xDAE3F3, 0xFBE5D6, 0xEDEDED, 0xFFF2CC, 0xDEEBF7,
        0xE2F0D9,
    ],
    [
        0xD9D9D9, 0x595959, 0xAFABAB, 0xADB9CA, 0xB4C7E7, 0xF8CBAD, 0xDBDBDB, 0xFFE699, 0xBDD7EE,
        0xC5E0B4,
    ],
    [
        0xBFBFBF, 0x404040, 0x767171, 0x8497B0, 0x8FAADC, 0xF4B183, 0xC9C9C9, 0xFFD966, 0x9DC3E6,
        0xA9D18E,
    ],
    [
        0xA6A6A6, 0x262626, 0x3B3838, 0x333F50, 0x2F5597, 0xC55A11, 0x7C7C7C, 0xBF9000, 0x2E75B6,
        0x548235,
    ],
    [
        0x808080, 0x0D0D0D, 0x181717, 0x222A35, 0x203864, 0x843C0B, 0x525252, 0x806000, 0x1F4E79,
        0x385723,
    ],
];

const STANDARD: [u32; COLUMNS] = [
    0xC00000, 0xFF0000, 0xFFC000, 0xFFFF00, 0x92D050, 0x00B050, 0x00B0F0, 0x0070C0, 0x002060,
    0x7030A0,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shade {
    Base,
    Lighter(u8),
    Darker(u8),
}

fn shade(row: usize, col: usize) -> Shade {
    const WHITE_DARKER: [u8; 5] = [5, 15, 25, 35, 50];
    const BLACK_LIGHTER: [u8; 5] = [50, 35, 25, 15, 5];
    const GRAY_DARKER: [u8; 5] = [10, 25, 50, 75, 90];
    const ACCENT: [Shade; 5] = [
        Shade::Lighter(80),
        Shade::Lighter(60),
        Shade::Lighter(40),
        Shade::Darker(25),
        Shade::Darker(50),
    ];
    let Some(step) = row.checked_sub(1) else {
        return Shade::Base;
    };
    match col {
        0 => Shade::Darker(WHITE_DARKER[step]),
        1 => Shade::Lighter(BLACK_LIGHTER[step]),
        2 => Shade::Darker(GRAY_DARKER[step]),
        _ => ACCENT[step],
    }
}

// One cell of the two grids the card shows; `row` and `col` are always inside them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Swatch {
    Theme { row: usize, col: usize },
    Standard { col: usize },
}

impl Swatch {
    pub(crate) const FIRST: Swatch = Swatch::Theme { row: 0, col: 0 };

    pub(crate) fn all() -> impl Iterator<Item = Swatch> {
        (0..THEME_ROWS)
            .flat_map(|row| (0..COLUMNS).map(move |col| Swatch::Theme { row, col }))
            .chain((0..COLUMNS).map(|col| Swatch::Standard { col }))
    }

    // The first cell that holds `color`, theme grid first and in reading order.
    pub(crate) fn of(color: Rgb) -> Option<Swatch> {
        Swatch::all().find(|swatch| swatch.rgb() == color)
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Swatch::Theme { row, col } => row * COLUMNS + col,
            Swatch::Standard { col } => THEME_ROWS * COLUMNS + col,
        }
    }

    pub(crate) fn rgb(self) -> Rgb {
        match self {
            Swatch::Theme { row, col } => Rgb(THEME[row][col]),
            Swatch::Standard { col } => Rgb(STANDARD[col]),
        }
    }

    pub(crate) fn name(self) -> String {
        match self {
            Swatch::Standard { col } => t!("tab_color.standard_names")
                .split(", ")
                .nth(col)
                .unwrap_or_default()
                .to_string(),
            Swatch::Theme { row, col } => {
                let base = t!("tab_color.theme_names")
                    .split(" / ")
                    .nth(col)
                    .unwrap_or_default();
                match shade(row, col) {
                    Shade::Base => base.to_string(),
                    Shade::Lighter(percent) => {
                        t!("tab_color.lighter", base = base, n = percent)
                    }
                    Shade::Darker(percent) => {
                        t!("tab_color.darker", base = base, n = percent)
                    }
                }
            }
        }
    }
}

pub(crate) fn hex(color: Rgb) -> String {
    format!("#{:06X}", color.0)
}

fn linear(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.03928 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(color: Rgb) -> f64 {
    let [_, red, green, blue] = color.0.to_be_bytes();
    0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

pub(crate) fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (a, b) = (luminance(a) + 0.05, luminance(b) + 0.05);
    a.max(b) / a.min(b)
}

// Black or white, whichever reads better on the swatch; the worst case still gets 4.58 to 1.
pub(crate) fn glyph_color(swatch: Rgb) -> Rgb {
    if contrast(swatch, BLACK) >= contrast(swatch, WHITE) {
        BLACK
    } else {
        WHITE
    }
}

#[cfg(test)]
mod tests {
    use zenkai_types::Rgb;

    use super::{COLUMNS, SWATCHES, Swatch, contrast, glyph_color, hex};

    fn named(row: usize, col: usize) -> (String, String) {
        let swatch = Swatch::Theme { row, col };
        (swatch.name(), hex(swatch.rgb()))
    }

    #[test]
    fn the_grid_holds_sixty_theme_colors_and_ten_standard_ones() {
        assert_eq!(Swatch::all().count(), SWATCHES);
        assert_eq!(SWATCHES, 70);
        for (position, swatch) in Swatch::all().enumerate() {
            assert_eq!(swatch.index(), position);
        }
    }

    #[test]
    fn the_first_row_names_the_ten_theme_colors() {
        let names: Vec<String> = (0..COLUMNS).map(|col| named(0, col).0).collect();
        assert_eq!(
            names,
            [
                "White, Background 1",
                "Black, Text 1",
                "Light Gray, Background 2",
                "Blue-Gray, Text 2",
                "Blue, Accent 1",
                "Orange, Accent 2",
                "Gray, Accent 3",
                "Gold, Accent 4",
                "Blue, Accent 5",
                "Green, Accent 6",
            ]
        );
        assert_eq!(named(0, 4).1, "#4472C4");
        assert_eq!(named(0, 9).1, "#70AD47");
    }

    #[test]
    fn tints_follow_the_percentages_of_each_column() {
        let column = |col: usize| -> Vec<String> {
            (1..6)
                .map(|row| named(row, col).0.rsplit(", ").next().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            column(0),
            [
                "Darker 5%",
                "Darker 15%",
                "Darker 25%",
                "Darker 35%",
                "Darker 50%"
            ]
        );
        assert_eq!(
            column(1),
            [
                "Lighter 50%",
                "Lighter 35%",
                "Lighter 25%",
                "Lighter 15%",
                "Lighter 5%"
            ]
        );
        assert_eq!(
            column(2),
            [
                "Darker 10%",
                "Darker 25%",
                "Darker 50%",
                "Darker 75%",
                "Darker 90%"
            ]
        );
        for col in 3..COLUMNS {
            assert_eq!(
                column(col),
                [
                    "Lighter 80%",
                    "Lighter 60%",
                    "Lighter 40%",
                    "Darker 25%",
                    "Darker 50%"
                ]
            );
        }
        assert_eq!(
            named(2, 4),
            (
                "Blue, Accent 1, Lighter 60%".to_string(),
                "#B4C7E7".to_string()
            )
        );
        assert_eq!(
            named(5, 9),
            (
                "Green, Accent 6, Darker 50%".to_string(),
                "#385723".to_string()
            )
        );
    }

    #[test]
    fn the_standard_row_has_the_ten_standard_colors() {
        let expected = [
            ("Dark Red", "#C00000"),
            ("Red", "#FF0000"),
            ("Orange", "#FFC000"),
            ("Yellow", "#FFFF00"),
            ("Light Green", "#92D050"),
            ("Green", "#00B050"),
            ("Light Blue", "#00B0F0"),
            ("Blue", "#0070C0"),
            ("Dark Blue", "#002060"),
            ("Purple", "#7030A0"),
        ];
        for (col, wanted) in expected.into_iter().enumerate() {
            let swatch = Swatch::Standard { col };
            assert_eq!((swatch.name().as_str(), hex(swatch.rgb()).as_str()), wanted);
        }
    }

    #[test]
    fn a_color_that_appears_twice_marks_the_first_theme_cell() {
        assert_eq!(
            Swatch::of(Rgb(0xFFC000)),
            Some(Swatch::Theme { row: 0, col: 7 })
        );
        assert_eq!(
            Swatch::of(Rgb(0x808080)),
            Some(Swatch::Theme { row: 1, col: 1 })
        );
        assert_eq!(Swatch::of(Rgb(0x7030A0)), Some(Swatch::Standard { col: 9 }));
        assert_eq!(Swatch::of(Rgb(0x123456)), None);
    }

    #[test]
    fn the_check_glyph_reads_on_every_swatch() {
        for swatch in Swatch::all() {
            let ratio = contrast(swatch.rgb(), glyph_color(swatch.rgb()));
            assert!(
                ratio >= 4.58,
                "{} on {}: {ratio}",
                hex(glyph_color(swatch.rgb())),
                hex(swatch.rgb())
            );
        }
    }

    #[test]
    fn the_glyph_is_black_on_light_swatches_and_white_on_dark_ones() {
        assert_eq!(glyph_color(Rgb(0xFFFFFF)), Rgb(0x000000));
        assert_eq!(glyph_color(Rgb(0xFFFF00)), Rgb(0x000000));
        assert_eq!(glyph_color(Rgb(0x000000)), Rgb(0xFFFFFF));
        assert_eq!(glyph_color(Rgb(0x002060)), Rgb(0xFFFFFF));
    }
}
