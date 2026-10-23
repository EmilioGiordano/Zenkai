use serde::{Deserialize, Serialize};

const STEPS: [u16; 12] = [50, 67, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300];
const DEFAULT: u16 = 100;

// How much a Markdown tab scales its text. Always one of the preset steps, so a value written
// by hand or by another version into the session still lands on a size the user can step from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u16", into = "u16")]
pub struct TextZoom(u16);

impl Default for TextZoom {
    fn default() -> TextZoom {
        TextZoom(DEFAULT)
    }
}

impl From<u16> for TextZoom {
    fn from(percent: u16) -> TextZoom {
        let nearest = STEPS
            .iter()
            .copied()
            .min_by_key(|step| step.abs_diff(percent))
            .unwrap_or(DEFAULT);
        TextZoom(nearest)
    }
}

impl From<TextZoom> for u16 {
    fn from(zoom: TextZoom) -> u16 {
        zoom.0
    }
}

impl TextZoom {
    pub fn percent(self) -> u16 {
        self.0
    }

    pub fn factor(self) -> f32 {
        f32::from(self.0) / 100.0
    }

    pub fn is_default(self) -> bool {
        self.0 == DEFAULT
    }

    pub fn larger(self) -> TextZoom {
        STEPS
            .iter()
            .copied()
            .find(|step| *step > self.0)
            .map_or(self, TextZoom)
    }

    pub fn smaller(self) -> TextZoom {
        STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < self.0)
            .map_or(self, TextZoom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zooming_in_walks_the_steps_up_to_the_last() {
        let mut zoom = TextZoom::default();
        let mut seen = vec![zoom.percent()];
        for _ in 0..20 {
            zoom = zoom.larger();
            if seen.last() != Some(&zoom.percent()) {
                seen.push(zoom.percent());
            }
        }
        assert_eq!(seen, [100, 110, 125, 150, 175, 200, 250, 300]);
    }

    #[test]
    fn zooming_out_walks_the_steps_down_to_the_first() {
        let mut zoom = TextZoom::default();
        for _ in 0..20 {
            zoom = zoom.smaller();
        }
        assert_eq!(zoom.percent(), 50);
        assert_eq!(TextZoom::default().smaller().percent(), 90);
    }

    #[test]
    fn a_stored_value_off_the_steps_snaps_to_the_nearest() {
        assert_eq!(TextZoom::from(0).percent(), 50);
        assert_eq!(TextZoom::from(140).percent(), 150);
        assert_eq!(TextZoom::from(u16::MAX).percent(), 300);
        assert!(TextZoom::from(101).is_default());
    }

    #[test]
    fn the_zoom_is_stored_as_a_plain_percent() {
        let json = serde_json::to_string(&TextZoom::default().larger()).unwrap();
        assert_eq!(json, "110");
        let back: TextZoom = serde_json::from_str("175").unwrap();
        assert_eq!(back.percent(), 175);
    }
}
