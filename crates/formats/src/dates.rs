// Dates written day first ("25/10/2026", "8-10-26"), as Excel exports them in most locales
// outside the US. The engine reads slashed dates month first, so day-first fields are
// rewritten to ISO (2026-10-25) before import; anything else is left alone.

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

/// Rewrites every valid day-first date to ISO; returns how many fields changed.
pub fn normalize_day_first(rows: &mut [Vec<String>]) -> usize {
    let mut changed = 0;
    for field in rows.iter_mut().flatten() {
        if let Some((day, month, year)) = parts(field)
            && valid(day, month, year)
        {
            *field = format!("{year:04}-{month:02}-{day:02}");
            changed += 1;
        }
    }
    changed
}

// "d/m/y" with one or two digits for d and m, two or four for y, and the same separator
// ('/', '-' or '.') twice. Two-digit years follow Excel: 00-29 are 2000s, 30-99 are 1900s.
fn parts(field: &str) -> Option<(u32, u32, u32)> {
    let separator = field.chars().find(|c| matches!(c, '/' | '-' | '.'))?;
    let mut pieces = field.split(separator);
    let (a, b, c) = (pieces.next()?, pieces.next()?, pieces.next()?);
    if pieces.next().is_some() {
        return None;
    }
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
    fn rewrites_valid_day_first_dates_to_iso() {
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
                "2026-10-25",
                "2026-01-08",
                "1995-02-01",
                "2024-02-29",
                "31/04/2026",
                "1/2/3/4",
                "12,5",
                "Total"
            ]
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
