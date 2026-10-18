use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecalcMode {
    Full,
    Incremental,
}

// How long the parts of one cell edit took, for the edit benchmark in `bench/`.
#[derive(Clone, Copy, Debug)]
pub struct EditProfile {
    pub edit: Duration,
    pub recalculation: Duration,
    pub mode: RecalcMode,
    pub warm_used_areas: Duration,
}
