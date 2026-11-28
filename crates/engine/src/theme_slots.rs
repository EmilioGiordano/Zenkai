use ironcalc::base::types::Theme;

fn is_hex_color(slot: &str) -> bool {
    slot.strip_prefix('#')
        .is_some_and(|digits| digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
}

// The model resolves a theme color by slicing its slot as `#RRGGBB`, which a file's own text
// (a malformed or missing color in the theme part) would make panic. A slot that is not
// `#RRGGBB` falls back to the default palette.
pub(crate) fn sanitize(theme: &mut Theme) {
    let default = Theme::default();
    let slots = [
        (&mut theme.dk1, &default.dk1),
        (&mut theme.lt1, &default.lt1),
        (&mut theme.dk2, &default.dk2),
        (&mut theme.lt2, &default.lt2),
        (&mut theme.accent1, &default.accent1),
        (&mut theme.accent2, &default.accent2),
        (&mut theme.accent3, &default.accent3),
        (&mut theme.accent4, &default.accent4),
        (&mut theme.accent5, &default.accent5),
        (&mut theme.accent6, &default.accent6),
        (&mut theme.hlink, &default.hlink),
        (&mut theme.fol_hlink, &default.fol_hlink),
    ];
    for (slot, fallback) in slots {
        if !is_hex_color(slot) {
            slot.clone_from(fallback);
        }
    }
}

#[cfg(test)]
mod tests {
    use ironcalc::base::types::Theme;

    use super::sanitize;

    #[test]
    fn a_slot_that_is_not_a_hex_color_falls_back_to_the_default() {
        let default = Theme::default();
        for bad in [
            "#ZZ",
            "",
            "#WINDOWTEXT",
            "#12345",
            "#1234567",
            "#\u{e9}12345",
            "4472C4",
        ] {
            let mut theme = Theme {
                accent1: bad.to_string(),
                hlink: bad.to_string(),
                ..Theme::default()
            };
            sanitize(&mut theme);
            assert_eq!(theme.accent1, default.accent1, "{bad:?}");
            assert_eq!(theme.hlink, default.hlink, "{bad:?}");
        }
    }

    #[test]
    fn valid_slots_are_kept() {
        let mut theme = Theme {
            accent2: "#12ab34".to_string(),
            ..Theme::default()
        };
        sanitize(&mut theme);
        assert_eq!(theme.accent2, "#12ab34");
    }
}
