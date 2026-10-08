use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Percent(u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0} is not a percentage between 0 and 100")]
pub struct InvalidPercent(pub u8);

impl Percent {
    pub const ZERO: Percent = Percent(0);

    pub fn new(value: u8) -> Result<Percent, InvalidPercent> {
        if value <= 100 {
            Ok(Percent(value))
        } else {
            Err(InvalidPercent(value))
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }

    pub(crate) fn of(self, total: usize) -> usize {
        (total * usize::from(self.0) + 50) / 100
    }
}

impl TryFrom<u8> for Percent {
    type Error = InvalidPercent;

    fn try_from(value: u8) -> Result<Percent, InvalidPercent> {
        Percent::new(value)
    }
}

impl From<Percent> for u8 {
    fn from(percent: Percent) -> u8 {
        percent.0
    }
}

impl JsonSchema for Percent {
    fn schema_name() -> Cow<'static, str> {
        "Percent".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "minimum": 0,
            "maximum": 100,
            "description": "Whole percentage from 0 to 100."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_above_one_hundred() {
        assert_eq!(Percent::new(101), Err(InvalidPercent(101)));
        assert_eq!(Percent::new(100).map(Percent::get), Ok(100));
    }

    #[test]
    fn share_rounds_to_nearest() {
        let five = Percent::new(5).unwrap();
        assert_eq!(five.of(1000), 50);
        assert_eq!(five.of(10), 1);
        assert_eq!(five.of(9), 0);
        assert_eq!(Percent::new(100).unwrap().of(7), 7);
    }
}
