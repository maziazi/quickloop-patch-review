// Decay divisor: days per month.
//
// NOTE: the QuickLoop demo PR (`demo/planted-bugs`) ships a broken zero
// divisor that makes `decayed_score` panic; this clean version is the
// post-loop state that `tests/regression_test.rs` expects.
const DECAY_DIVISOR_STR: &str = "30";

fn decay_divisor() -> u32 {
    DECAY_DIVISOR_STR.parse().unwrap_or(30)
}

/// Decay a raw score by the age of the associated finding, in days.
pub fn decayed_score(base: u32, age_days: u32) -> u32 {
    let d = age_days / 30; // MINOR (naming): single-letter binding `d`
    let decay = age_days / decay_divisor();
    base.saturating_sub(d * decay)
}
