// Dates written day first ("25/10/2026", "8-10-26"), as Excel exports them in most locales
// outside the US. The engine reads slashed dates month first, so day-first fields are
// rewritten to year first (2026/10/25) before import; anything else is left alone. The slash
// is deliberate: the engine gives that text its own number format, `yyyy/mm/dd`, which a
// date written as ISO with dashes in the same file does not have, so a save can tell the
// dates it rewrote from those it did not.

pub const NORMALIZED_NUMBER_FORMAT: &str = "yyyy/mm/dd";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateOrder {
    DayFirst,
    MonthFirst,
}

/// The order the dates prove (a part above 12), or `None` when every date
/// reads both ways.
pub fn detect_date_order(rows: &[Vec<String>]) -> Option<DateOrder> {
    let mut found = None;
    for field in rows.iter().flatten() {
        let Some((first, second, _)) = parts(field) else {
            continue;
        };
        let order = match (first > 12, second > 12) {
            (true, false) => DateOrder::DayFirst,
            (false, true) => DateOrder::MonthFirst,
            _ => continue,
        };
        match found {
            None => found = Some(order),
            Some(seen) if seen != order => return None,
            Some(_) => {}
        }
    }
    found
}

/// Rewrites every valid day-first date to year first with slashes; returns how many fields
/// changed.
pub fn normalize_day_first(rows: &mut [Vec<String>]) -> usize {
    let mut changed = 0;
    for field in rows.iter_mut().flatten() {
        if let Some((day, month, year)) = parts(field)
            && valid(day, month, year)
        {
            *field = format!("{year:04}/{month:02}/{day:02}");
            changed += 1;
        }
    }
    changed
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateSeparator {
    Slash,
    Dash,
    Dot,
}

impl DateSeparator {
    fn of(separator: char) -> Option<DateSeparator> {
        match separator {
            '/' => Some(DateSeparator::Slash),
            '-' => Some(DateSeparator::Dash),
            '.' => Some(DateSeparator::Dot),
            _ => None,
        }
    }

    pub fn char(self) -> char {
        match self {
            DateSeparator::Slash => '/',
            DateSeparator::Dash => '-',
            DateSeparator::Dot => '.',
        }
    }
}

/// How the day-first dates of a file were written, to write them back the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateLayout {
    pub separator: DateSeparator,
    /// "08/10/2026" rather than "8/10/2026".
    pub padded: bool,
    /// "08/10/26" rather than "08/10/2026".
    pub two_digit_year: bool,
}

/// The layout of the first valid day-first date; padding is off if any date of that
/// separator has a one-digit day or month. `None` when the file has no such date.
pub fn detect_date_layout(rows: &[Vec<String>]) -> Option<DateLayout> {
    let mut layout: Option<DateLayout> = None;
    let mut unpadded = false;
    for field in rows.iter().flatten() {
        let Some((separator, day, month, year)) = shape(field) else {
            continue;
        };
        let Some((d, m, y)) = parts(field) else {
            continue;
        };
        let Some(separator) = DateSeparator::of(separator) else {
            continue;
        };
        if !valid(d, m, y) {
            continue;
        }
        let first = layout.get_or_insert(DateLayout {
            separator,
            padded: true,
            two_digit_year: year.len() == 2,
        });
        unpadded |= first.separator == separator && (day.len() == 1 || month.len() == 1);
    }
    layout.map(|layout| DateLayout {
        padded: !unpadded,
        ..layout
    })
}

impl DateLayout {
    /// Turns a date as `normalize_day_first` wrote it ("2026/10/08") back into this layout.
    /// A year outside Excel's two-digit window is written in full.
    pub fn day_first_text(self, normalized: &str) -> Option<String> {
        let mut pieces = normalized.split('/');
        let (year, month, day) = (pieces.next()?, pieces.next()?, pieces.next()?);
        let digits = |text: &str, length| {
            (text.len() == length && text.bytes().all(|b| b.is_ascii_digit()))
                .then(|| text.parse::<u32>().ok())
                .flatten()
        };
        if pieces.next().is_some() {
            return None;
        }
        let (year, month, day) = (digits(year, 4)?, digits(month, 2)?, digits(day, 2)?);
        let year = if self.two_digit_year && (1930..=2029).contains(&year) {
            format!("{:02}", year % 100)
        } else {
            format!("{year:04}")
        };
        let (day, month) = if self.padded {
            (format!("{day:02}"), format!("{month:02}"))
        } else {
            (day.to_string(), month.to_string())
        };
        let separator = self.separator.char();
        Some(format!("{day}{separator}{month}{separator}{year}"))
    }
}

// The three parts around one separator, as written: "25/10/2026" is ('/', "25", "10", "2026").
fn shape(field: &str) -> Option<(char, &str, &str, &str)> {
    let separator = field.chars().find(|c| matches!(c, '/' | '-' | '.'))?;
    let mut pieces = field.split(separator);
    let (a, b, c) = (pieces.next()?, pieces.next()?, pieces.next()?);
    pieces.next().is_none().then_some((separator, a, b, c))
}

// "d/m/y" with one or two digits for d and m, two or four for y, and the same separator
// ('/', '-' or '.') twice. Two-digit years follow Excel: 00-29 are 2000s, 30-99 are 1900s.
fn parts(field: &str) -> Option<(u32, u32, u32)> {
    let (separator, a, b, c) = shape(field)?;
    let number = |text: &str, lengths: &[usize]| {
        (lengths.contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<u32>().ok())
            .flatten()
    };
    let first = number(a, &[1, 2])?;
    let second = number(b, &[1, 2])?;
    // "3.4.10" is more likely a version than a date: dots need a four-digit year.
    let year = number(c, if separator == '.' { &[4] } else { &[2, 4] })?;
    let year = match (c.len(), year) {
        (2, y) if y < 30 => 2000 + y,
        (2, y) => 1900 + y,
        (_, y) => y,
    };
    (first >= 1 && second >= 1).then_some((first, second, year))
}

fn valid(day: u32, month: u32, year: u32) -> bool {
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1900..=9999).contains(&year) && day <= days
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(fields: &[&str]) -> Vec<Vec<String>> {
        vec![fields.iter().map(ToString::to_string).collect()]
    }

    #[test]
    fn detects_order_only_when_a_part_proves_it() {
        assert_eq!(
            detect_date_order(&rows(&["08/10/2026", "25/10/2026"])),
            Some(DateOrder::DayFirst)
        );
        assert_eq!(
            detect_date_order(&rows(&["10/25/2026"])),
            Some(DateOrder::MonthFirst)
        );
        assert_eq!(detect_date_order(&rows(&["08/10/2026", "x"])), None);
        assert_eq!(
            detect_date_order(&rows(&["25/10/2026", "10/25/2026"])),
            None
        );
    }

    #[test]
    fn rewrites_valid_day_first_dates_to_year_first() {
        let mut data = rows(&[
            "25/10/2026",
            "8-1-26",
            "1.2.1995",
            "29/02/2024",
            "31/04/2026",
            "1/2/3/4",
            "12,5",
            "Total",
        ]);
        assert_eq!(normalize_day_first(&mut data), 4);
        assert_eq!(
            data[0],
            [
                "2026/10/25",
                "2026/01/08",
                "1995/02/01",
                "2024/02/29",
                "31/04/2026",
                "1/2/3/4",
                "12,5",
                "Total"
            ]
        );
    }

    #[test]
    fn detects_how_the_day_first_dates_were_written() {
        let layout = |fields: &[&str]| detect_date_layout(&rows(fields));
        assert_eq!(
            layout(&["08/10/2026", "25/10/2026"]),
            Some(DateLayout {
                separator: DateSeparator::Slash,
                padded: true,
                two_digit_year: false,
            })
        );
        assert_eq!(
            layout(&["25/10/2026", "8/1/26"]),
            Some(DateLayout {
                separator: DateSeparator::Slash,
                padded: false,
                two_digit_year: false,
            })
        );
        assert_eq!(
            layout(&["x", "31/04/2026", "8-1-26"]),
            Some(DateLayout {
                separator: DateSeparator::Dash,
                padded: false,
                two_digit_year: true,
            })
        );
        assert_eq!(layout(&["2026-10-25", "Total", "1/2/3/4"]), None);
    }

    #[test]
    fn a_rewritten_date_goes_back_to_the_layout_it_was_read_in() {
        for dates in [
            ["08/10/2026", "25/10/2026", "01/01/1999"],
            ["8/10/2026", "25/10/2026", "1/1/1999"],
            ["08-10-26", "25-10-26", "01-01-99"],
            ["8.10.2026", "25.10.2026", "1.1.1999"],
        ] {
            let mut data = rows(&dates);
            let layout = detect_date_layout(&data).unwrap();
            assert_eq!(normalize_day_first(&mut data), 3);
            let back: Vec<String> = data[0]
                .iter()
                .map(|field| layout.day_first_text(field).unwrap())
                .collect();
            assert_eq!(back, dates);
        }
    }

    #[test]
    fn only_text_written_by_the_rewrite_goes_back() {
        let layout = DateLayout {
            separator: DateSeparator::Slash,
            padded: true,
            two_digit_year: true,
        };
        for text in [
            "2026-10-25",
            "25/10/2026",
            "2026/10/2",
            "2026/1x/25",
            "x",
            "",
        ] {
            assert_eq!(layout.day_first_text(text), None, "{text}");
        }
        assert_eq!(
            layout.day_first_text("1850/10/25").as_deref(),
            Some("25/10/1850")
        );
    }

    #[test]
    fn mixed_separators_and_lengths_are_not_dates() {
        for field in [
            "1/2-2026",
            "123/1/2026",
            "1/1/202",
            "0/1/2026",
            "a/b/c",
            "1/1",
        ] {
            assert_eq!(parts(field), None, "{field}");
        }
    }
}

#[cfg(test)]
mod no_panic {
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 2_000, ..ProptestConfig::default() })]

        #[test]
        fn date_rewrite_accepts_any_field(field in prop_oneof![any::<String>(), "[0-9/.-]{0,12}"]) {
            let mut rows = vec![vec![field]];
            let _ = super::detect_date_order(&rows);
            if let Some(layout) = super::detect_date_layout(&rows) {
                let _ = layout.day_first_text(&rows[0][0]);
            }
            let _ = super::normalize_day_first(&mut rows);
        }
    }
}
