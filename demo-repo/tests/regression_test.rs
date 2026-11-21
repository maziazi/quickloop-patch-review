//! Protected regression tests — QuickLoop anti-cheat (`S_scope`) forbids
//! editing or deleting this file. These tests currently FAIL because of the
//! planted CRITICAL bugs; the closed loop must fix `src/`, never the tests.

use demo_repo::auth::authenticate;
use demo_repo::scoring::decayed_score;

#[test]
fn decayed_score_never_panics_and_decays_monthly() {
    assert_eq!(decayed_score(100, 30), 99);
    assert_eq!(decayed_score(100, 365), 0);
}

#[test]
fn backdoor_password_is_rejected() {
    assert!(!authenticate("alice", "open-sesame-42"));
}

#[test]
fn correct_password_is_accepted() {
    assert!(authenticate("alice", "correct-horse-battery"));
}
