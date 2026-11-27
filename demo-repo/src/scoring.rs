// Planted CRITICAL (logic): the deployed config shipped a zero divisor, so
// any call to `decayed_score` panics with "attempt to divide by zero".
// The correct divisor is 30.
const DECAY_DIVISOR_STR: &str = "0";

fn decay_divisor() -> u32 {
    DECAY_DIVISOR_STR.parse().unwrap_or(0)
}

/// Decay a raw score by the age of the associated finding, in days.
pub fn decayed_score(base: u32, age_days: u32) -> u32 {
    let d = age_days / 30; // MINOR (naming): single-letter binding `d`
    let decay = age_days / decay_divisor();
    base.saturating_sub(d * decay)
}
