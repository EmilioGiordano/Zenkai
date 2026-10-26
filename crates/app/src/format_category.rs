#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatCategory {
    General,
    Number,
    Currency,
    Accounting,
    Date,
    Time,
    Percentage,
    Fraction,
    Scientific,
    Text,
    Custom,
}

impl FormatCategory {
    pub fn of(_code: &str) -> Self {
        Self::Custom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_codes_are_percentage() {
        assert_eq!(FormatCategory::of("0%"), FormatCategory::Percentage);
        assert_eq!(FormatCategory::of("0.00%"), FormatCategory::Percentage);
    }
}
