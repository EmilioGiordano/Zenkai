use crate::panel_width::{KEY_STEP, Resize};

const MINIMUM: f32 = 120.0;
const ABSOLUTE_MAXIMUM: f32 = 600.0;
pub const DEFAULT: f32 = 200.0;

// The panel may fill half of the PDF view, up to an absolute cap; `view` is the whole view width.
pub fn limits(view: f32) -> (f32, f32) {
    (MINIMUM, ABSOLUTE_MAXIMUM.min(view / 2.0).max(MINIMUM))
}

pub fn within(requested: f32, view: f32) -> f32 {
    let (min, max) = limits(view);
    requested.clamp(min, max)
}

// A stored value out of range, or not a number, goes back to the default.
pub fn stored(value: f32) -> f32 {
    if (MINIMUM..=ABSOLUTE_MAXIMUM).contains(&value) {
        value
    } else {
        DEFAULT
    }
}

pub fn stepped(current: f32, resize: Resize, view: f32) -> f32 {
    within(
        match resize {
            Resize::Wider => current + KEY_STEP,
            Resize::Narrower => current - KEY_STEP,
        },
        view,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_starts_at_200_and_never_goes_narrower_than_its_minimum() {
        assert_eq!(DEFAULT, 200.0);
        assert_eq!(limits(2000.0).0, 120.0);
        assert_eq!(within(10.0, 1000.0), 120.0);
    }

    #[test]
    fn the_panel_stops_at_half_of_its_view_and_at_its_absolute_maximum() {
        assert_eq!(within(5000.0, 700.0), 350.0);
        assert_eq!(within(5000.0, 4000.0), 600.0);
        assert_eq!(within(300.0, 100.0), 120.0);
        assert_eq!(limits(700.0), (120.0, 350.0));
        assert_eq!(within(5000.0, f32::INFINITY), 600.0);
    }

    #[test]
    fn a_key_step_moves_sixteen_pixels_and_stops_at_the_limits() {
        assert_eq!(stepped(120.0, Resize::Wider, 2000.0), 120.0 + KEY_STEP);
        assert_eq!(stepped(120.0, Resize::Narrower, 2000.0), 120.0);
        assert_eq!(stepped(600.0, Resize::Wider, 2000.0), 600.0);
    }

    #[test]
    fn a_stored_width_out_of_range_or_not_a_number_is_the_default() {
        assert_eq!(stored(300.0), 300.0);
        assert_eq!(stored(5.0), 200.0);
        assert_eq!(stored(9000.0), 200.0);
        assert_eq!(stored(f32::NAN), 200.0);
    }
}
